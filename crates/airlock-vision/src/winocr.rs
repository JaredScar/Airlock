//! Windows.Media.Ocr. Compiled only with `--features winocr` so Linux CI does
//! not need the WinRT bindings. Coordinates come back normalized (origin top-left).

use airlock_core::{Rect, TextBox};
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

use crate::{FrameRef, PixelFormat, VisionError};

pub fn recognize(frame: &FrameRef<'_>) -> Result<Vec<TextBox>, VisionError> {
    if frame.format != PixelFormat::Bgra8 {
        return Err(VisionError::Ocr("Windows OCR expects BGRA8".into()));
    }
    if frame.width == 0 || frame.height == 0 {
        return Ok(Vec::new());
    }
    let bmp = encode_bmp(frame);
    let bitmap = decode_bitmap(&bmp)?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| VisionError::Ocr(e.to_string()))?
        .get()
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    let lines = result
        .Lines()
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    let width = frame.width as f32;
    let height = frame.height as f32;
    let mut boxes = Vec::new();
    for line in lines {
        let text = line.Text().map_err(|e| VisionError::Ocr(e.to_string()))?;
        let text = text.to_string();
        if text.trim().is_empty() {
            continue;
        }
        let words = line.Words().map_err(|e| VisionError::Ocr(e.to_string()))?;
        let mut x0 = f32::MAX;
        let mut y0 = f32::MAX;
        let mut x1 = 0.0f32;
        let mut y1 = 0.0f32;
        let mut any = false;
        for word in words {
            let r = word
                .BoundingRect()
                .map_err(|e| VisionError::Ocr(e.to_string()))?;
            any = true;
            x0 = x0.min(r.X);
            y0 = y0.min(r.Y);
            x1 = x1.max(r.X + r.Width);
            y1 = y1.max(r.Y + r.Height);
        }
        if !any {
            continue;
        }
        boxes.push(TextBox {
            text,
            rect: Rect {
                x: (x0 / width).clamp(0.0, 1.0),
                y: (y0 / height).clamp(0.0, 1.0),
                w: ((x1 - x0) / width).clamp(0.0, 1.0),
                h: ((y1 - y0) / height).clamp(0.0, 1.0),
            },
            confidence: 0.9,
        });
    }
    Ok(boxes)
}

fn decode_bitmap(bmp: &[u8]) -> Result<windows::Graphics::Imaging::SoftwareBitmap, VisionError> {
    let stream = InMemoryRandomAccessStream::new().map_err(|e| VisionError::Ocr(e.to_string()))?;
    let writer =
        DataWriter::CreateDataWriter(&stream).map_err(|e| VisionError::Ocr(e.to_string()))?;
    writer
        .WriteBytes(bmp)
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    writer
        .StoreAsync()
        .map_err(|e| VisionError::Ocr(e.to_string()))?
        .get()
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    writer
        .DetachStream()
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    stream
        .Seek(0)
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| VisionError::Ocr(e.to_string()))?
        .get()
        .map_err(|e| VisionError::Ocr(e.to_string()))?;
    decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| VisionError::Ocr(e.to_string()))?
        .get()
        .map_err(|e| VisionError::Ocr(e.to_string()))
}

fn encode_bmp(frame: &FrameRef<'_>) -> Vec<u8> {
    let width = frame.width;
    let height = frame.height;
    let row = (width * 4) as usize;
    let pixels = row * height as usize;
    let mut out = Vec::with_capacity(54 + pixels);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixels as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..height).rev() {
        let start = y as usize * frame.stride as usize;
        out.extend_from_slice(&frame.data[start..start + row]);
    }
    out
}
