//! `Session`: one live broadcast. Owns the decision engine and the fast detectors,
//! keeps the rolling transcript for the context judge, and produces `UiEvent`s for
//! the desktop app.
//!
//! Threading model (PLAN.md Phase 2): the media threads, ASR thread, OCR thread and
//! judge thread each send results over channels to one thread that owns `Session`.
//! `Session` itself is single-threaded and synchronous on purpose, so it's trivially
//! testable (see tests/end_to_end.rs).

pub mod eval_label;
pub mod incident;

use airlock_core::{
    Action, Category, DecisionEngine, Detection, Dur, Platform, Policy, Redaction, RedactionId,
    Span, StallFallback, Target, TextBox, Ts, Word,
};
use airlock_detect::{ScreenTextScanner, SpokenPiiDetector, WordlistDetector};
use airlock_llm::JudgeRequest;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const DEFAULT_PROFANITY: &str = include_str!("../../../config/wordlists/profanity.txt");
pub const DEFAULT_SLURS: &str = include_str!("../../../config/wordlists/slurs.txt");
pub const DEFAULT_ALLOWLIST: &str = include_str!("../../../config/wordlists/allowlist.txt");

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub policy: Policy,
    pub platform: Platform,
    /// Real name, town, school, employer, family names... Local only.
    pub private_terms: Vec<String>,
    /// Extra rules passed to the context judge ("don't mention my ex by name").
    pub streamer_rules: Vec<String>,
    pub profanity_list: String,
    pub slur_list: String,
    pub allowlist: String,
    /// Words of already-judged context sent along with each judge request.
    pub judge_context_words: usize,
    /// How much transcript to keep in memory.
    pub transcript_keep: Dur,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            policy: Policy::default(),
            platform: Platform::Twitch,
            private_terms: vec![],
            streamer_rules: vec![],
            profanity_list: DEFAULT_PROFANITY.into(),
            slur_list: DEFAULT_SLURS.into(),
            allowlist: DEFAULT_ALLOWLIST.into(),
            judge_context_words: 60,
            transcript_keep: Dur::from_millis(120_000),
        }
    }
}

/// Everything the UI needs, as a single serializable stream.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum UiEvent {
    Transcript {
        words: Vec<Word>,
    },
    RedactionUpserted {
        redaction: Redaction,
    },
    RedactionAired {
        redaction: Redaction,
    },
    Clock {
        live: Ts,
        playhead: Ts,
    },
    /// Speech recognition went quiet while the mix still had audio, or recovered.
    AnalysisStall {
        active: bool,
        fallback: StallFallback,
        message: String,
    },
}

/// What the judge thread needs for one request (owned, so it can cross threads).
#[derive(Debug, Clone)]
pub struct JudgeWindow {
    pub words: Vec<Word>,
    pub focus_start: usize,
    /// Pass back to `Session::on_judge_result` so the session knows what was covered.
    pub covers_until: Ts,
}

pub struct Session {
    cfg: SessionConfig,
    engine: DecisionEngine,
    wordlist: WordlistDetector,
    pii: SpokenPiiDetector,
    screen: ScreenTextScanner,
    transcript: Vec<Word>,
    judged_until: Ts,
    live: Ts,
    /// Extra hold added by the stall fail-safe. Not removed when words return:
    /// shrinking it would jump the playhead forward and skip audio.
    extra_delay: Dur,
    last_word_at: Option<Ts>,
    stall_latched: bool,
    events: Vec<UiEvent>,
    aired: Vec<Redaction>,
}

impl Session {
    pub fn new(cfg: SessionConfig) -> Self {
        let mut wordlist = WordlistDetector::new();
        wordlist
            .add_list(Category::Profanity, &cfg.profanity_list)
            .add_list(Category::Slur, &cfg.slur_list)
            .add_allowlist(&cfg.allowlist);
        Session {
            engine: DecisionEngine::new(cfg.policy.clone()),
            pii: SpokenPiiDetector::with_private_terms(&cfg.private_terms),
            screen: ScreenTextScanner::default(),
            wordlist,
            transcript: Vec::new(),
            judged_until: Ts::ZERO,
            live: Ts::ZERO,
            extra_delay: Dur::ZERO,
            last_word_at: None,
            stall_latched: false,
            events: Vec::new(),
            aired: Vec::new(),
            cfg,
        }
    }

    pub fn engine(&self) -> &DecisionEngine {
        &self.engine
    }

    pub fn playhead(&self) -> Ts {
        self.live - (self.cfg.policy.delay() + self.extra_delay)
    }

    /// How long audio is currently held. Policy delay plus any stall extension.
    pub fn hold(&self) -> Dur {
        self.cfg.policy.delay() + self.extra_delay
    }

    fn ingest_all(&mut self, detections: Vec<Detection>) {
        let playhead = self.playhead();
        let (late, live): (Vec<_>, Vec<_>) =
            detections.into_iter().partition(|d| d.span.end <= playhead);
        for d in late {
            if let Some(r) = self.engine.file_missed(d) {
                self.events.push(UiEvent::RedactionAired {
                    redaction: r.clone(),
                });
                self.aired.push(r);
            }
        }
        let mut touched: Vec<RedactionId> = live
            .into_iter()
            .filter_map(|d| self.engine.ingest(d))
            .collect();
        touched.sort_unstable();
        touched.dedup();
        for id in touched {
            if let Some(r) = self.engine.get(id) {
                self.events.push(UiEvent::RedactionUpserted {
                    redaction: r.clone(),
                });
            }
        }
    }

    /// New finalized words from ASR.
    pub fn on_words(&mut self, words: Vec<Word>) {
        if words.is_empty() {
            return;
        }
        let mut found = self.wordlist.scan(&words);
        // PII can span batches ("my number is" | "631-555-0123"), so scan a little context.
        let ctx_start = self.transcript.len().saturating_sub(8);
        let mut window: Vec<Word> = self.transcript[ctx_start..].to_vec();
        window.extend(words.iter().cloned());
        let new_from = words[0].span.start;
        found.extend(
            self.pii
                .scan(&window)
                .into_iter()
                .filter(|d| d.span.end > new_from),
        );

        if let Some(end) = words.iter().map(|w| w.span.end).max() {
            self.last_word_at = Some(self.last_word_at.map(|t| t.max(end)).unwrap_or(end));
        }
        self.clear_stall();
        self.transcript.extend(words.iter().cloned());
        self.events.push(UiEvent::Transcript { words });
        self.ingest_all(found);
    }

    /// OCR results for one sampled frame; `span` from `FrameSampler::offer`.
    pub fn on_screen_text(&mut self, boxes: &[TextBox], span: Span) {
        let found = self.screen.scan(boxes, span);
        self.ingest_all(found);
    }

    /// Builds the next context-judge request, or `None` if there are no new words.
    pub fn next_judge_window(&self) -> Option<JudgeWindow> {
        let focus_start = self
            .transcript
            .partition_point(|w| w.span.start < self.judged_until);
        if focus_start >= self.transcript.len() {
            return None;
        }
        let ctx_start = focus_start.saturating_sub(self.cfg.judge_context_words);
        Some(JudgeWindow {
            words: self.transcript[ctx_start..].to_vec(),
            focus_start: focus_start - ctx_start,
            covers_until: self
                .transcript
                .last()
                .map(|w| w.span.end)
                .unwrap_or(self.judged_until),
        })
    }

    /// Fills the judge request from this session. Streamer rules are included.
    /// Private terms are not: they stay in the local detector.
    pub fn judge_request<'a>(&'a self, window: &'a JudgeWindow) -> JudgeRequest<'a> {
        JudgeRequest {
            words: &window.words,
            focus_start: window.focus_start,
            platform: self.cfg.platform,
            delay: self.hold(),
            streamer_rules: &self.cfg.streamer_rules,
        }
    }

    pub fn on_judge_result(&mut self, window: &JudgeWindow, detections: Vec<Detection>) {
        self.judged_until = self.judged_until.max(window.covers_until);
        self.ingest_all(detections);
    }

    /// Advances live media time (newest ingested timestamp). Retires redactions that
    /// have fully aired into the incident log.
    pub fn on_live_time(&mut self, live: Ts) {
        self.live = self.live.max(live);
        let playhead = self.playhead();
        for r in self.engine.retire_before(playhead) {
            self.events.push(UiEvent::RedactionAired {
                redaction: r.clone(),
            });
            self.aired.push(r);
        }
        let keep_from = self.live - self.cfg.transcript_keep;
        let drop = self.transcript.partition_point(|w| w.span.end < keep_from);
        self.transcript.drain(..drop);
        self.events.push(UiEvent::Clock {
            live: self.live,
            playhead,
        });
    }

    /// Mutes everything still in the hold. `blackout_video` also blacks the frame.
    /// Does not extend the delay.
    pub fn dump(&mut self, blackout_video: bool) {
        for id in self.engine.dump(self.live, self.hold(), blackout_video) {
            self.push_upsert(id);
        }
    }

    /// Call after `on_live_time` when a chunk of program audio has been measured.
    /// `energetic` should come from `airlock_asr::audio::has_speech_energy`.
    pub fn note_audio_energy(&mut self, energetic: bool) {
        let limit = Dur::from_millis(self.cfg.policy.stall_after_ms);
        let since_words = self.live - self.last_word_at.unwrap_or(Ts::ZERO);
        let stalled = energetic && self.live > Ts::ZERO && since_words >= limit;
        if !stalled {
            self.clear_stall();
            return;
        }
        match self.cfg.policy.stall_fallback {
            StallFallback::Warn => {}
            StallFallback::Mute => {
                let start = self.last_word_at.unwrap_or(Ts::ZERO).max(self.playhead());
                let id = self.engine.cover_manual(
                    Action::Mute,
                    Target::Audio,
                    Span {
                        start,
                        end: self.live + Dur(1),
                    },
                    "No speech recognition while the mix has audio",
                );
                self.push_upsert(id);
            }
            StallFallback::ExtendDelay => {
                if !self.stall_latched {
                    let step = Dur::from_millis(self.cfg.policy.stall_after_ms);
                    let cap = Dur::from_millis(60_000);
                    if self.hold() + step <= cap {
                        self.extra_delay = self.extra_delay + step;
                        let playhead = self.playhead();
                        self.events.push(UiEvent::Clock {
                            live: self.live,
                            playhead,
                        });
                    }
                }
            }
        }
        if !self.stall_latched {
            self.stall_latched = true;
            let fallback = self.cfg.policy.stall_fallback;
            let message = self.stall_message();
            self.events.push(UiEvent::AnalysisStall {
                active: true,
                fallback,
                message,
            });
        }
    }

    fn stall_message(&self) -> String {
        match self.cfg.policy.stall_fallback {
            StallFallback::Warn => {
                "Speech recognition stalled. Audio is still airing unanalyzed.".into()
            }
            StallFallback::Mute => {
                "Speech recognition stalled. Muting audio still in the delay.".into()
            }
            StallFallback::ExtendDelay => {
                format!(
                    "Speech recognition stalled. Delay extended to {:.1}s.",
                    self.hold().as_secs_f64()
                )
            }
        }
    }

    fn clear_stall(&mut self) {
        if !self.stall_latched {
            return;
        }
        self.stall_latched = false;
        self.events.push(UiEvent::AnalysisStall {
            active: false,
            fallback: self.cfg.policy.stall_fallback,
            message: String::new(),
        });
    }

    pub fn approve(&mut self, id: RedactionId) -> airlock_core::Result<()> {
        self.engine.approve(id)?;
        self.push_upsert(id);
        Ok(())
    }

    pub fn veto(&mut self, id: RedactionId) -> airlock_core::Result<()> {
        self.engine.veto(id)?;
        self.push_upsert(id);
        Ok(())
    }

    fn push_upsert(&mut self, id: RedactionId) {
        if let Some(r) = self.engine.get(id) {
            self.events.push(UiEvent::RedactionUpserted {
                redaction: r.clone(),
            });
        }
    }

    pub fn drain_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.events)
    }

    /// Everything that has aired (for the incident log / VOD export).
    pub fn aired(&self) -> &[Redaction] {
        &self.aired
    }
}
