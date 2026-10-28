//! Turns detections into redactions according to policy, merges overlapping ones,
//! tracks streamer approve/veto, and answers "what do I redact in this chunk?"
//! for the output stage.

use crate::error::{Error, Result};
use crate::model::{Category, Detection, DetectorKind, Modality, Rect};
use crate::policy::{AudioAction, Mode, Policy, VideoAction};
use crate::time::{Span, Ts};
use serde::{Deserialize, Serialize};

pub type RedactionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// The whole program audio mix.
    Audio,
    /// A region of the video frame (normalized coordinates).
    Video { region: Rect },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Bleep,
    Mute,
    Blur,
    Pixelate,
    Blackout,
    /// Recorded for the incident log only.
    LogOnly,
}

/// Streamer review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Approved,
    Vetoed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redaction {
    pub id: RedactionId,
    pub category: Category,
    pub mode: Mode,
    pub action: Action,
    pub target: Target,
    pub span: Span,
    pub status: Status,
    pub confidence: f32,
    pub sources: Vec<DetectorKind>,
    pub reasons: Vec<String>,
    /// Masked evidence strings, safe to show.
    pub evidence: Vec<String>,
}

impl Redaction {
    /// Whether the output stage should actually apply this redaction.
    pub fn is_active(&self) -> bool {
        if self.action == Action::LogOnly {
            return false;
        }
        match self.mode {
            Mode::Apply => self.status != Status::Vetoed,
            Mode::Suggest => self.status == Status::Approved,
            Mode::LogOnly => false,
        }
    }

    pub fn is_audio(&self) -> bool {
        matches!(self.target, Target::Audio)
    }
}

#[derive(Debug)]
pub struct DecisionEngine {
    policy: Policy,
    next_id: RedactionId,
    redactions: Vec<Redaction>,
}

impl DecisionEngine {
    pub fn new(policy: Policy) -> Self {
        DecisionEngine {
            policy,
            next_id: 1,
            redactions: Vec::new(),
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
    }

    /// Feeds one detection. Returns the id of the redaction it created or merged into,
    /// or `None` if policy drops it (category disabled or confidence too low).
    pub fn ingest(&mut self, d: Detection) -> Option<RedactionId> {
        let cp = self.policy.get(d.category)?.clone();
        if d.confidence < cp.min_confidence {
            return None;
        }

        let (action, target) = match d.modality {
            Modality::Audio => (
                match cp.audio {
                    AudioAction::Bleep => Action::Bleep,
                    AudioAction::Mute => Action::Mute,
                    AudioAction::None => Action::LogOnly,
                },
                Target::Audio,
            ),
            Modality::Video => (
                match cp.video {
                    VideoAction::Blur => Action::Blur,
                    VideoAction::Pixelate => Action::Pixelate,
                    VideoAction::Blackout => Action::Blackout,
                    VideoAction::None => Action::LogOnly,
                },
                Target::Video {
                    region: d.region.map(|r| r.pad(0.01)).unwrap_or(Rect::FULL),
                },
            ),
        };
        let action = if cp.mode == Mode::LogOnly {
            Action::LogOnly
        } else {
            action
        };
        let span = d.span.pad(cp.pad_before(), cp.pad_after());
        let gap = self.policy.merge_gap();

        // Merge into an existing compatible redaction if one is close enough.
        if let Some(r) = self.redactions.iter_mut().find(|r| {
            r.category == d.category
                && r.action == action
                && r.status != Status::Vetoed
                && r.span.near(&span, gap)
                && match (&r.target, &target) {
                    (Target::Audio, Target::Audio) => true,
                    (Target::Video { region: a }, Target::Video { region: b }) => a.intersects(b),
                    _ => false,
                }
        }) {
            r.span = r.span.union(&span);
            if let (Target::Video { region: a }, Target::Video { region: b }) =
                (&mut r.target, &target)
            {
                *a = a.union(b);
            }
            r.confidence = r.confidence.max(d.confidence);
            if !r.sources.contains(&d.source) {
                r.sources.push(d.source);
            }
            if !r.reasons.contains(&d.reason) {
                r.reasons.push(d.reason);
            }
            if !d.evidence.is_empty() && !r.evidence.contains(&d.evidence) {
                r.evidence.push(d.evidence);
            }
            return Some(r.id);
        }

        let id = self.next_id;
        self.next_id += 1;
        self.redactions.push(Redaction {
            id,
            category: d.category,
            mode: cp.mode,
            action,
            target,
            span,
            status: Status::Pending,
            confidence: d.confidence,
            sources: vec![d.source],
            reasons: vec![d.reason],
            evidence: if d.evidence.is_empty() {
                vec![]
            } else {
                vec![d.evidence]
            },
        });
        Some(id)
    }

    pub fn approve(&mut self, id: RedactionId) -> Result<()> {
        self.set_status(id, Status::Approved)
    }

    pub fn veto(&mut self, id: RedactionId) -> Result<()> {
        self.set_status(id, Status::Vetoed)
    }

    fn set_status(&mut self, id: RedactionId, s: Status) -> Result<()> {
        let r = self
            .redactions
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(Error::UnknownRedaction(id))?;
        r.status = s;
        Ok(())
    }

    pub fn get(&self, id: RedactionId) -> Option<&Redaction> {
        self.redactions.iter().find(|r| r.id == id)
    }

    /// Active audio redactions overlapping an outgoing audio chunk.
    pub fn audio_redactions_in(&self, chunk: Span) -> impl Iterator<Item = &Redaction> {
        self.redactions
            .iter()
            .filter(move |r| r.is_audio() && r.is_active() && r.span.overlaps(&chunk))
    }

    /// Active video redactions covering an outgoing frame.
    pub fn video_redactions_at(&self, frame: Ts) -> impl Iterator<Item = &Redaction> {
        self.redactions
            .iter()
            .filter(move |r| !r.is_audio() && r.is_active() && r.span.contains(frame))
    }

    /// Everything that hasn't fully aired yet, for the review panel. Sorted by start.
    pub fn upcoming(&self, playhead: Ts) -> Vec<&Redaction> {
        let mut v: Vec<_> = self
            .redactions
            .iter()
            .filter(|r| r.span.end > playhead)
            .collect();
        v.sort_by_key(|r| r.span.start);
        v
    }

    /// Removes redactions that finished airing before `playhead` and returns them
    /// for the incident log.
    pub fn retire_before(&mut self, playhead: Ts) -> Vec<Redaction> {
        let (done, keep): (Vec<_>, Vec<_>) = self
            .redactions
            .drain(..)
            .partition(|r| r.span.end <= playhead);
        self.redactions = keep;
        done
    }

    pub fn all(&self) -> &[Redaction] {
        &self.redactions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(cat: Category, modality: Modality, start: i64, end: i64, conf: f32) -> Detection {
        Detection {
            source: DetectorKind::Wordlist,
            category: cat,
            modality,
            span: Span::from_millis(start, end),
            region: None,
            confidence: conf,
            reason: "test".into(),
            evidence: String::new(),
        }
    }

    #[test]
    fn profanity_is_padded_and_active() {
        let mut e = DecisionEngine::new(Policy::default());
        let id = e
            .ingest(det(Category::Profanity, Modality::Audio, 3000, 3300, 0.9))
            .unwrap();
        let r = e.get(id).unwrap();
        assert_eq!(r.action, Action::Bleep);
        assert_eq!(r.span, Span::from_millis(2920, 3380));
        assert!(r.is_active());
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(3000, 3020)).count(),
            1
        );
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(4000, 4020)).count(),
            0
        );
    }

    #[test]
    fn low_confidence_dropped() {
        let mut e = DecisionEngine::new(Policy::default());
        assert!(
            e.ingest(det(Category::Profanity, Modality::Audio, 0, 100, 0.1))
                .is_none()
        );
    }

    #[test]
    fn nearby_words_merge() {
        let mut e = DecisionEngine::new(Policy::default());
        let a = e
            .ingest(det(Category::Profanity, Modality::Audio, 1000, 1200, 0.9))
            .unwrap();
        let b = e
            .ingest(det(Category::Profanity, Modality::Audio, 1400, 1600, 0.9))
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(e.all().len(), 1);
        assert_eq!(e.get(a).unwrap().span, Span::from_millis(920, 1680));
        // Far away = separate redaction.
        let c = e
            .ingest(det(Category::Profanity, Modality::Audio, 5000, 5200, 0.9))
            .unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn veto_and_suggest() {
        let mut e = DecisionEngine::new(Policy::default());
        let p = e
            .ingest(det(Category::Profanity, Modality::Audio, 0, 100, 0.9))
            .unwrap();
        e.veto(p).unwrap();
        assert!(!e.get(p).unwrap().is_active());

        // TOS risk defaults to Suggest: inactive until approved.
        let t = e
            .ingest(det(Category::TosRisk, Modality::Audio, 2000, 4000, 0.9))
            .unwrap();
        assert!(!e.get(t).unwrap().is_active());
        e.approve(t).unwrap();
        assert!(e.get(t).unwrap().is_active());
    }

    #[test]
    fn video_regions_merge_when_overlapping() {
        let mut e = DecisionEngine::new(Policy::default());
        let mut d1 = det(Category::ScreenSecret, Modality::Video, 1000, 1250, 0.9);
        d1.region = Some(Rect {
            x: 0.1,
            y: 0.1,
            w: 0.3,
            h: 0.05,
        });
        let mut d2 = det(Category::ScreenSecret, Modality::Video, 1250, 1500, 0.9);
        d2.region = Some(Rect {
            x: 0.12,
            y: 0.1,
            w: 0.3,
            h: 0.05,
        });
        let mut d3 = det(Category::ScreenSecret, Modality::Video, 1250, 1500, 0.9);
        d3.region = Some(Rect {
            x: 0.7,
            y: 0.8,
            w: 0.1,
            h: 0.05,
        });
        let a = e.ingest(d1).unwrap();
        let b = e.ingest(d2).unwrap();
        let c = e.ingest(d3).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(e.video_redactions_at(Ts::from_millis(1300)).count(), 2);
    }

    #[test]
    fn retire_moves_aired_items_out() {
        let mut e = DecisionEngine::new(Policy::default());
        e.ingest(det(Category::Profanity, Modality::Audio, 0, 100, 0.9));
        e.ingest(det(
            Category::Profanity,
            Modality::Audio,
            10_000,
            10_100,
            0.9,
        ));
        let done = e.retire_before(Ts::from_millis(5000));
        assert_eq!(done.len(), 1);
        assert_eq!(e.upcoming(Ts::from_millis(5000)).len(), 1);
    }
}
