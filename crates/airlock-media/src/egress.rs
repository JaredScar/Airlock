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

/// Ingest address without the stream key. `None` when the platform has no public ingest.
pub fn publish_base(platform: Platform) -> Option<&'static str> {
    match platform {
        Platform::Twitch => Some("rtmp://live.twitch.tv/app"),
        Platform::Youtube => Some("rtmp://a.rtmp.youtube.com/live2"),
        Platform::Kick => Some("rtmps://fa723fc1b171.global-contribute.live-video.net/app"),
        Platform::Generic => None,
    }
}

/// Keychain account for that platform's stream key. Not the key itself.
pub fn key_account(platform: Platform) -> &'static str {
    match platform {
        Platform::Twitch => "airlock/twitch",
        Platform::Youtube => "airlock/youtube",
        Platform::Kick => "airlock/kick",
        Platform::Generic => "airlock/generic",
    }
}

pub fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Twitch => "Twitch",
        Platform::Youtube => "YouTube",
        Platform::Kick => "Kick",
        Platform::Generic => "the platform",
    }
}
