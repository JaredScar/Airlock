//! Speech recognition behind one trait so local Whisper and cloud STT are swappable.
//!
//! Contract: callers push 16 kHz mono f32 audio tagged with its media start time and
//! receive *finalized* words with media-time spans. Recognizers must not emit a word
//! twice; partial/unstable hypotheses stay internal.

pub mod audio;
pub mod scripted;
pub mod whisper;

use airlock_core::{Ts, Word};
use thiserror::Error;

pub use scripted::ScriptedRecognizer;

pub const ASR_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Error)]
pub enum AsrError {
    #[error("{0} is not implemented yet (see PLAN.md)")]
    NotImplemented(&'static str),
    #[error("model load failed: {0}")]
    ModelLoad(String),
    #[error("inference failed: {0}")]
    Inference(String),
}

pub trait SpeechRecognizer: Send {
    fn name(&self) -> &str;

    /// Feeds 16 kHz mono samples that begin at media time `start`.
    /// Returns words finalized as a result of this audio.
    fn push(&mut self, samples: &[f32], start: Ts) -> Result<Vec<Word>, AsrError>;

    /// Forces out any buffered words (stream ending, or deadline pressure).
    fn flush(&mut self) -> Result<Vec<Word>, AsrError>;
}
