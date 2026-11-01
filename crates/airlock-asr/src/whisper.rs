//! Local Whisper. The desktop app builds this behind the `whisper` feature.
//!
//! Each stretch of audio is committed once, from the stable middle of a window.
//! Whisper stops after the first phrase, so the rest of that window is decoded
//! again. Time is sealed only through the last word that was written, and the
//! tail of an open window waits for the next hop. A pause commits the phrase
//! immediately. The same word is not written twice. A very unsure guess is
//! dropped. Non-speech markers such as `[BLANK_AUDIO]` are dropped.

use crate::{ASR_SAMPLE_RATE, AsrError, SpeechRecognizer};
use airlock_core::{Dur, Span, Ts, Word};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct WhisperConfig {
    pub model_path: PathBuf,
    pub language: Option<String>,
    pub use_gpu: bool,
    pub window_ms: u32,
    pub hop_ms: u32,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        WhisperConfig {
            model_path: PathBuf::from("models/ggml-large-v3-turbo.bin"),
            language: Some("en".into()),
            use_gpu: true,
            window_ms: 3_000,
            hop_ms: 1_000,
        }
    }
}

impl WhisperConfig {
    pub fn window(&self) -> SampleWindow {
        SampleWindow::new(ASR_SAMPLE_RATE, self.window_ms, self.hop_ms)
    }
}

pub struct WhisperRecognizer {
    window: SampleWindow,
    agreement: WordAgreement,
    /// The open phrase was already transcribed when speech paused.
    phrase_sent: bool,
    #[cfg(feature = "whisper")]
    state: whisper_rs::WhisperState,
    #[cfg(feature = "whisper")]
    language: Option<String>,
    #[cfg(feature = "whisper")]
    threads: i32,
}

impl WhisperRecognizer {
    pub fn new(config: WhisperConfig) -> Result<Self, AsrError> {
        #[cfg(not(feature = "whisper"))]
        {
            let _ = config;
            Err(AsrError::NotImplemented(
                "WhisperRecognizer (build the desktop app with the whisper feature)",
            ))
        }
        #[cfg(feature = "whisper")]
        {
            // Token dumps go to stderr by default and fill the dev console.
            // Without a log backend this hook keeps them off that stream.
            whisper_rs::install_logging_hooks();
            let path = config
                .model_path
                .to_str()
                .ok_or_else(|| AsrError::ModelLoad("model path is not unicode".into()))?;
            let mut params = whisper_rs::WhisperContextParameters::default();
            params.use_gpu(config.use_gpu);
            let ctx = whisper_rs::WhisperContext::new_with_params(path, params)
                .map_err(|err| AsrError::ModelLoad(err.to_string()))?;
            let state = ctx
                .create_state()
                .map_err(|err| AsrError::ModelLoad(err.to_string()))?;
            let threads = if config.use_gpu {
                4
            } else {
                let threads = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                threads.saturating_sub(2).clamp(4, 12) as i32
            };
            Ok(WhisperRecognizer {
                window: config.window(),
                agreement: WordAgreement::default(),
                phrase_sent: false,
                state,
                language: config.language,
                threads,
            })
        }
    }
}

/// Buffers 16 kHz audio and emits a window, then keeps the overlap for the next hop.
#[derive(Debug, Clone)]
pub struct SampleWindow {
    buf: Vec<f32>,
    origin: Option<Ts>,
    hopped: usize,
    rate: u32,
    window: usize,
    hop: usize,
}

impl SampleWindow {
    pub fn new(rate: u32, window_ms: u32, hop_ms: u32) -> Self {
        let rate = rate.max(1);
        let window = ((rate as u64 * window_ms as u64) / 1_000).max(1) as usize;
        let mut hop = ((rate as u64 * hop_ms as u64) / 1_000) as usize;
        if hop == 0 || hop > window {
            hop = window;
        }
        SampleWindow {
            buf: Vec::new(),
            origin: None,
            hopped: 0,
            rate,
            window,
            hop,
        }
    }

    /// Completed windows. Each start time is media time at the first sample of that window.
    pub fn push(&mut self, samples: &[f32], start: Ts) -> Vec<(Ts, Vec<f32>)> {
        if samples.is_empty() {
            return Vec::new();
        }
        if self.origin.is_none() {
            self.origin = Some(start);
        }
        self.buf.extend_from_slice(samples);
        let mut ready = Vec::new();
        while self.buf.len() >= self.window {
            let elapsed = Dur((self.hopped as i64) * 1_000_000 / self.rate as i64);
            let at = self.origin.unwrap_or(start) + elapsed;
            ready.push((at, self.buf[..self.window].to_vec()));
            self.buf.drain(..self.hop);
            self.hopped += self.hop;
        }
        ready
    }

    /// The next chunk jumped forward. The samples in between were already too
    /// late to bleep, so the window starts again at `start`.
    pub fn is_gap(&self, start: Ts) -> bool {
        let Some(origin) = self.origin else {
            return false;
        };
        let consumed = self.hopped + self.buf.len();
        let expected = origin + Dur((consumed as i64) * 1_000_000 / self.rate as i64);
        start.0 > expected.0 + 200_000
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.origin = None;
        self.hopped = 0;
    }

    /// The samples that have not yet filled a window, including the overlap.
    pub fn finish(&mut self) -> Option<(Ts, Vec<f32>)> {
        if self.buf.is_empty() {
            return None;
        }
        let elapsed = Dur((self.hopped as i64) * 1_000_000 / self.rate as i64);
        let at = self.origin.unwrap_or(Ts::ZERO) + elapsed;
        Some((at, std::mem::take(&mut self.buf)))
    }

    /// Audio still waiting for a full window.
    fn pending(&self) -> Option<(Ts, &[f32])> {
        if self.buf.is_empty() {
            return None;
        }
        let elapsed = Dur((self.hopped as i64) * 1_000_000 / self.rate as i64);
        let at = self.origin.unwrap_or(Ts::ZERO) + elapsed;
        Some((at, &self.buf))
    }
}

/// Commits a word only after the same text appears at the start of two windows.
#[derive(Debug, Default)]
pub struct LocalAgreement {
    last: Vec<String>,
}

/// One word heard inside a window, before it is committed.
#[derive(Debug, Clone)]
struct Heard {
    text: String,
    start: Ts,
    end: Ts,
    confidence: f32,
}

/// Commits words before the tail of an open window. Time is sealed only through
/// the last word that was written, so audio Whisper skipped stays open.
#[derive(Debug, Default)]
struct WordAgreement {
    previous: Vec<Heard>,
    committed_until: Ts,
    said: Vec<Said>,
}

#[derive(Debug, Clone)]
struct Said {
    norm: String,
    start: Ts,
    end: Ts,
    confidence: f32,
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
impl WordAgreement {
    /// `window_end` is the media time of the last sample in this decode.
    /// `complete` is a pause or a flush, so the tail is committed too.
    fn push(&mut self, heard: Vec<Heard>, window_end: Ts, complete: bool) -> Vec<Word> {
        let heard = collapse_consecutive(heard);
        let stable_until = if complete {
            window_end
        } else {
            Ts(window_end.0.saturating_sub(TAIL_US))
        };
        let mut out = Vec::new();
        for word in &heard {
            if word.end <= self.committed_until {
                continue;
            }
            if word.start < self.committed_until {
                let mid = word.start.0 + (word.end.0 - word.start.0) / 2;
                if mid < self.committed_until.0 {
                    continue;
                }
            }
            if word.start >= stable_until {
                continue;
            }
            self.commit_heard(word, &mut out);
        }
        out
    }

    fn commit_heard(&mut self, word: &Heard, out: &mut Vec<Word>) {
        let text = word.text.trim();
        if text.is_empty() || is_non_speech(text) || word.confidence < MIN_WORD_CONFIDENCE {
            return;
        }
        let norm = normalize_word(text);
        self.said
            .retain(|prev| word.end.0.saturating_sub(prev.end.0) < 30_000_000);
        let hits: Vec<usize> = self
            .said
            .iter()
            .enumerate()
            .filter(|(_, prev)| {
                times_overlap(prev.start, prev.end, word.start, word.end)
                    || (prev.norm == norm
                        && within_repeat(prev.start, prev.end, word.start, word.end))
            })
            .map(|(idx, _)| idx)
            .collect();
        if !hits.is_empty() {
            let same = hits.iter().all(|idx| self.said[*idx].norm == norm);
            let best = hits
                .iter()
                .map(|idx| self.said[*idx].confidence)
                .fold(0.0_f32, f32::max);
            if same || word.confidence + 0.1 < best {
                return;
            }
            for idx in hits.into_iter().rev() {
                self.said.remove(idx);
            }
        }
        self.committed_until = self.committed_until.max(word.end);
        self.said.push(Said {
            norm,
            start: word.start,
            end: word.end,
            confidence: word.confidence,
        });
        out.push(Word::new(
            text,
            Span::new(word.start, word.end),
            word.confidence,
        ));
    }

    fn flush(&mut self) -> Vec<Word> {
        let pending = std::mem::take(&mut self.previous);
        let mut out = Vec::new();
        for word in pending {
            if word.end <= self.committed_until {
                continue;
            }
            self.commit_heard(&word, &mut out);
        }
        out
    }
}

/// Guesses below this are usually noise. A later window can still say the word.
const MIN_WORD_CONFIDENCE: f32 = 0.4;

/// The same word heard again inside this gap is one utterance.
const REPEAT_GAP_US: i64 = 2_500_000;

/// Words in the last part of an open window are held for the next hop.
/// Short enough that a word still shows well before the delay lets it air.
const TAIL_US: i64 = 400_000;

/// A first phrase that leaves at least this much audio gets a second decode.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
const REMAINDER_US: i64 = 800_000;

fn collapse_consecutive(heard: Vec<Heard>) -> Vec<Heard> {
    let mut out: Vec<Heard> = Vec::new();
    for word in heard {
        if let Some(prev) = out.last() {
            let same = normalize_word(&prev.text) == normalize_word(&word.text);
            if same && within_repeat(prev.start, prev.end, word.start, word.end) {
                continue;
            }
        }
        out.push(word);
    }
    out
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn within_repeat(a0: Ts, a1: Ts, b0: Ts, b1: Ts) -> bool {
    b0.0 < a1.0 + REPEAT_GAP_US && b1.0 > a0.0 - REPEAT_GAP_US
}

fn times_overlap(a0: Ts, a1: Ts, b0: Ts, b1: Ts) -> bool {
    let overlap = a1.0.min(b1.0) - a0.0.max(b0.0);
    if overlap <= 0 {
        return false;
    }
    let shorter = (a1.0 - a0.0).min(b1.0 - b0.0).max(1);
    overlap * 2 >= shorter
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn normalize_word(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '\'')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn is_non_speech(text: &str) -> bool {
    let norm = normalize_word(text);
    if norm.is_empty() || norm == "blankaudio" {
        return true;
    }
    let marked = text.contains('[') || text.contains('(') || text.contains('♪');
    marked
        && matches!(
            norm.as_str(),
            "music" | "noise" | "silence" | "applause" | "laughter"
        )
}

/// A short phrase followed by a quiet tail. Transcribe it now instead of waiting
/// for the window to fill.
fn phrase_is_ready(audio: &[f32], rate: u32) -> bool {
    let tail = (rate as usize / 5).max(1);
    let min = rate as usize * 3 / 10;
    if audio.len() < min.max(tail + 1) {
        return false;
    }
    let split = audio.len() - tail;
    crate::audio::is_near_silence(&audio[split..])
        && !crate::audio::is_near_silence(&audio[..split])
}

/// When every word in a phrase shares one start time, split that interval by
/// length. Otherwise the bleep for one bad word covers the whole phrase.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn spread_shared_times(times: &mut [(i64, i64)], chars: &[usize], seg_t0: i64, seg_t1: i64) {
    if times.len() < 2 || times.len() != chars.len() {
        return;
    }
    if !times.iter().all(|slot| slot.0 == times[0].0) {
        return;
    }
    let missing = times[0].0 == 0 && times[0].1 == 0;
    let start = if missing { seg_t0 } else { times[0].0 };
    let end = if missing {
        seg_t1
    } else {
        times
            .iter()
            .map(|slot| slot.1)
            .max()
            .unwrap_or(seg_t1)
            .max(start + times.len() as i64)
    };
    if end <= start {
        return;
    }
    let total = chars.iter().sum::<usize>().max(1) as i64;
    let dur = end - start;
    let last = times.len() - 1;
    let mut cursor = start;
    for (i, (slot, n)) in times.iter_mut().zip(chars).enumerate() {
        let share = if i == last {
            end - cursor
        } else {
            (dur * (*n as i64) / total).max(1)
        };
        slot.0 = cursor;
        slot.1 = (cursor + share).min(end);
        cursor = slot.1;
    }
}

impl LocalAgreement {
    pub fn commit(&mut self, hypothesis: &[impl AsRef<str>]) -> Vec<String> {
        let current: Vec<String> = hypothesis.iter().map(|s| s.as_ref().to_string()).collect();
        let mut agreed = Vec::new();
        for (prev, next) in self.last.iter().zip(current.iter()) {
            if prev != next {
                break;
            }
            agreed.push(next.clone());
        }
        self.last = current;
        agreed
    }
}

impl SpeechRecognizer for WhisperRecognizer {
    fn name(&self) -> &str {
        "whisper"
    }

    fn push(&mut self, samples: &[f32], start: Ts) -> Result<Vec<Word>, AsrError> {
        let mut out = Vec::new();
        if !crate::audio::is_near_silence(samples) {
            self.phrase_sent = false;
        }
        if self.window.is_gap(start) {
            out.extend(self.agreement.flush());
            self.window.reset();
            self.phrase_sent = false;
        }
        let ready = self.window.push(samples, start);
        for (at, audio) in ready {
            if crate::audio::is_near_silence(&audio) {
                out.extend(self.agreement.flush());
                continue;
            }
            self.phrase_sent = true;
            out.extend(self.transcribe(&audio, at, false)?);
        }
        if !self.phrase_sent {
            let rate = self.window.rate;
            let early = self
                .window
                .pending()
                .and_then(|(at, audio)| phrase_is_ready(audio, rate).then(|| (at, audio.to_vec())));
            if let Some((at, audio)) = early {
                self.phrase_sent = true;
                out.extend(self.transcribe(&audio, at, true)?);
            }
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        let mut out = Vec::new();
        if let Some((at, audio)) = self.window.finish()
            && audio.len() >= (ASR_SAMPLE_RATE / 2) as usize
        {
            out.extend(self.transcribe(&audio, at, true)?);
        }
        out.extend(self.agreement.flush());
        Ok(out)
    }
}

impl WhisperRecognizer {
    fn transcribe(&mut self, audio: &[f32], at: Ts, complete: bool) -> Result<Vec<Word>, AsrError> {
        #[cfg(not(feature = "whisper"))]
        {
            let _ = (audio, at, complete);
            Err(AsrError::NotImplemented("WhisperRecognizer::push"))
        }
        #[cfg(feature = "whisper")]
        {
            self.run_model(audio, at, complete)
        }
    }

    #[cfg(feature = "whisper")]
    fn run_model(&mut self, audio: &[f32], at: Ts, complete: bool) -> Result<Vec<Word>, AsrError> {
        let mut heard = self.decode_slice(audio, at)?;
        if let Some(last) = heard.last() {
            let covered = (last.end.0 - at.0).max(0);
            let total = (audio.len() as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE);
            if total - covered >= REMAINDER_US {
                let skip = (covered * i64::from(ASR_SAMPLE_RATE) / 1_000_000) as usize;
                if skip > 0 && skip < audio.len() {
                    let rest = &audio[skip..];
                    if rest.len() >= (ASR_SAMPLE_RATE / 2) as usize {
                        let origin = at + Dur(covered);
                        if let Ok(more) = self.decode_slice(rest, origin) {
                            heard.extend(more);
                            heard.sort_by_key(|word| (word.start, word.end));
                        }
                    }
                }
            }
        }
        let window_end = at + Dur((audio.len() as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE));
        Ok(self.agreement.push(heard, window_end, complete))
    }

    /// One Whisper pass. The decoder stops after its first phrase, so the caller
    /// decodes whatever audio that phrase did not cover.
    #[cfg(feature = "whisper")]
    fn decode_slice(&mut self, audio: &[f32], at: Ts) -> Result<Vec<Heard>, AsrError> {
        let mut samples = audio.to_vec();
        if samples.len() < ASR_SAMPLE_RATE as usize {
            samples.resize(ASR_SAMPLE_RATE as usize, 0.0);
        }
        let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::BeamSearch {
            beam_size: 2,
            patience: -1.0,
        });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_single_segment(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_token_timestamps(true);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        params.set_max_tokens(0);
        if let Some(language) = self.language.as_deref() {
            params.set_language(Some(language));
        }
        self.state
            .full(params, &samples)
            .map_err(|err| AsrError::Inference(err.to_string()))?;
        Ok(words_from_state(&self.state, at, audio.len()))
    }
}

#[cfg(feature = "whisper")]
fn words_from_state(
    state: &whisper_rs::WhisperState,
    origin: Ts,
    real_samples: usize,
) -> Vec<Heard> {
    let limit = origin + Dur((real_samples as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE));
    let mut heard = Vec::new();
    for segment in state.as_iter() {
        if segment.no_speech_probability() > 0.5 {
            continue;
        }
        let mut partials = Vec::new();
        for idx in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(idx) else {
                continue;
            };
            let Ok(raw) = token.to_str() else {
                continue;
            };
            if raw.starts_with('<') || raw.starts_with('[') {
                continue;
            }
            let data = token.token_data();
            let (t0, t1) = if data.t0 == 0 && data.t1 == 0 {
                (segment.start_timestamp(), segment.end_timestamp())
            } else {
                (data.t0, data.t1)
            };
            if raw.starts_with(' ') || partials.is_empty() {
                let text = raw.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                partials.push(PartialWord {
                    text,
                    t0,
                    t1,
                    p_sum: token.token_probability(),
                    n: 1,
                });
            } else if let Some(word) = partials.last_mut() {
                word.text.push_str(raw.trim());
                word.t1 = t1;
                word.p_sum += token.token_probability();
                word.n += 1;
            }
        }
        if partials.is_empty() {
            continue;
        }
        let mut confidence = 0.0;
        for word in &partials {
            confidence += word.p_sum / word.n.max(1) as f32;
        }
        if confidence / (partials.len() as f32) < 0.35 {
            continue;
        }
        let mut times: Vec<(i64, i64)> = partials.iter().map(|w| (w.t0, w.t1)).collect();
        let chars: Vec<usize> = partials.iter().map(|w| w.text.chars().count()).collect();
        spread_shared_times(
            &mut times,
            &chars,
            segment.start_timestamp(),
            segment.end_timestamp(),
        );
        for (word, (t0, t1)) in partials.iter_mut().zip(times) {
            word.t0 = t0;
            word.t1 = t1;
            take_word(word.clone(), origin, limit, &mut heard);
        }
    }
    heard
}

#[cfg(feature = "whisper")]
#[derive(Clone)]
struct PartialWord {
    text: String,
    t0: i64,
    t1: i64,
    p_sum: f32,
    n: u32,
}

#[cfg(feature = "whisper")]
fn take_word(word: PartialWord, origin: Ts, limit: Ts, out: &mut Vec<Heard>) {
    let text = word.text.trim();
    if normalize_word(text).is_empty() {
        return;
    }
    let start = origin + Dur(word.t0.saturating_mul(10_000));
    if start >= limit {
        return;
    }
    let mut end = origin + Dur(word.t1.max(word.t0 + 1).saturating_mul(10_000));
    if end > limit {
        end = limit;
    }
    if end <= start {
        end = start + Dur(10_000);
    }
    out.push(Heard {
        text: text.to_string(),
        start,
        end,
        confidence: word.p_sum / word.n.max(1) as f32,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_committed_only_when_the_next_window_agrees() {
        let mut agree = LocalAgreement::default();
        assert!(agree.commit(&["hello", "chat"]).is_empty());
        assert_eq!(
            agree.commit(&["hello", "chat", "friends"]),
            vec!["hello", "chat"]
        );
        assert!(agree.commit(&["goodbye"]).is_empty());
    }

    #[test]
    fn a_three_second_window_hops_one_second_and_keeps_media_time() {
        let mut window = SampleWindow::new(4, 1_000, 500);
        let first = window.push(&[1.0, 2.0, 3.0, 4.0], Ts::ZERO);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, Ts::ZERO);
        assert_eq!(first[0].1, vec![1.0, 2.0, 3.0, 4.0]);
        let second = window.push(&[5.0, 6.0], Ts::from_millis(1_000));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0, Ts(500_000));
        assert_eq!(second[0].1, vec![3.0, 4.0, 5.0, 6.0]);
        let defaults = WhisperConfig::default().window();
        assert_eq!(defaults.window, 48_000);
        assert_eq!(defaults.hop, 16_000);
    }

    #[test]
    fn a_word_shows_on_the_first_window_and_an_overlap_is_not_repeated() {
        let mut agree = WordAgreement::default();
        let heard = |text: &str, start_ms: i64| Heard {
            text: text.into(),
            start: Ts::from_millis(start_ms),
            end: Ts::from_millis(start_ms + 300),
            confidence: 0.8,
        };
        let first = agree.push(
            vec![heard("Hello,", 100), heard("there", 500)],
            Ts::from_millis(3_000),
            false,
        );
        assert_eq!(
            first
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Hello,", "there"]
        );
        let overlap = agree.push(
            vec![
                heard("hello", 140),
                heard("there", 520),
                heard("friend", 2_800),
            ],
            Ts::from_millis(4_000),
            false,
        );
        assert_eq!(
            overlap
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["friend"]
        );
        let marker = agree.push(
            vec![heard("[BLANK_AUDIO]", 4_000)],
            Ts::from_millis(5_000),
            true,
        );
        assert!(marker.iter().all(|word| !word.text.contains("BLANK")));
    }

    #[test]
    fn a_pause_is_ready_before_the_window_fills() {
        let rate = 16_000u32;
        let mut audio = vec![0.2; 8_000];
        audio.extend(std::iter::repeat_n(0.0, 3_200));
        assert!(phrase_is_ready(&audio, rate));
        assert!(!phrase_is_ready(&vec![0.2; 8_000], rate));
        assert!(!phrase_is_ready(&vec![0.0; 16_000], rate));
    }

    #[test]
    fn a_shared_timestamp_is_split_across_the_words() {
        let mut times = [(0, 100), (0, 100), (0, 100)];
        spread_shared_times(&mut times, &[5, 4, 11], 0, 100);
        assert!(times[0].0 < times[1].0);
        assert!(times[1].0 < times[2].0);
        assert_eq!(times[0].0, 0);
        assert_eq!(times[2].1, 100);
        assert!(times[0].1 <= times[1].0);
        let mut distinct = [(10, 20), (30, 40)];
        spread_shared_times(&mut distinct, &[4, 4], 0, 100);
        assert_eq!(distinct, [(10, 20), (30, 40)]);
    }

    #[test]
    fn a_window_keeps_its_stable_words_and_drops_repeats_and_the_tail() {
        let mut agree = WordAgreement::default();
        let heard = |text: &str, start_ms: i64, confidence: f32| Heard {
            text: text.into(),
            start: Ts::from_millis(start_ms),
            end: Ts::from_millis(start_ms + 300),
            confidence,
        };
        let first = agree.push(
            vec![heard("free", 100, 0.6), heard("ride", 500, 0.6)],
            Ts::from_millis(3_000),
            false,
        );
        assert_eq!(
            first
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["free", "ride"]
        );
        let tail = heard("later", 2_600, 0.9);
        let held = agree.push(vec![tail], Ts::from_millis(3_000), false);
        assert!(held.is_empty());
        let next = agree.push(
            vec![
                heard("I", 100, 0.9),
                heard("the", 2_800, 0.8),
                heard("the", 3_000, 0.8),
                heard("the", 3_200, 0.8),
            ],
            Ts::from_millis(4_000),
            false,
        );
        assert_eq!(
            next.iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["the"]
        );
        let weak = agree.push(
            vec![heard("baloney", 3_200, 0.1)],
            Ts::from_millis(5_000),
            true,
        );
        assert!(weak.is_empty());
    }

    #[test]
    fn a_word_in_the_skipped_stretch_is_kept_on_the_next_window() {
        let mut agree = WordAgreement::default();
        let heard = |text: &str, start_ms: i64| Heard {
            text: text.into(),
            start: Ts::from_millis(start_ms),
            end: Ts::from_millis(start_ms + 300),
            confidence: 0.8,
        };
        let first = agree.push(vec![heard("free", 100)], Ts::from_millis(2_000), false);
        assert_eq!(first.len(), 1);
        let next = agree.push(vec![heard("world", 1_200)], Ts::from_millis(2_250), false);
        assert_eq!(
            next.iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["world"]
        );
    }
}
