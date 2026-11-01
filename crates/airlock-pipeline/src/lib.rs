//! `Session`: one live broadcast. Owns the decision engine and the fast detectors,
//! keeps the rolling transcript for the context judge, and produces `UiEvent`s for
//! the desktop app.
//!
//! Threading model (PLAN.md Phase 2): the media threads, ASR thread, OCR thread and
//! judge thread each send results over channels to one thread that owns `Session`.
//! `Session` itself is single-threaded and synchronous on purpose, so it's trivially
//! testable (see tests/end_to_end.rs).

pub mod eval_label;
pub mod eval_run;
pub mod incident;
pub mod judge_sched;
pub mod license;
pub mod studio;
pub mod telemetry;
pub mod terms;
pub mod threads;
pub mod watchdog;

pub use eval_label::{LabeledSpan, parse_labels};
pub use eval_run::{ClipScript, EvalReport, score_scripts};
pub use judge_sched::JudgeScheduler;
pub use license::{Feature, allows, issue_dev_license};
pub use studio::StudioSettings;
pub use telemetry::telemetry_report;
pub use terms::{open_terms, random_key, seal_terms};
pub use threads::{OutputSnapshot, SessionBus, SessionInbox};
pub use watchdog::Watchdog;

use airlock_core::{
    Action, Category, DecisionEngine, DelayRamp, Detection, Dur, Platform, Policy, Redaction,
    RedactionId, Span, StallFallback, Target, TextBox, Ts, Word,
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
    /// A later guess replaced words that have not started airing.
    TranscriptRevised {
        replaced: Vec<Span>,
        words: Vec<Word>,
    },
    RedactionUpserted {
        redaction: Redaction,
    },
    /// A held word-list bleep no longer matches the transcript.
    RedactionRemoved {
        id: RedactionId,
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
    /// A publisher is connected to the local RTMP listener, or it hung up.
    Ingest {
        connected: bool,
    },
    /// The held stream is being sent to the platform, or it is not.
    /// `detail` is safe to show. It never contains a stream key.
    Egress {
        active: bool,
        detail: String,
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
    /// User delay changes move this by at most 5% of new media, so the playhead does not jump.
    ramp: DelayRamp,
    /// Extra hold added by the stall fail-safe. Not removed when words return:
    /// shrinking it would jump the playhead forward and skip audio.
    extra_delay: Dur,
    last_word_at: Option<Ts>,
    /// Media time of the newest audio the recognizer has accepted.
    recognizer_audio: Option<Ts>,
    /// True once Go live has a recognizer loading or running.
    recognizer_ready: bool,
    stall_latched: bool,
    safe_mode: bool,
    /// Manual mute kept open while safe mode is on.
    safe_mute: Option<RedactionId>,
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
            ramp: DelayRamp::new(cfg.policy.delay()),
            extra_delay: Dur::ZERO,
            last_word_at: None,
            recognizer_audio: None,
            recognizer_ready: false,
            stall_latched: false,
            safe_mode: false,
            safe_mute: None,
            events: Vec::new(),
            aired: Vec::new(),
            cfg,
        }
    }

    pub fn engine(&self) -> &DecisionEngine {
        &self.engine
    }

    pub fn transcript(&self) -> &[Word] {
        &self.transcript
    }

    pub fn playhead(&self) -> Ts {
        self.live - (self.cfg.policy.delay() + self.extra_delay)
    }

    /// How long audio is currently held. Policy delay plus any stall extension.
    pub fn hold(&self) -> Dur {
        self.ramp.current() + self.extra_delay
    }

    /// Aims the hold at a new delay. The change is applied as live media arrives.
    pub fn set_hold_target(&mut self, delay: Dur) {
        let ms = delay.as_millis().clamp(1_000, 60_000);
        self.ramp.set_target(Dur::from_millis(ms));
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

    /// New words from ASR. A word that overlaps one still in the hold replaces it
    /// when the new guess is not clearly worse. A word that has started airing stays.
    pub fn on_words(&mut self, words: Vec<Word>) {
        if words.is_empty() {
            return;
        }
        let playhead = self.playhead();
        let mut fresh = Vec::new();
        let mut revised = Vec::new();
        let mut replace_idx = vec![false; self.transcript.len()];
        let mut claimed = vec![false; self.transcript.len()];
        for word in words {
            let overlaps_aired = self
                .transcript
                .iter()
                .any(|old| old.span.start <= playhead && time_overlap(old.span, word.span));
            if overlaps_aired {
                continue;
            }
            let hits: Vec<usize> = self
                .transcript
                .iter()
                .enumerate()
                .filter(|(idx, old)| {
                    !claimed[*idx]
                        && !replace_idx[*idx]
                        && old.span.start > playhead
                        && time_overlap(old.span, word.span)
                })
                .map(|(idx, _)| idx)
                .collect();
            if hits.is_empty() {
                if is_repeat(&word, &self.transcript)
                    || is_repeat(&word, &fresh)
                    || repeats_latest(&word, self.transcript.last())
                    || repeats_latest(&word, fresh.last())
                {
                    continue;
                }
                fresh.push(word);
                continue;
            }
            let same = hits.len() == 1
                && norm_word(&self.transcript[hits[0]].text) == norm_word(&word.text);
            if same {
                claimed[hits[0]] = true;
                continue;
            }
            let best = hits
                .iter()
                .map(|idx| self.transcript[*idx].confidence)
                .fold(0.0_f32, f32::max);
            if word.confidence < best + 0.15 {
                continue;
            }
            for idx in hits {
                replace_idx[idx] = true;
            }
            revised.push(word);
        }

        let mut replaced = Vec::new();
        let mut kept = Vec::new();
        for (idx, word) in self.transcript.drain(..).enumerate() {
            if replace_idx[idx] {
                replaced.push(word.span);
            } else {
                kept.push(word);
            }
        }
        self.transcript = kept;
        let accepted: Vec<Word> = revised.iter().chain(fresh.iter()).cloned().collect();
        self.transcript.extend(accepted.iter().cloned());
        self.transcript
            .sort_by_key(|word| (word.span.start, word.span.end));
        if accepted.is_empty() {
            return;
        }

        let mut found = self.wordlist.scan(&accepted);
        let new_from = accepted
            .iter()
            .map(|word| word.span.start)
            .min()
            .unwrap_or(playhead);
        let ctx_start = self.transcript.len().saturating_sub(8 + accepted.len());
        found.extend(
            self.pii
                .scan(&self.transcript[ctx_start..])
                .into_iter()
                .filter(|detection| detection.span.end > new_from),
        );
        if let Some(end) = accepted.iter().map(|word| word.span.end).max() {
            self.last_word_at = Some(self.last_word_at.map(|t| t.max(end)).unwrap_or(end));
        }
        self.clear_stall();
        let still: Vec<Span> = self
            .wordlist
            .scan(&self.transcript)
            .into_iter()
            .map(|detection| detection.span)
            .collect();
        for id in self.engine.drop_wordlist_outside(&still) {
            self.events.push(UiEvent::RedactionRemoved { id });
        }
        if !fresh.is_empty() {
            self.events.push(UiEvent::Transcript { words: fresh });
        }
        if !replaced.is_empty() {
            self.events.push(UiEvent::TranscriptRevised {
                replaced,
                words: revised,
            });
        }
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
        let previous = self.live;
        self.live = self.live.max(live);
        let elapsed = self.live - previous;
        if elapsed > Dur::ZERO {
            self.ramp.advance(elapsed);
        }
        if self.safe_mode
            && let Some(id) = self.safe_mute
        {
            let end = self.live + Dur(1);
            if self.engine.extend_end(id, end) {
                self.push_upsert(id);
            }
        }
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

    /// While on, audio still in the hold stays muted as the live edge moves.
    pub fn set_safe_mode(&mut self, on: bool) {
        self.safe_mode = on;
        if !on {
            self.safe_mute = None;
            return;
        }
        let ids = self.engine.dump(self.live, self.hold(), false);
        self.safe_mute = ids.first().copied();
        for id in ids {
            self.push_upsert(id);
        }
    }

    pub fn safe_mode(&self) -> bool {
        self.safe_mode
    }

    /// Go live sets this when a speech model is present. The stall banner then says
    /// it is listening, instead of claiming recognition was never connected.
    pub fn note_recognizer(&mut self, ready: bool) {
        self.recognizer_ready = ready;
        if self.stall_latched && self.last_word_at.is_none() {
            let fallback = self.cfg.policy.stall_fallback;
            let message = self.stall_message();
            self.events.push(UiEvent::AnalysisStall {
                active: true,
                fallback,
                message,
            });
        }
    }

    /// The recognizer has accepted program audio through `at`, even if that
    /// audio produced no words. A loud game mix is not a stall while this
    /// keeps up with the live edge.
    pub fn note_recognizer_progress(&mut self, at: Ts) {
        self.recognizer_audio = Some(self.recognizer_audio.map(|t| t.max(at)).unwrap_or(at));
    }

    /// Call after `on_live_time` when a chunk of program audio has been measured.
    /// `energetic` should come from `airlock_asr::audio::has_speech_energy`.
    pub fn note_audio_energy(&mut self, energetic: bool) {
        let analyzed = if self.recognizer_ready {
            self.recognizer_audio
                .or(self.last_word_at)
                .unwrap_or(Ts::ZERO)
        } else {
            self.last_word_at.unwrap_or(Ts::ZERO)
        };
        let since = self.live - analyzed;
        let limit = if self.recognizer_ready {
            self.hold()
                .max(Dur::from_millis(self.cfg.policy.stall_after_ms))
        } else {
            Dur::from_millis(self.cfg.policy.stall_after_ms)
        };
        let behind = self.live > Ts::ZERO && since >= limit;
        // A quiet blip must not clear a real stall and immediately raise it again.
        let stalled = if self.stall_latched {
            behind
        } else {
            energetic && behind
        };
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
        if self.last_word_at.is_none() {
            if self.recognizer_ready {
                return "Listening. Words show up about a second after they are spoken.".into();
            }
            return "Speech recognition is not connected yet. Download the speech model in Settings, then go live again.".into();
        }
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

fn time_overlap(a: Span, b: Span) -> bool {
    let overlap = a.end.0.min(b.end.0) - a.start.0.max(b.start.0);
    if overlap <= 0 {
        return false;
    }
    let shorter = (a.end.0 - a.start.0).min(b.end.0 - b.start.0).max(1);
    overlap * 2 >= shorter
}

fn repeats_latest(word: &Word, previous: Option<&Word>) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    norm_word(&previous.text) == norm_word(&word.text)
        && word.span.start.0 < previous.span.end.0 + 4_000_000
        && word.span.end.0 > previous.span.start.0 - 4_000_000
}

fn is_repeat(word: &Word, existing: &[Word]) -> bool {
    let norm = norm_word(&word.text);
    existing.iter().any(|old| {
        norm_word(&old.text) == norm
            && word.span.start.0 < old.span.end.0 + 2_500_000
            && word.span.end.0 > old.span.start.0 - 2_500_000
    })
}

fn norm_word(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '\'')
        .flat_map(|c| c.to_lowercase())
        .collect()
}
