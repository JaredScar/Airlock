//! Media plumbing. Phase 1 target topology (see docs/ARCHITECTURE.md):
//!
//!   OBS --RTMP/SRT (localhost, high bitrate)--> ingest --decode-->
//!     audio PCM ---> DelayLine ---> audio_fx (bleep/mute) --encode AAC--\
//!     video frames-> DelayLine ---> video_fx (blur/pixelate) --NVENC----+--> mux --> RTMP(S) to platform
//!   with analysis taps (ASR, OCR) reading from the *live* side of each DelayLine.
//!
//! Passthrough mode (no video redaction enabled): video packets are delayed and
//! remuxed without decode/re-encode. Cheap, and the Phase 1 MVP.

pub mod audio_fx;
pub mod audio_path;
pub mod egress;
pub mod video_fx;

#[cfg(feature = "ffmpeg")]
pub mod ingest;

pub use audio_fx::{AudioChunk, apply_audio_redactions};
pub use audio_path::{AudioHold, MediaError, Pcm, decode_studio_pcm, encode_aac};
pub use egress::{EgressTarget, VideoEncoder};
