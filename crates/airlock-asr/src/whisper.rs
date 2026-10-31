//! Local Whisper. The 3s/1s sample window and LocalAgreement commit are implemented.
//! The desktop app builds this behind the `whisper` feature. `cargo test` does not,
//! so a machine without libclang can still test the engine.
//!
//! Plan (PLAN.md Phase 2):
//! - Add `whisper-rs` as an optional dependency behind a `whisper` feature (`cuda` sub-feature).
//! - Streaming strategy: sliding window (~3s window, ~1s hop), `token_timestamps = true`,
//!   commit words only once they've appeared stable in two consecutive windows
//!   ("LocalAgreement-2"), so we never emit a word twice or retract one.
//! - Target: words finalized <= 1.5s after spoken on an RTX-class GPU with `large-v3-turbo`
//!   or `distil-large-v3`; fall back to `small.en` on CPU.
//! - Word-timestamp error is ~50–150ms; policy padding absorbs it.

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
    quiet_left: u32,
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
            let threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            let threads = threads.saturating_sub(2).clamp(2, 8) as i32;
            Ok(WhisperRecognizer {
                window: config.window(),
                agreement: WordAgreement::default(),
                quiet_left: 0,
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

    /// The samples that have not yet filled a window, including the overlap.
    pub fn finish(&mut self) -> Option<(Ts, Vec<f32>)> {
        if self.buf.is_empty() {
            return None;
        }
        let elapsed = Dur((self.hopped as i64) * 1_000_000 / self.rate as i64);
        let at = self.origin.unwrap_or(Ts::ZERO) + elapsed;
        Some((at, std::mem::take(&mut self.buf)))
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

/// Commits a word after a later window hears the same text at about the same time.
#[derive(Debug, Default)]
struct WordAgreement {
    previous: Vec<Heard>,
    committed_until: Ts,
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
impl WordAgreement {
    fn push(&mut self, heard: Vec<Heard>) -> Vec<Word> {
        let mut out = Vec::new();
        for word in &heard {
            if word.end <= self.committed_until {
                continue;
            }
            let agreed = self.previous.iter().any(|prev| {
                same_word(&prev.text, &word.text) && (prev.start.0 - word.start.0).abs() <= 400_000
            });
            if !agreed {
                continue;
            }
            let text = word.text.trim();
            if text.is_empty() {
                continue;
            }
            out.push(Word::new(
                text,
                Span::new(word.start, word.end),
                word.confidence,
            ));
            self.committed_until = word.end;
        }
        self.previous = heard;
        out
    }

    fn flush(&mut self) -> Vec<Word> {
        let pending = std::mem::take(&mut self.previous);
        pending
            .into_iter()
            .filter(|word| word.end > self.committed_until && !word.text.trim().is_empty())
            .map(|word| {
                Word::new(
                    word.text.trim(),
                    Span::new(word.start, word.end),
                    word.confidence,
                )
            })
            .collect()
    }
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn same_word(left: &str, right: &str) -> bool {
    let left = normalize_word(left);
    let right = normalize_word(right);
    !left.is_empty() && left == right
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn normalize_word(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '\'')
        .flat_map(|c| c.to_lowercase())
        .collect()
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
        let ready = self.window.push(samples, start);
        let mut out = Vec::new();
        for (at, audio) in ready {
            if !crate::audio::has_speech_energy(&audio) {
                if self.quiet_left == 0 {
                    self.agreement.previous.clear();
                    continue;
                }
                self.quiet_left -= 1;
            } else {
                self.quiet_left = 1;
            }
            out.extend(self.transcribe(&audio, at)?);
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        let mut out = Vec::new();
        if let Some((at, audio)) = self.window.finish()
            && audio.len() >= (ASR_SAMPLE_RATE / 2) as usize
        {
            out.extend(self.transcribe(&audio, at)?);
        }
        out.extend(self.agreement.flush());
        Ok(out)
    }
}

impl WhisperRecognizer {
    fn transcribe(&mut self, audio: &[f32], at: Ts) -> Result<Vec<Word>, AsrError> {
        #[cfg(not(feature = "whisper"))]
        {
            let _ = (audio, at);
            Err(AsrError::NotImplemented("WhisperRecognizer::push"))
        }
        #[cfg(feature = "whisper")]
        {
            self.run_model(audio, at)
        }
    }

    #[cfg(feature = "whisper")]
    fn run_model(&mut self, audio: &[f32], at: Ts) -> Result<Vec<Word>, AsrError> {
        let mut samples = audio.to_vec();
        if samples.len() < ASR_SAMPLE_RATE as usize {
            samples.resize(ASR_SAMPLE_RATE as usize, 0.0);
        }
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_single_segment(false);
        params.set_suppress_blank(true);
        params.set_token_timestamps(true);
        if let Some(language) = self.language.as_deref() {
            params.set_language(Some(language));
        }
        self.state
            .full(params, &samples)
            .map_err(|err| AsrError::Inference(err.to_string()))?;
        let heard = words_from_state(&self.state, at, audio.len());
        Ok(self.agreement.push(heard))
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
        let mut current: Option<PartialWord> = None;
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
            if raw.starts_with(' ') || current.is_none() {
                if let Some(word) = current.take() {
                    take_word(word, origin, limit, &mut heard);
                }
                let text = raw.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                current = Some(PartialWord {
                    text,
                    t0,
                    t1,
                    p_sum: token.token_probability(),
                    n: 1,
                });
            } else if let Some(word) = current.as_mut() {
                word.text.push_str(raw.trim());
                word.t1 = t1;
                word.p_sum += token.token_probability();
                word.n += 1;
            }
        }
        if let Some(word) = current.take() {
            take_word(word, origin, limit, &mut heard);
        }
    }
    heard
}

#[cfg(feature = "whisper")]
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
    fn a_word_is_kept_only_after_the_next_window_agrees() {
        let mut agree = WordAgreement::default();
        let heard = |text: &str, start_ms: i64| Heard {
            text: text.into(),
            start: Ts::from_millis(start_ms),
            end: Ts::from_millis(start_ms + 300),
            confidence: 0.8,
        };
        assert!(
            agree
                .push(vec![heard("Hello,", 100), heard("there", 500)])
                .is_empty()
        );
        let committed = agree.push(vec![
            heard("hello", 140),
            heard("there", 520),
            heard("friend", 2_000),
        ]);
        assert_eq!(
            committed
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["hello", "there"]
        );
        assert!(
            agree
                .push(vec![heard("hello", 140), heard("there", 520)])
                .is_empty()
        );
    }
}
