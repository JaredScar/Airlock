//! Local Whisper. The 3s/1s sample window and LocalAgreement commit are implemented.
//! Linking whisper.cpp needs libclang (`clang.dll`), which is not on this machine, so
//! `WhisperRecognizer::new` still refuses to load a model.
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
use airlock_core::{Dur, Ts, Word};
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
    _config: WhisperConfig,
}

impl WhisperRecognizer {
    pub fn new(_config: WhisperConfig) -> Result<Self, AsrError> {
        Err(AsrError::NotImplemented(
            "WhisperRecognizer (needs libclang to build whisper.cpp)",
        ))
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
    fn push(&mut self, _samples: &[f32], _start: Ts) -> Result<Vec<Word>, AsrError> {
        Err(AsrError::NotImplemented("WhisperRecognizer::push"))
    }
    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        Err(AsrError::NotImplemented("WhisperRecognizer::flush"))
    }
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
}
