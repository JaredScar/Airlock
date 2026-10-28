//! Where the cleaned stream goes.

use airlock_core::Platform;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoEncoder {
    /// Copy packets through untouched (no video redaction possible). Phase 1 MVP.
    Passthrough,
    Nvenc,
    Amf,
    Qsv,
    X264,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EgressTarget {
    pub platform: Platform,
    /// Ingest URL without the key, e.g. `rtmp://live.twitch.tv/app`.
    pub url: String,
    /// Name of the OS keychain entry holding the stream key. The key itself is never
    /// stored in config files or logged.
    pub stream_key_ref: String,
    pub encoder: VideoEncoder,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

impl EgressTarget {
    pub fn twitch_default() -> Self {
        EgressTarget {
            platform: Platform::Twitch,
            url: "rtmp://live.twitch.tv/app".into(),
            stream_key_ref: "airlock/twitch".into(),
            encoder: VideoEncoder::Passthrough,
            video_bitrate_kbps: 6_000,
            audio_bitrate_kbps: 160,
        }
    }
}
