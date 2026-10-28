//! On-screen analysis: decide which frames to sample, run OCR on them, and hand
//! text boxes to `airlock_detect::ScreenTextScanner`.
//!
//! Why sampling works: the output is delayed, so when a secret is first seen at
//! sample k we can blur *backwards* to sample k-1. Nothing between samples airs
//! unredacted as long as OCR finishes inside the delay budget.

use airlock_core::{Dur, Span, TextBox, Ts};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VisionError {
    #[error("{0} is not implemented yet (see PLAN.md)")]
    NotImplemented(&'static str),
    #[error("ocr failed: {0}")]
    Ocr(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8,
    Nv12,
}

/// A borrowed decoded frame (CPU memory). GPU frames are a later optimization.
pub struct FrameRef<'a> {
    pub ts: Ts,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: &'a [u8],
}

pub trait TextRecognizer: Send {
    fn name(&self) -> &str;
    /// Returns text boxes in normalized coordinates.
    fn recognize(&mut self, frame: &FrameRef<'_>) -> Result<Vec<TextBox>, VisionError>;
}

/// Picks frames to OCR at a fixed rate, and computes the span each sample covers.
#[derive(Debug)]
pub struct FrameSampler {
    interval: Dur,
    last: Option<Ts>,
}

impl FrameSampler {
    pub fn new(fps: f64) -> Self {
        FrameSampler {
            interval: Dur::from_secs_f64(1.0 / fps.max(0.1)),
            last: None,
        }
    }

    /// Returns `Some(span)` if this frame should be sampled. The span runs from the
    /// previous sample to this one, which is what a detection here must cover.
    pub fn offer(&mut self, ts: Ts) -> Option<Span> {
        match self.last {
            Some(prev) if ts - prev < self.interval => None,
            prev => {
                self.last = Some(ts);
                Some(Span::new(prev.unwrap_or(ts), ts))
            }
        }
    }
}

/// Windows.Media.Ocr via the `windows` crate. Fast, free, on every Windows 10/11 box.
/// NOT IMPLEMENTED: PLAN.md Phase 4.
pub struct WindowsOcr;

impl TextRecognizer for WindowsOcr {
    fn name(&self) -> &str {
        "windows-ocr"
    }
    fn recognize(&mut self, _frame: &FrameRef<'_>) -> Result<Vec<TextBox>, VisionError> {
        Err(VisionError::NotImplemented("WindowsOcr::recognize"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_at_rate_and_covers_gaps() {
        let mut s = FrameSampler::new(4.0); // every 250ms
        let picked: Vec<_> = (0..30)
            .filter_map(|i| s.offer(Ts::from_millis(i * 1000 / 60)))
            .collect();
        // 30 frames at 60fps = 500ms -> samples at ~0, ~250, ~500ms
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0], Span::new(Ts::ZERO, Ts::ZERO));
        assert_eq!(picked[1].start, Ts::ZERO);
        assert!(picked[1].end >= Ts::from_millis(250));
    }
}
