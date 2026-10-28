//! What the analysis taps produce.

use crate::time::Span;
use serde::{Deserialize, Serialize};

/// What kind of risk a detection represents. Policy is configured per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Ordinary swearing.
    Profanity,
    /// Slurs and hate speech terms.
    Slur,
    /// The streamer saying identifying info out loud: address, phone, real name, etc.
    SpokenPii,
    /// Credentials visible on screen: API keys, stream keys, passwords, tokens.
    ScreenSecret,
    /// Personal info visible on screen: emails, DMs, phone numbers, addresses.
    ScreenPii,
    /// Context-level platform-rule risk found by the LLM judge (no banned word needed).
    TosRisk,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Profanity,
        Category::Slur,
        Category::SpokenPii,
        Category::ScreenSecret,
        Category::ScreenPii,
        Category::TosRisk,
    ];
}

/// Which stream a detection applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Audio,
    Video,
}

/// Which analyzer produced a detection. Used for the incident log and evals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorKind {
    Wordlist,
    Pii,
    PrivateTerms,
    Secret,
    ContextLlm,
    Ocr,
    VisionModel,
}

/// Target platform. Context rules differ per platform (Twitch vs YouTube vs Kick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    #[default]
    Twitch,
    Youtube,
    Kick,
    Generic,
}

/// Rectangle in normalized frame coordinates (0.0..=1.0), origin top-left.
/// Normalized so detections survive the analysis tap running at a lower resolution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const FULL: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect {
            x,
            y,
            w: self.right().max(o.right()) - x,
            h: self.bottom().max(o.bottom()) - y,
        }
    }
    /// Grows the rect by `m` on every side, clamped to the frame.
    pub fn pad(&self, m: f32) -> Rect {
        let x = (self.x - m).max(0.0);
        let y = (self.y - m).max(0.0);
        Rect {
            x,
            y,
            w: (self.right() + m).min(1.0) - x,
            h: (self.bottom() + m).min(1.0) - y,
        }
    }
    /// Converts to pixel coordinates `(x, y, w, h)` for a frame of the given size.
    pub fn to_pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let x0 = (self.x.clamp(0.0, 1.0) * width as f32).floor() as u32;
        let y0 = (self.y.clamp(0.0, 1.0) * height as f32).floor() as u32;
        let x1 = (self.right().clamp(0.0, 1.0) * width as f32).ceil() as u32;
        let y1 = (self.bottom().clamp(0.0, 1.0) * height as f32).ceil() as u32;
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }
}

/// One recognized word from ASR, with media-time bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub span: Span,
    pub confidence: f32,
}

impl Word {
    pub fn new(text: impl Into<String>, span: Span, confidence: f32) -> Self {
        Word {
            text: text.into(),
            span,
            confidence,
        }
    }
}

/// One OCR'd line or block of on-screen text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub text: String,
    pub rect: Rect,
    pub confidence: f32,
}

/// Something an analyzer thinks should not air. Raw input to the decision engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub source: DetectorKind,
    pub category: Category,
    pub modality: Modality,
    pub span: Span,
    /// Video only; `None` on a video detection means the whole frame.
    pub region: Option<Rect>,
    pub confidence: f32,
    /// Human-readable reason shown to the streamer ("Twitch stream key on screen").
    pub reason: String,
    /// Masked evidence, safe to log and display. Never the raw secret or PII.
    pub evidence: String,
}

/// Masks sensitive text for logs and UI: keeps the first two characters.
/// `"sk-ant-abc123"` -> `"sk•••••••••••"`. Every detector must use this for `evidence`.
pub fn mask_evidence(s: &str) -> String {
    let n = s.chars().count();
    let keep = if n <= 4 { 1 } else { 2 };
    let mut out: String = s.chars().take(keep).collect();
    out.extend(std::iter::repeat_n('•', n.saturating_sub(keep).min(24)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = Rect {
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.2,
        };
        let b = Rect {
            x: 0.25,
            y: 0.25,
            w: 0.2,
            h: 0.2,
        };
        let c = Rect {
            x: 0.6,
            y: 0.6,
            w: 0.1,
            h: 0.1,
        };
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        let u = a.union(&b);
        assert!((u.right() - 0.45).abs() < 1e-6 && (u.bottom() - 0.45).abs() < 1e-6);
        let p = Rect {
            x: 0.0,
            y: 0.95,
            w: 0.1,
            h: 0.05,
        }
        .pad(0.02);
        assert_eq!(p.x, 0.0);
        assert!((p.bottom() - 1.0).abs() < 1e-6);
        assert_eq!(Rect::FULL.to_pixels(1920, 1080), (0, 0, 1920, 1080));
    }

    #[test]
    fn masking_never_leaks() {
        assert_eq!(mask_evidence("sk-ant-abc123"), "sk•••••••••••");
        assert_eq!(mask_evidence("abc"), "a••");
        assert!(mask_evidence(&"x".repeat(200)).chars().count() <= 26);
    }
}
