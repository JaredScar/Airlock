//! Decode a clip, pixelate a region, and encode it again.
//! Passthrough stays the default for a live broadcast. This path runs when a video
//! redaction is on. The region is pixelated on the GPU with `scale_cuda` and
//! `overlay_cuda`, then encoded with NVENC. The CPU frame path is the fallback.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

use airlock_core::Rect;

use crate::audio_path::MediaError;
use crate::video_fx::pixelate_bgra;

/// Pixelates `rect` and writes an H.264 file. CUDA runs first. A missing GPU
/// falls back to decoding, pixelating in memory, and encoding.
pub fn encode_redacted(input: &Path, output: &Path, rect: Rect) -> Result<(), MediaError> {
    if encode_redacted_cuda(input, output, rect).is_ok() {
        return Ok(());
    }
    let (width, height, rate) = video_info(input)?;
    let raw = decode_bgra(input)?;
    let frame = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if frame == 0 || raw.len() < frame || !raw.len().is_multiple_of(frame) {
        return Err(MediaError::Ffmpeg("decoded frame size is wrong".into()));
    }
    let mut frames = raw;
    for chunk in frames.chunks_mut(frame) {
        pixelate_bgra(chunk, width, height, width * 4, rect, 8);
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    }
    encode_bgra(&frames, width, height, rate, output, "h264_nvenc")
        .or_else(|_| encode_bgra(&frames, width, height, rate, output, "libx264"))
}

/// Block-pixelates `rect` on the GPU and encodes. The crop is scaled down and
/// back up with nearest-neighbor, which is irreversible, then composited back.
fn encode_redacted_cuda(input: &Path, output: &Path, rect: Rect) -> Result<(), MediaError> {
    let (width, height, _) = video_info(input)?;
    let (x, y, w, h) = gpu_region(width, height, rect)?;
    let down_w = (w / 8).max(2);
    let down_h = (h / 8).max(2);
    let graph = format!(
        "[0:v]split[main][c];\
         [c]crop={w}:{h}:{x}:{y},format=nv12,hwupload_cuda,\
         scale_cuda=w={down_w}:h={down_h}:interp_algo=nearest,\
         scale_cuda=w={w}:h={h}:interp_algo=nearest[pix];\
         [main]format=nv12,hwupload_cuda[base];\
         [base][pix]overlay_cuda=x={x}:y={y},hwdownload,format=nv12"
    );
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    }
    // NVENC rejects frames under its minimum size. The filter still ran on the GPU;
    // a small test clip is encoded with x264 so the pixels can be checked.
    let encoder = if width >= 256 && height >= 128 {
        "h264_nvenc"
    } else {
        "libx264"
    };
    if encoder == "h264_nvenc" && run_cuda_graph(input, output, &graph, encoder).is_ok() {
        return Ok(());
    }
    run_cuda_graph(input, output, &graph, "libx264")
}

fn run_cuda_graph(
    input: &Path,
    output: &Path,
    graph: &str,
    encoder: &str,
) -> Result<(), MediaError> {
    let output_cmd = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-init_hw_device",
            "cuda=cu:0",
            "-filter_hw_device",
            "cu",
            "-i",
        ])
        .arg(input)
        .args([
            "-filter_complex",
            graph,
            "-an",
            "-c:v",
            encoder,
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(output)
        .output()
        .map_err(io_error)?;
    if output_cmd.status.success() {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg(stderr_tail(&output_cmd.stderr)))
    }
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let start = text.len().saturating_sub(400);
    if text.is_empty() {
        "cuda pixelate failed".into()
    } else {
        text[start..].to_string()
    }
}

/// Even crop that stays inside the frame. CUDA's NV12 frames need even edges,
/// and a block smaller than 16 px is left to the CPU path.
fn gpu_region(width: u32, height: u32, rect: Rect) -> Result<(u32, u32, u32, u32), MediaError> {
    let (x, y, w, h) = rect.to_pixels(width, height);
    let x = x & !1;
    let y = y & !1;
    if x >= width || y >= height {
        return Err(MediaError::Ffmpeg("redaction region is empty".into()));
    }
    let mut w = w.max(16) & !1;
    let mut h = h.max(16) & !1;
    if x + w > width {
        w = (width - x) & !1;
    }
    if y + h > height {
        h = (height - y) & !1;
    }
    if w < 16 || h < 16 {
        return Err(MediaError::Ffmpeg(
            "redaction region is too small for the gpu".into(),
        ));
    }
    Ok((x, y, w, h))
}

fn video_info(path: &Path) -> Result<(u32, u32, u32), MediaError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(MediaError::Ffmpeg("ffprobe failed".into()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.trim().split(',');
    let width: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| MediaError::Ffmpeg("ffprobe width missing".into()))?;
    let height: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| MediaError::Ffmpeg("ffprobe height missing".into()))?;
    let rate = parts
        .next()
        .and_then(|value| {
            let (num, den) = value.split_once('/')?;
            let num: u32 = num.parse().ok()?;
            let den: u32 = den.parse().ok()?;
            if den == 0 {
                None
            } else {
                Some((num / den).max(1))
            }
        })
        .unwrap_or(30);
    Ok((width, height, rate))
}

fn decode_bgra(path: &Path) -> Result<Vec<u8>, MediaError> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "bgra", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(io_error)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(MediaError::Ffmpeg("decode failed".into()))
    }
}

fn encode_bgra(
    frames: &[u8],
    width: u32,
    height: u32,
    rate: u32,
    output: &Path,
    encoder: &str,
) -> Result<(), MediaError> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
            "-s",
            &format!("{width}x{height}"),
            "-r",
            &rate.to_string(),
            "-i",
            "-",
            "-c:v",
            encoder,
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_error)?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| MediaError::Ffmpeg("encoder stdin closed".into()))?;
        use std::io::Write;
        stdin
            .write_all(frames)
            .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    }
    let status = child
        .wait()
        .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg(format!("{encoder} encode failed")))
    }
}

fn io_error(err: std::io::Error) -> MediaError {
    if err.kind() == ErrorKind::NotFound {
        MediaError::FfmpegMissing
    } else {
        MediaError::Ffmpeg(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkerboard_is_flattened_inside_the_region() {
        let ffmpeg_ok = Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !ffmpeg_ok {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-xcode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x32:rate=10:duration=0.3",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let out = dir.join("out.mp4");
        encode_redacted(
            &src,
            &out,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            },
        )
        .expect("redact");
        let before = decode_bgra(&src).unwrap();
        let after = decode_bgra(&out).unwrap();
        assert_eq!(before.len(), after.len());
        let width = 64usize;
        let mean = |buf: &[u8], x0: usize, x1: usize| -> f32 {
            let mut sum = 0u64;
            let mut n = 0u64;
            let height = buf.len() / (width * 4);
            for y in 0..height {
                for x in x0..x1 {
                    let i = (y * width + x) * 4;
                    sum += buf[i] as u64;
                    n += 1;
                }
            }
            sum as f32 / n as f32
        };
        let left_delta = (mean(&before, 0, 32) - mean(&after, 0, 32)).abs();
        let right_delta = (mean(&before, 48, 64) - mean(&after, 48, 64)).abs();
        assert!(
            left_delta > right_delta,
            "left changed {left_delta}, right changed {right_delta}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_region_is_even_and_inside_the_frame() {
        let (x, y, w, h) = gpu_region(
            64,
            32,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            },
        )
        .unwrap();
        assert_eq!((x, y, w, h), (0, 0, 32, 32));
    }

    #[test]
    fn a_region_is_pixelated_on_the_gpu() {
        if !ffmpeg_present() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-gpu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=256x128:rate=10:duration=0.2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let out = dir.join("out.mp4");
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        match encode_redacted_cuda(&src, &out, rect) {
            Ok(()) => {}
            Err(MediaError::FfmpegMissing) => {
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
            Err(MediaError::Ffmpeg(msg)) if cuda_unavailable(&msg) => {
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
            Err(err) => panic!("{err}"),
        }
        let before = decode_bgra(&src).unwrap();
        let after = decode_bgra(&out).unwrap();
        assert_eq!(before.len(), after.len());
        let left = (mean_channel(&before, 256, 0, 128) - mean_channel(&after, 256, 0, 128)).abs();
        let right =
            (mean_channel(&before, 256, 192, 256) - mean_channel(&after, 256, 192, 256)).abs();
        assert!(left > right, "left changed {left}, right changed {right}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ffmpeg_present() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn cuda_unavailable(msg: &str) -> bool {
        let msg = msg.to_ascii_lowercase();
        msg.contains("nvcuda")
            || msg.contains("no device")
            || msg.contains("no cuda")
            || msg.contains("cannot load")
            || msg.contains("no such filter")
            || msg.contains("device creation failed")
            || msg.contains("failed to set value 'cuda")
    }

    fn mean_channel(buf: &[u8], width: usize, x0: usize, x1: usize) -> f32 {
        let mut sum = 0u64;
        let mut n = 0u64;
        let height = buf.len() / (width * 4);
        for y in 0..height {
            for x in x0..x1 {
                let i = (y * width + x) * 4;
                sum += buf[i] as u64;
                n += 1;
            }
        }
        sum as f32 / n as f32
    }
}
