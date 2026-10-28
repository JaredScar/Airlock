//! Turns detections into redactions according to policy, merges overlapping ones,
//! tracks streamer approve/veto, and answers "what do I redact in this chunk?"
//! for the output stage.

use crate::error::{Error, Result};
use crate::model::{Category, Detection, DetectorKind, Modality, Rect};
use crate::policy::{AudioAction, Mode, Policy, VideoAction};
use crate::time::{Dur, Span, Ts};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub type RedactionId = u64;

#[derive(Clone, Copy)]
struct Planned {
    mode: Mode,
    action: Action,
    target: Target,
    span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Target {
    /// The whole program audio mix.
    Audio,
    /// A region of the video frame (normalized coordinates).
    Video { region: Rect },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Status {
    Pending,
    Approved,
    Vetoed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
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
    /// The span had already aired when this flag arrived. Kept for VOD cleanup.
    /// The output stage must not apply it.
    #[serde(default)]
    pub missed_live: bool,
}

impl Redaction {
    /// Whether the output stage should actually apply this redaction.
    pub fn is_active(&self) -> bool {
        if self.missed_live || self.action == Action::LogOnly {
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
        let planned = self.classify(&d)?;
        let gap = self.policy.merge_gap();

        // Merge into an existing compatible redaction if one is close enough.
        if let Some(r) = self.redactions.iter_mut().find(|r| {
            r.category == d.category
                && r.action == planned.action
                && r.status != Status::Vetoed
                && r.span.near(&planned.span, gap)
                && match (&r.target, &planned.target) {
                    (Target::Audio, Target::Audio) => true,
                    (Target::Video { region: a }, Target::Video { region: b }) => a.intersects(b),
                    _ => false,
                }
        }) {
            r.span = r.span.union(&planned.span);
            if let (Target::Video { region: a }, Target::Video { region: b }) =
                (&mut r.target, &planned.target)
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
            mode: planned.mode,
            action: planned.action,
            target: planned.target,
            span: planned.span,
            status: Status::Pending,
            confidence: d.confidence,
            sources: vec![d.source],
            reasons: vec![d.reason],
            evidence: if d.evidence.is_empty() {
                vec![]
            } else {
                vec![d.evidence]
            },
            missed_live: false,
        });
        Some(id)
    }

    /// Records a detection whose span has already aired. It is not added to the
    /// live list, so the output stage cannot apply it. The caller files the
    /// returned redaction on the incident log.
    pub fn file_missed(&mut self, d: Detection) -> Option<Redaction> {
        let planned = self.classify(&d)?;
        let id = self.next_id;
        self.next_id += 1;
        let mut reasons = vec![d.reason];
        reasons.push("Missed live".into());
        Some(Redaction {
            id,
            category: d.category,
            mode: planned.mode,
            action: planned.action,
            target: planned.target,
            span: planned.span,
            status: Status::Pending,
            confidence: d.confidence,
            sources: vec![d.source],
            reasons,
            evidence: if d.evidence.is_empty() {
                vec![]
            } else {
                vec![d.evidence]
            },
            missed_live: true,
        })
    }

    fn classify(&self, d: &Detection) -> Option<Planned> {
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
        Some(Planned {
            mode: cp.mode,
            action,
            target,
            span: d.span.pad(cp.pad_before(), cp.pad_after()),
        })
    }

    /// Mutes everything still in the delay at media time `at`.
    /// The covered interval is (at − delay, at]: a chunk that ends at `at − delay`
    /// is already on air, and the sample at `at` is included.
    /// `blackout_video` adds a full-frame blackout over the same interval.
    /// Does not change the delay length.
    pub fn dump(&mut self, at: Ts, delay: Dur, blackout_video: bool) -> Vec<RedactionId> {
        let start = if at.0 > delay.0 { at - delay } else { Ts::ZERO };
        let span = Span {
            start,
            end: at + Dur(1),
        };
        let mut ids = vec![self.push_manual(Action::Mute, Target::Audio, span, "Dump")];
        if blackout_video {
            ids.push(self.push_manual(
                Action::Blackout,
                Target::Video { region: Rect::FULL },
                span,
                "Dump",
            ));
        }
        ids
    }

    /// Inserts or extends a manual redaction. Used by dump and the stall fail-safe.
    /// No policy padding: the span is exactly what the caller asked to hold back.
    pub fn cover_manual(
        &mut self,
        action: Action,
        target: Target,
        span: Span,
        reason: &str,
    ) -> RedactionId {
        self.push_manual(action, target, span, reason)
    }

    fn push_manual(
        &mut self,
        action: Action,
        target: Target,
        span: Span,
        reason: &str,
    ) -> RedactionId {
        let gap = self.policy.merge_gap();
        if let Some(r) = self.redactions.iter_mut().find(|r| {
            r.category == Category::Manual
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
            if !r.reasons.iter().any(|existing| existing == reason) {
                r.reasons.push(reason.to_string());
            }
            return r.id;
        }

        let id = self.next_id;
        self.next_id += 1;
        self.redactions.push(Redaction {
            id,
            category: Category::Manual,
            mode: Mode::Apply,
            action,
            target,
            span,
            status: Status::Pending,
            confidence: 1.0,
            sources: vec![DetectorKind::Manual],
            reasons: vec![reason.to_string()],
            evidence: vec![],
            missed_live: false,
        });
        id
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
    fn dump_covers_the_open_hold_through_now() {
        let mut e = DecisionEngine::new(Policy::default());
        let at = Ts::from_millis(20_000);
        let delay = Dur::from_millis(8_000);
        let ids = e.dump(at, delay, false);
        assert_eq!(ids.len(), 1);
        let r = e.get(ids[0]).unwrap();
        assert_eq!(r.category, Category::Manual);
        assert_eq!(r.action, Action::Mute);
        assert!(r.is_active());
        // (T − delay, T] is muted. The chunk that ends exactly at T − delay has aired.
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(12_000, 12_000))
                .count(),
            0
        );
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(10_000, 12_000))
                .count(),
            0
        );
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(12_001, 20_001))
                .count(),
            1
        );
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(20_002, 21_000))
                .count(),
            0
        );
        assert_eq!(e.video_redactions_at(Ts::from_millis(15_000)).count(), 0);

        e.dump(at, delay, true);
        assert_eq!(e.video_redactions_at(Ts::from_millis(15_000)).count(), 1);
        e.veto(ids[0]).unwrap();
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(15_000, 15_100))
                .count(),
            0
        );
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

    #[test]
    fn missed_flag_is_logged_and_not_applied() {
        let mut e = DecisionEngine::new(Policy::default());
        let missed = e
            .file_missed(det(Category::Profanity, Modality::Audio, 1000, 1400, 0.9))
            .unwrap();
        assert!(missed.missed_live);
        assert!(!missed.is_active());
        assert!(missed.reasons.iter().any(|r| r == "Missed live"));
        assert!(e.all().is_empty());
        assert_eq!(
            e.audio_redactions_in(Span::from_millis(1000, 1400)).count(),
            0
        );
        assert!(
            e.file_missed(det(Category::Profanity, Modality::Audio, 0, 100, 0.1))
                .is_none()
        );
    }
}
