//! What the streamer configures before a session.
//!
//! Several destinations can receive the same cleaned stream. A second destination
//! needs Pro. Airlock's platform packs stay versioned files. The streamer's own
//! rules are stored per platform, and every destination being published is included
//! in the judge prompt. Private terms stay on the machine.

use crate::{DEFAULT_ALLOWLIST, DEFAULT_PROFANITY, Feature, SessionConfig, allows};
use airlock_core::{Platform, Policy};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StudioSettings {
    /// The platform whose stream key and rules are on screen.
    pub platform: Platform,
    /// Destinations that receive the cleaned stream. Empty means `platform` only.
    #[serde(default)]
    pub destinations: Vec<Platform>,
    /// The streamer's own judge rules, per destination. Not the shipped pack.
    pub rules_twitch: Vec<String>,
    pub rules_youtube: Vec<String>,
    pub rules_kick: Vec<String>,
    /// Matched locally. Never copied into a judge prompt.
    pub private_terms: Vec<String>,
    pub policy: Policy,
    /// Extra profanity entries, one per line, appended to the shipped list.
    pub extra_profanity: String,
    /// Extra allowlist tokens, one per line.
    pub extra_allow: String,
    /// The streamer opted in to sending transcript text to a cloud judge.
    #[serde(default)]
    pub cloud_judge_consent: bool,
    /// The streamer opted in to sending audio to Deepgram for speech recognition.
    #[serde(default)]
    pub cloud_stt_consent: bool,
    #[serde(default = "shortcut_veto")]
    pub shortcut_veto: String,
    #[serde(default = "shortcut_approve")]
    pub shortcut_approve: String,
    #[serde(default = "shortcut_dump")]
    pub shortcut_dump: String,
    #[serde(default = "shortcut_safe")]
    pub shortcut_safe: String,
    /// Empty until the streamer pastes a Pro key. Checked offline.
    #[serde(default)]
    pub license_key: String,
    /// Crash and latency reports. Off, and there is no destination unless one is set.
    #[serde(default)]
    pub telemetry_opt_in: bool,
    /// Open the local listener when OBS Studio's process appears.
    #[serde(default = "start_with_obs_default")]
    pub start_with_obs: bool,
    /// OBS source names that are screenshotted even when they are not on the program scene.
    #[serde(default)]
    pub scan_always: Vec<String>,
    /// OBS source names that are never screenshotted.
    #[serde(default)]
    pub scan_never: Vec<String>,
    /// A moderator on another computer may approve or veto during the delay.
    #[serde(default)]
    pub moderator_enabled: bool,
    /// TCP port for that connection. The default is 1940.
    #[serde(default = "moderator_port_default")]
    pub moderator_port: u16,
}

fn moderator_port_default() -> u16 {
    1940
}

fn shortcut_veto() -> String {
    "F8".into()
}
fn shortcut_approve() -> String {
    "F9".into()
}
fn shortcut_dump() -> String {
    "F10".into()
}
fn shortcut_safe() -> String {
    "F11".into()
}
fn start_with_obs_default() -> bool {
    true
}

impl Default for StudioSettings {
    fn default() -> Self {
        StudioSettings {
            platform: Platform::Twitch,
            destinations: Vec::new(),
            rules_twitch: vec![],
            rules_youtube: vec![],
            rules_kick: vec![],
            // Keeps the scripted demo's school name flagged until the streamer replaces it.
            private_terms: vec!["Springfield High".into()],
            policy: Policy::default(),
            extra_profanity: String::new(),
            extra_allow: String::new(),
            cloud_judge_consent: false,
            cloud_stt_consent: false,
            shortcut_veto: shortcut_veto(),
            shortcut_approve: shortcut_approve(),
            shortcut_dump: shortcut_dump(),
            shortcut_safe: shortcut_safe(),
            license_key: String::new(),
            telemetry_opt_in: false,
            start_with_obs: true,
            scan_always: Vec::new(),
            scan_never: Vec::new(),
            moderator_enabled: false,
            moderator_port: moderator_port_default(),
        }
    }
}

impl StudioSettings {
    pub fn active_rules(&self) -> &[String] {
        self.rules_of(self.platform)
    }

    /// Platforms the streamer asked to publish to, before the license check.
    pub fn saved_destinations(&self) -> Vec<Platform> {
        let source = if self.destinations.is_empty() {
            vec![self.platform]
        } else {
            self.destinations.clone()
        };
        let mut out = Vec::new();
        for platform in source {
            if is_destination(platform) && !out.contains(&platform) {
                out.push(platform);
            }
        }
        out
    }

    /// Platforms this session publishes. Without Pro, only one remains.
    pub fn publish_platforms(&self) -> Vec<Platform> {
        let saved = self.saved_destinations();
        if saved.len() > 1 && !allows(&self.license_key, Feature::Multistream) {
            if saved.contains(&self.platform) {
                return vec![self.platform];
            }
            return vec![saved[0]];
        }
        saved
    }

    /// Streamer rules for every platform in `platforms`, in that order, without duplicates.
    pub fn rules_for(&self, platforms: &[Platform]) -> Vec<String> {
        let mut out = Vec::new();
        for platform in platforms {
            for rule in self.rules_of(*platform) {
                let rule = rule.trim();
                if rule.is_empty() || out.iter().any(|kept: &String| kept == rule) {
                    continue;
                }
                out.push(rule.to_string());
            }
        }
        out
    }

    fn rules_of(&self, platform: Platform) -> &[String] {
        match platform {
            Platform::Twitch => &self.rules_twitch,
            Platform::Youtube => &self.rules_youtube,
            Platform::Kick => &self.rules_kick,
            Platform::Generic => &[],
        }
    }

    /// Builds the session the next broadcast uses. Rejects a policy the engine would refuse.
    pub fn session_config(&self) -> Result<SessionConfig, String> {
        self.policy.validate().map_err(|e| e.to_string())?;
        let platforms = self.publish_platforms();
        let platform = platforms.first().copied().unwrap_or(self.platform);
        let also_platforms = platforms.into_iter().skip(1).collect();
        Ok(SessionConfig {
            policy: self.policy.clone(),
            platform,
            also_platforms,
            private_terms: tidy(&self.private_terms),
            streamer_rules: self.rules_for(&self.publish_platforms()),
            profanity_list: append_lines(DEFAULT_PROFANITY, &self.extra_profanity),
            allowlist: append_lines(DEFAULT_ALLOWLIST, &self.extra_allow),
            ..SessionConfig::default()
        })
    }
}

fn is_destination(platform: Platform) -> bool {
    matches!(
        platform,
        Platform::Twitch | Platform::Youtube | Platform::Kick
    )
}

fn tidy(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn append_lines(base: &str, extra: &str) -> String {
    let mut out = base.trim_end().to_string();
    let mut added = false;
    for line in extra.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !added {
            out.push('\n');
            added = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
