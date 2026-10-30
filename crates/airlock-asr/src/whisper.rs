//! Local Whisper via `whisper-rs` (whisper.cpp) with CUDA. NOT IMPLEMENTED YET.
//!
//! Plan (PLAN.md Phase 2):
//! - Add `whisper-rs` as an optional dependency behind a `whisper` feature (`cuda` sub-feature).
//! - Streaming strategy: sliding window (~3s window, ~1s hop), `token_timestamps = true`,
//!   commit words only once they've appeared stable in two consecutive windows
//!   ("LocalAgreement-2"), so we never emit a word twice or retract one.
//! - Target: words finalized <= 1.5s after spoken on an RTX-class GPU with `large-v3-turbo`
//!   or `distil-large-v3`; fall back to `small.en` on CPU.
//! - Word-timestamp error is ~50–150ms; policy padding absorbs it.

use crate::{AsrError, SpeechRecognizer};
use airlock_core::{Ts, Word};
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

pub struct WhisperRecognizer {
    _config: WhisperConfig,
}

impl WhisperRecognizer {
    pub fn new(_config: WhisperConfig) -> Result<Self, AsrError> {
        Err(AsrError::NotImplemented("WhisperRecognizer"))
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
}
