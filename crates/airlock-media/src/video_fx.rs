//! In-memory video redaction. The file encode path pixelates on the GPU
//! (`encode_redacted`). These functions stay for a frame that is already in memory,
//! and for machines where CUDA is missing.

use airlock_core::Rect;

/// Pixelates `rect` in a BGRA8 frame with square blocks of `block` pixels.
/// Pixelation with a large block is irreversible, unlike a light Gaussian blur, which
/// can sometimes be partially undone. Prefer it for secrets.
pub fn pixelate_bgra(
    data: &mut [u8],
    width: u32,
    height: u32,
    stride: u32,
    rect: Rect,
    block: u32,
) {
    let (x0, y0, w, h) = rect.to_pixels(width, height);
    let block = block.max(2);
    let stride = stride as usize;
    let mut by = y0;
    while by < y0 + h {
        let bh = block.min(y0 + h - by);
        let mut bx = x0;
        while bx < x0 + w {
            let bw = block.min(x0 + w - bx);
            let mut sum = [0u32; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = y as usize * stride + x as usize * 4;
                    for c in 0..4 {
                        sum[c] += data[i + c] as u32;
                    }
                }
            }
            let n = bw * bh;
            let avg = sum.map(|s| (s / n) as u8);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = y as usize * stride + x as usize * 4;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
            bx += bw;
        }
        by += bh;
    }
}

/// Fills `rect` with opaque black in a BGRA8 frame.
pub fn blackout_bgra(data: &mut [u8], width: u32, height: u32, stride: u32, rect: Rect) {
    let (x0, y0, w, h) = rect.to_pixels(width, height);
    for y in y0..y0 + h {
        let row = y as usize * stride as usize;
        for x in x0..x0 + w {
            let i = row + x as usize * 4;
            data[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixelate_averages_blocks_inside_rect_only() {
        let (w, h) = (8u32, 8u32);
        let mut data: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 3) as u8, 0, 0, 255])
            .collect();
        let before = data.clone();
        pixelate_bgra(
            &mut data,
            w,
            h,
            w * 4,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 0.5,
            },
            4,
        );
        // Top-left 4x4 block is uniform now.
        let px = |d: &[u8], x: u32, y: u32| d[(y * w * 4 + x * 4) as usize];
        assert_eq!(px(&data, 0, 0), px(&data, 3, 3));
        // Outside the rect is untouched.
        assert_eq!(px(&data, 7, 7), px(&before, 7, 7));
    }

    #[test]
    fn blackout() {
        let mut data = vec![200u8; 4 * 4 * 4];
        blackout_bgra(&mut data, 4, 4, 16, Rect::FULL);
        assert!(data.chunks(4).all(|p| p == [0, 0, 0, 255]));
    }
}
