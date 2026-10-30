//! On-screen analysis: decide which frames to sample, run OCR on them, and hand
//! text boxes to `airlock_detect::ScreenTextScanner`.
//!
//! Why sampling works: the output is delayed, so when a secret is first seen at
//! sample k we can blur *backwards* to sample k-1. Nothing between samples airs
//! unredacted as long as OCR finishes inside the delay budget.

use airlock_core::{Dur, Span, TextBox, Ts};
use thiserror::Error;

#[cfg(feature = "winocr")]
mod winocr;

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

/// A downscaled BGRA frame. Coordinates stay normalized, so a detection on this
/// image maps back onto the original frame without a scale factor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampledFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Nearest-neighbor shrink so the longer edge is at most `max_edge`.
pub fn downscale_bgra(frame: &FrameRef<'_>, max_edge: u32) -> Result<SampledFrame, VisionError> {
    if frame.format != PixelFormat::Bgra8 {
        return Err(VisionError::NotImplemented("downscale expects BGRA8"));
    }
    if frame.width == 0 || frame.height == 0 || max_edge == 0 {
        return Err(VisionError::Ocr("empty frame".into()));
    }
    let long = frame.width.max(frame.height);
    if long <= max_edge {
        return Ok(copy_bgra(frame));
    }
    let scale = max_edge as f64 / long as f64;
    let width = ((frame.width as f64) * scale).round().max(1.0) as u32;
    let height = ((frame.height as f64) * scale).round().max(1.0) as u32;
    let mut data = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let src_y = (y as u64 * frame.height as u64 / height as u64) as u32;
        for x in 0..width {
            let src_x = (x as u64 * frame.width as u64 / width as u64) as u32;
            let src = (src_y as usize) * frame.stride as usize + (src_x as usize) * 4;
            let dst = ((y * width + x) * 4) as usize;
            data[dst..dst + 4].copy_from_slice(&frame.data[src..src + 4]);
        }
    }
    Ok(SampledFrame {
        width,
        height,
        data,
    })
}

fn copy_bgra(frame: &FrameRef<'_>) -> SampledFrame {
    let width = frame.width;
    let height = frame.height;
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row = (y as usize) * frame.stride as usize;
        let end = row + (width as usize) * 4;
        data.extend_from_slice(&frame.data[row..end]);
    }
    SampledFrame {
        width,
        height,
        data,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackedBox {
    pub id: u64,
    pub rect: airlock_core::Rect,
}

/// Keeps an id when a box still overlaps the previous sample, so a blur follows
/// a moving window instead of flickering on and off.
pub fn associate_regions(
    previous: &[TrackedBox],
    next: &[airlock_core::Rect],
    min_iou: f32,
    next_id: &mut u64,
) -> Vec<TrackedBox> {
    let mut used = vec![false; previous.len()];
    let mut out = Vec::with_capacity(next.len());
    for rect in next {
        let mut best: Option<(usize, f32)> = None;
        for (i, prev) in previous.iter().enumerate() {
            if used[i] {
                continue;
            }
            let score = iou(&prev.rect, rect);
            if score < min_iou {
                continue;
            }
            if best.map(|(_, s)| score > s).unwrap_or(true) {
                best = Some((i, score));
            }
        }
        let id = if let Some((i, _)) = best {
            used[i] = true;
            previous[i].id
        } else {
            let id = *next_id;
            *next_id += 1;
            id
        };
        out.push(TrackedBox { id, rect: *rect });
    }
    out
}

fn iou(a: &airlock_core::Rect, b: &airlock_core::Rect) -> f32 {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    let w = (x1 - x0).max(0.0);
    let h = (y1 - y0).max(0.0);
    let inter = w * h;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}
pub struct WindowsOcr;

impl TextRecognizer for WindowsOcr {
    fn name(&self) -> &str {
        "windows-ocr"
    }
    fn recognize(&mut self, frame: &FrameRef<'_>) -> Result<Vec<TextBox>, VisionError> {
        #[cfg(feature = "winocr")]
        {
            winocr::recognize(frame)
        }
        #[cfg(not(feature = "winocr"))]
        {
            let _ = frame;
            Err(VisionError::NotImplemented("WindowsOcr::recognize"))
        }
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

    #[test]
    fn downscale_keeps_the_right_half_on_the_right() {
        let width = 4u32;
        let height = 2u32;
        let mut data = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let i = ((y * width + x) * 4) as usize;
                data[i] = if x < 2 { 10 } else { 200 };
                data[i + 3] = 255;
            }
        }
        let frame = FrameRef {
            ts: Ts::ZERO,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Bgra8,
            data: &data,
        };
        let small = downscale_bgra(&frame, 2).unwrap();
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.data[0], 10);
        assert_eq!(small.data[4], 200);
        let rect = airlock_core::Rect {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        let (x, _, w, _) = rect.to_pixels(small.width, small.height);
        assert_eq!(x, 1);
        assert!(w >= 1);
    }

    #[test]
    fn a_moving_box_keeps_its_id_and_a_new_one_does_not() {
        let mut next_id = 1u64;
        let first = associate_regions(
            &[],
            &[airlock_core::Rect {
                x: 0.10,
                y: 0.10,
                w: 0.20,
                h: 0.20,
            }],
            0.3,
            &mut next_id,
        );
        assert_eq!(first[0].id, 1);
        let second = associate_regions(
            &first,
            &[
                airlock_core::Rect {
                    x: 0.14,
                    y: 0.12,
                    w: 0.20,
                    h: 0.20,
                },
                airlock_core::Rect {
                    x: 0.70,
                    y: 0.70,
                    w: 0.10,
                    h: 0.10,
                },
            ],
            0.3,
            &mut next_id,
        );
        assert_eq!(second[0].id, 1);
        assert_eq!(second[1].id, 2);
    }

    #[cfg(all(windows, feature = "winocr"))]
    #[test]
    fn windows_ocr_reads_a_painted_word() {
        let width = 640u32;
        let height = 180u32;
        let pixels = paint_word(width, height, "AIRLOCK");
        let frame = FrameRef {
            ts: Ts::ZERO,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Bgra8,
            data: &pixels,
        };
        let boxes = WindowsOcr.recognize(&frame).expect("ocr");
        let text = boxes
            .iter()
            .map(|b| b.text.to_uppercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("AIRLOCK"), "{text}");
        assert!(boxes.iter().all(|b| {
            (0.0..=1.0).contains(&b.rect.x)
                && (0.0..=1.0).contains(&b.rect.y)
                && b.rect.w > 0.0
                && b.rect.h > 0.0
        }));
    }

    #[cfg(all(windows, feature = "winocr"))]
    fn paint_word(width: u32, height: u32, word: &str) -> Vec<u8> {
        use windows::Win32::Foundation::{COLORREF, RECT};
        use windows::Win32::Graphics::Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS,
            DT_LEFT, DT_SINGLELINE, DT_TOP, DeleteDC, DeleteObject, DrawTextW, FW_SEMIBOLD,
            OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        };
        use windows::core::PCWSTR;

        unsafe {
            let hdc = CreateCompatibleDC(None);
            assert!(!hdc.is_invalid(), "display context");
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let dib =
                CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            SelectObject(hdc, dib.into());
            let n = (width * height * 4) as usize;
            std::ptr::write_bytes(bits, 0xFF, n);
            let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
            let font = CreateFontW(
                72,
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                PCWSTR(face.as_ptr()),
            );
            SelectObject(hdc, font.into());
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0));
            let mut wide: Vec<u16> = word.encode_utf16().collect();
            let mut rect = RECT {
                left: 32,
                top: 48,
                right: (width as i32) - 16,
                bottom: (height as i32) - 16,
            };
            DrawTextW(hdc, &mut wide, &mut rect, DT_LEFT | DT_SINGLELINE | DT_TOP);
            let mut out = vec![0u8; n];
            std::ptr::copy_nonoverlapping(bits.cast::<u8>(), out.as_mut_ptr(), n);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(dib.into());
            let _ = DeleteDC(hdc);
            out
        }
    }
}
