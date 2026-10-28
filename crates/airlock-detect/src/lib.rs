//! Deterministic, sub-millisecond detectors. These run on every ASR word batch and
//! every OCR sample, before (and independent of) the slower LLM judge, so the cheap
//! stuff is always caught even if the LLM misses its deadline.

pub mod normalize;
pub mod pii;
pub mod secrets;
pub mod transcript;
pub mod wordlist;

pub use pii::SpokenPiiDetector;
pub use secrets::ScreenTextScanner;
pub use transcript::TranscriptIndex;
pub use wordlist::WordlistDetector;
