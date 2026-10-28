//! In-process ingest via `ffmpeg-next`. NOT IMPLEMENTED: PLAN.md Phase 1.
//!
//! Responsibilities:
//! - Listen for OBS on `rtmp://127.0.0.1:1935/live` (and later SRT for lower latency).
//! - Demux; decode audio to f32 48 kHz; in passthrough mode keep video as packets,
//!   otherwise decode to frames (hardware decode where available).
//! - Stamp everything with `Ts` from PTS (rebased so the first packet = 0).
//! - Push into the audio/video `DelayLine`s and fan out copies to the analysis taps
//!   (ASR gets downmixed 16 kHz mono via airlock-asr::audio; OCR gets sampled frames).
//!
//! Enable with `--features ffmpeg` once ffmpeg-next is added as an optional dependency.
