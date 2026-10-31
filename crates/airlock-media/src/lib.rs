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
pub mod eval_media;
pub mod link;
pub mod pacer;
pub mod transcode;
pub mod video_fx;
pub mod video_hold;

pub mod ingest;

pub use audio_fx::{AudioChunk, apply_audio_redactions};
pub use audio_path::{AudioHold, MediaError, Pcm, decode_studio_pcm, encode_aac, time_stretch};
pub use egress::{EgressTarget, VideoEncoder};
pub use eval_media::write_eval_clips;
pub use ingest::{INGEST_KEY, INGEST_PORT, PtsClock, accept_publisher, ingest_url, serve};
pub use link::{
    publish_file, publish_with_retry, receive_srt_wav, reconnect_backoff, secure_publish_url,
};
pub use pacer::{FlvMux, Pacer, push_flv};
pub use transcode::encode_redacted;
pub use video_hold::{VideoHold, VideoPacket};
