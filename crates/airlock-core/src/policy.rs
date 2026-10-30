//! What to do about each category. Loaded from TOML (see config/policy.default.toml).

use crate::error::{Error, Result};
use crate::model::Category;
use crate::time::Dur;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// How a category is handled once detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Mode {
    /// Redact automatically. The streamer can veto during the delay window.
    Apply,
    /// Flag it. Only redacted if the streamer approves before it airs.
    Suggest,
    /// Never redact live; record in the incident log for VOD cleanup.
    LogOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AudioAction {
    Bleep,
    Mute,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum VideoAction {
    Blur,
    Pixelate,
    Blackout,
    None,
}

/// What to do when speech recognition goes quiet while the mix still has audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StallFallback {
    /// Red banner only. Audio still airs.
    Warn,
    /// Mute audio from the last recognized word through now.
    Mute,
    /// Hold the stream longer. The extra delay stays for the rest of the session.
    ExtendDelay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CategoryPolicy {
    pub mode: Mode,
    pub audio: AudioAction,
    pub video: VideoAction,
    /// Detections below this confidence are dropped.
    pub min_confidence: f32,
    /// Time padding around the detection. Covers ASR timestamp error and OCR sample gaps.
    pub pad_before_ms: i64,
    pub pad_after_ms: i64,
}

impl CategoryPolicy {
    pub fn pad_before(&self) -> Dur {
        Dur::from_millis(self.pad_before_ms)
    }
    pub fn pad_after(&self) -> Dur {
        Dur::from_millis(self.pad_after_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Policy {
    /// Broadcast delay. Everything (ASR, LLM, OCR, streamer review) must fit inside it.
    pub delay_ms: i64,
    /// Redactions of the same kind closer than this get merged into one.
    pub merge_gap_ms: i64,
    /// No finalized words for this long, while audio has energy, counts as a stall.
    pub stall_after_ms: i64,
    pub stall_fallback: StallFallback,
    pub categories: BTreeMap<Category, CategoryPolicy>,
}

impl Policy {
    pub fn delay(&self) -> Dur {
        Dur::from_millis(self.delay_ms)
    }
    pub fn merge_gap(&self) -> Dur {
        Dur::from_millis(self.merge_gap_ms)
    }
    pub fn get(&self, c: Category) -> Option<&CategoryPolicy> {
        self.categories.get(&c)
    }

    pub fn from_toml(s: &str) -> Result<Policy> {
        let p: Policy = toml::from_str(s)?;
        p.validate()?;
        Ok(p)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn validate(&self) -> Result<()> {
        if !(1_000..=60_000).contains(&self.delay_ms) {
            return Err(Error::Policy(format!(
                "delay_ms must be 1000..=60000, got {}",
                self.delay_ms
            )));
        }
        for (c, cp) in &self.categories {
            if !(0.0..=1.0).contains(&cp.min_confidence) {
                return Err(Error::Policy(format!(
                    "{c:?}: min_confidence must be 0..=1"
                )));
            }
            if cp.pad_before_ms < 0 || cp.pad_after_ms < 0 {
                return Err(Error::Policy(format!("{c:?}: padding must be >= 0")));
            }
        }
        if !(500..=60_000).contains(&self.stall_after_ms) {
            return Err(Error::Policy(format!(
                "stall_after_ms must be 500..=60000, got {}",
                self.stall_after_ms
            )));
        }
        Ok(())
    }
}

impl Default for Policy {
    fn default() -> Self {
        use AudioAction as A;
        use VideoAction as V;
        let cp = |mode, audio, video, min_confidence, pad_before_ms, pad_after_ms| CategoryPolicy {
            mode,
            audio,
            video,
            min_confidence,
            pad_before_ms,
            pad_after_ms,
        };
        let mut categories = BTreeMap::new();
        categories.insert(
            Category::Profanity,
            cp(Mode::Apply, A::Bleep, V::None, 0.6, 80, 80),
        );
        categories.insert(
            Category::Slur,
            cp(Mode::Apply, A::Bleep, V::None, 0.5, 120, 120),
        );
        categories.insert(
            Category::SpokenPii,
            cp(Mode::Apply, A::Mute, V::None, 0.6, 150, 150),
        );
        // OCR samples every ~250ms, so pad generously: text could have appeared between samples.
        categories.insert(
            Category::ScreenSecret,
            cp(Mode::Apply, A::None, V::Blur, 0.5, 500, 750),
        );
        categories.insert(
            Category::ScreenPii,
            cp(Mode::Suggest, A::None, V::Blur, 0.6, 500, 750),
        );
        // LLM judgments have the highest false-positive cost, so suggest by default.
        categories.insert(
            Category::TosRisk,
            cp(Mode::Suggest, A::Mute, V::None, 0.75, 200, 200),
        );
        Policy {
            delay_ms: 8_000,
            merge_gap_ms: 250,
            stall_after_ms: 3_000,
            stall_fallback: StallFallback::Warn,
            categories,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let p = Policy::default();
        let s = p.to_toml().unwrap();
        assert_eq!(Policy::from_toml(&s).unwrap(), p);
    }

    #[test]
    fn shipped_default_file_parses() {
        let s = include_str!("../../../config/policy.default.toml");
        let p = Policy::from_toml(s).unwrap();
        assert_eq!(
            p,
            Policy::default(),
            "config/policy.default.toml drifted from Policy::default()"
        );
    }

    #[test]
    fn rejects_bad_delay() {
        let p = Policy {
            delay_ms: 100,
            ..Policy::default()
        };
        assert!(p.validate().is_err());
    }
}
