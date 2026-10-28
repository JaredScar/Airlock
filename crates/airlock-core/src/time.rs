//! Media time. Everything in Airlock is keyed on ingest PTS converted to
//! microseconds since stream start. Wall clock only matters for pacing output.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

/// A point in media time: microseconds since stream start.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Ts(pub i64);

/// A length of media time in microseconds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Dur(pub i64);

impl Ts {
    pub const ZERO: Ts = Ts(0);
    pub fn from_millis(ms: i64) -> Self {
        Ts(ms * 1_000)
    }
    pub fn from_secs_f64(s: f64) -> Self {
        Ts((s * 1_000_000.0).round() as i64)
    }
    pub fn as_millis(self) -> i64 {
        self.0 / 1_000
    }
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }
}

impl Dur {
    pub const ZERO: Dur = Dur(0);
    pub fn from_millis(ms: i64) -> Self {
        Dur(ms * 1_000)
    }
    pub fn from_secs_f64(s: f64) -> Self {
        Dur((s * 1_000_000.0).round() as i64)
    }
    pub fn as_millis(self) -> i64 {
        self.0 / 1_000
    }
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }
}

impl Add<Dur> for Ts {
    type Output = Ts;
    fn add(self, rhs: Dur) -> Ts {
        Ts(self.0 + rhs.0)
    }
}
impl Sub<Dur> for Ts {
    type Output = Ts;
    fn sub(self, rhs: Dur) -> Ts {
        Ts(self.0 - rhs.0)
    }
}
impl Sub<Ts> for Ts {
    type Output = Dur;
    fn sub(self, rhs: Ts) -> Dur {
        Dur(self.0 - rhs.0)
    }
}

impl fmt::Display for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_ms = self.0.max(0) / 1_000;
        let (h, m, s, ms) = (
            total_ms / 3_600_000,
            (total_ms / 60_000) % 60,
            (total_ms / 1000) % 60,
            total_ms % 1000,
        );
        write!(f, "{h:02}:{m:02}:{s:02}.{ms:03}")
    }
}

/// Half-open interval `[start, end)` in media time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: Ts,
    pub end: Ts,
}

impl Span {
    /// Builds a span, swapping the ends if they arrive reversed.
    pub fn new(a: Ts, b: Ts) -> Self {
        if a <= b {
            Span { start: a, end: b }
        } else {
            Span { start: b, end: a }
        }
    }
    pub fn from_millis(start_ms: i64, end_ms: i64) -> Self {
        Span::new(Ts::from_millis(start_ms), Ts::from_millis(end_ms))
    }
    pub fn duration(&self) -> Dur {
        self.end - self.start
    }
    pub fn contains(&self, t: Ts) -> bool {
        t >= self.start && t < self.end
    }
    pub fn overlaps(&self, other: &Span) -> bool {
        self.start < other.end && other.start < self.end
    }
    /// True when the spans overlap or the gap between them is at most `gap`.
    pub fn near(&self, other: &Span, gap: Dur) -> bool {
        self.start <= other.end + gap && other.start <= self.end + gap
    }
    pub fn union(&self, other: &Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
    pub fn pad(&self, before: Dur, after: Dur) -> Span {
        Span {
            start: self.start - before,
            end: self.end + after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_basics() {
        let a = Span::from_millis(1000, 2000);
        let b = Span::from_millis(1900, 2500);
        let c = Span::from_millis(2100, 2200);
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
        assert!(a.near(&c, Dur::from_millis(100)));
        assert!(!a.near(&c, Dur::from_millis(50)));
        assert_eq!(a.union(&b), Span::from_millis(1000, 2500));
        assert_eq!(
            a.pad(Dur::from_millis(80), Dur::from_millis(80)),
            Span::from_millis(920, 2080)
        );
        assert_eq!(Span::from_millis(5, 1), Span::from_millis(1, 5));
    }

    #[test]
    fn display() {
        assert_eq!(Ts::from_millis(3_723_045).to_string(), "01:02:03.045");
    }
}
