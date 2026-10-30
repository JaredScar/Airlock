//! File-side audio path: decode to studio PCM, hold it, redact, encode AAC.
//!
//! Studio audio is interleaved `f32`, 48 kHz, stereo. Decode and AAC encode call the
//! `ffmpeg` binary. Live RTMP ingest is still the Phase 1 spike; this path proves the
//! same PCM contract on a file, with no FFmpeg dev libraries linked in.

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use airlock_core::{DelayLine, Dur, Redaction, Ts};

use crate::audio_fx::{AudioChunk, apply_audio_redactions};

pub const STUDIO_RATE_HZ: u32 = 48_000;
pub const STUDIO_CHANNELS: usize = 2;
/// 20 ms. A chunk airs when its start reaches the playhead, so this is the granularity.
pub const STUDIO_CHUNK_FRAMES: usize = 960;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("audio needs a positive sample rate and at least one channel")]
    BadAudio,
    #[error("AAC encode expects 48 kHz stereo f32")]
    NotStudio,
    #[error("ffmpeg is not on PATH")]
    FfmpegMissing,
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
}

/// Owned interleaved PCM.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub start: Ts,
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    /// Resamples to 48 kHz and folds the layout to stereo. Mono is duplicated.
    /// Channels past the second are dropped.
    pub fn into_studio(self) -> Result<Pcm, MediaError> {
        if self.sample_rate == 0 || self.channels == 0 {
            return Err(MediaError::BadAudio);
        }
        if self.sample_rate == STUDIO_RATE_HZ && self.channels == STUDIO_CHANNELS {
            return Ok(self);
        }
        let left = resample_channel(channel(&self, 0), self.sample_rate, STUDIO_RATE_HZ);
        let right = if self.channels == 1 {
            left.clone()
        } else {
            resample_channel(channel(&self, 1), self.sample_rate, STUDIO_RATE_HZ)
        };
        let n = left.len().min(right.len());
        let mut samples = Vec::with_capacity(n * 2);
        for i in 0..n {
            samples.push(left[i]);
            samples.push(right[i]);
        }
        Ok(Pcm {
            start: self.start,
            sample_rate: STUDIO_RATE_HZ,
            channels: STUDIO_CHANNELS,
            samples,
        })
    }

    fn into_chunks(self, frames: usize) -> Vec<Pcm> {
        let frames = frames.max(1);
        let step = frames * self.channels.max(1);
        let mut offset = 0usize;
        self.samples
            .chunks(step)
            .map(|chunk| {
                let start =
                    self.start + Dur::from_secs_f64(offset as f64 / self.sample_rate.max(1) as f64);
                offset += chunk.len() / self.channels.max(1);
                Pcm {
                    start,
                    sample_rate: self.sample_rate,
                    channels: self.channels,
                    samples: chunk.to_vec(),
                }
            })
            .collect()
    }
}

/// Holds studio PCM until `live - delay`, then applies audio redactions.
#[derive(Debug)]
pub struct AudioHold {
    line: DelayLine<Pcm>,
}

impl AudioHold {
    pub fn new(delay: Dur) -> Self {
        AudioHold {
            line: DelayLine::new(delay),
        }
    }

    pub fn push(&mut self, pcm: Pcm) -> Result<(), MediaError> {
        let studio = pcm.into_studio()?;
        for chunk in studio.into_chunks(STUDIO_CHUNK_FRAMES) {
            let ts = chunk.start;
            self.line.push(ts, chunk);
        }
        Ok(())
    }

    /// PCM whose chunk start is at or before the playhead, with redactions applied.
    pub fn release<'a>(
        &mut self,
        live: Ts,
        redactions: impl IntoIterator<Item = &'a Redaction>,
    ) -> Vec<f32> {
        let redactions: Vec<&Redaction> = redactions.into_iter().collect();
        let playhead = self.line.playhead(live);
        let mut out = Vec::new();
        for (_, pcm) in self.line.drain_due(playhead) {
            let mut samples = pcm.samples;
            let mut chunk = AudioChunk {
                start: pcm.start,
                sample_rate: pcm.sample_rate,
                channels: pcm.channels,
                samples: &mut samples,
            };
            apply_audio_redactions(&mut chunk, redactions.iter().copied());
            out.extend_from_slice(&samples);
        }
        out
    }
}

/// Decodes any file ffmpeg can read into 48 kHz stereo f32, starting at media time 0.
pub fn decode_studio_pcm(input: &Path) -> Result<Pcm, MediaError> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args([
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(ffmpeg_failure(&output.stderr, output.status.code()));
    }
    Ok(Pcm {
        start: Ts::ZERO,
        sample_rate: STUDIO_RATE_HZ,
        channels: STUDIO_CHANNELS,
        samples: f32le_to_samples(&output.stdout),
    })
}

/// Encodes studio PCM to an AAC file in an MP4 container (`.m4a`).
pub fn encode_aac(pcm: &Pcm, output: &Path) -> Result<(), MediaError> {
    if pcm.sample_rate != STUDIO_RATE_HZ || pcm.channels != STUDIO_CHANNELS {
        return Err(MediaError::NotStudio);
    }
    if pcm.samples.is_empty() {
        return Err(MediaError::BadAudio);
    }
    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "f32le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
            "160k",
        ])
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| MediaError::Ffmpeg("could not write PCM to ffmpeg".into()))?;
        let mut bytes = Vec::with_capacity(pcm.samples.len() * 4);
        for sample in &pcm.samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        stdin.write_all(&bytes).map_err(io_error)?;
    }
    let finished = child.wait_with_output().map_err(io_error)?;
    if !finished.status.success() {
        return Err(ffmpeg_failure(&finished.stderr, finished.status.code()));
    }
    Ok(())
}

fn channel(pcm: &Pcm, index: usize) -> Vec<f32> {
    pcm.samples
        .iter()
        .skip(index)
        .step_by(pcm.channels)
        .copied()
        .collect()
}

fn resample_channel(input: Vec<f32>, from_hz: u32, to_hz: u32) -> Vec<f32> {
    if from_hz == to_hz || input.is_empty() {
        return input;
    }
    let ratio = from_hz as f64 / to_hz as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(input.len() - 1);
            let frac = (pos - i0 as f64) as f32;
            input[i0] * (1.0 - frac) + input[i1] * frac
        })
        .collect()
}

fn f32le_to_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(chunk);
            f32::from_le_bytes(buf)
        })
        .collect()
}

fn io_error(err: std::io::Error) -> MediaError {
    if err.kind() == ErrorKind::NotFound {
        MediaError::FfmpegMissing
    } else {
        MediaError::Ffmpeg(err.to_string())
    }
}

/// Stretches or squeezes interleaved samples so a delay change stays within ±5%.
/// `rate` below 1 makes the chunk longer. `rate` of 1 returns a copy.
pub fn time_stretch(samples: &[f32], rate: f64) -> Vec<f32> {
    if samples.is_empty() || (rate - 1.0).abs() < 1e-4 || rate <= 0.0 {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as f64) / rate).round().max(1.0) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * rate;
            let i0 = (pos.floor() as usize).min(samples.len() - 1);
            let i1 = (i0 + 1).min(samples.len() - 1);
            let frac = (pos - i0 as f64) as f32;
            samples[i0] * (1.0 - frac) + samples[i1] * frac
        })
        .collect()
}

fn ffmpeg_failure(stderr: &[u8], code: Option<i32>) -> MediaError {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    let text: String = text.chars().take(400).collect();
    if text.is_empty() {
        MediaError::Ffmpeg(format!("exit {}", code.unwrap_or(-1)))
    } else {
        MediaError::Ffmpeg(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::{Action, Category, Mode, Span, Status, Target};
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tone(ms: usize) -> Pcm {
        let frames = STUDIO_RATE_HZ as usize * ms / 1000;
        let mut samples = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let s =
                (2.0 * std::f32::consts::PI * 440.0 * n as f32 / STUDIO_RATE_HZ as f32).sin() * 0.5;
            samples.push(s);
            samples.push(s);
        }
        Pcm {
            start: Ts::ZERO,
            sample_rate: STUDIO_RATE_HZ,
            channels: STUDIO_CHANNELS,
            samples,
        }
    }

    fn mute(start_ms: i64, end_ms: i64) -> Redaction {
        Redaction {
            id: 1,
            category: Category::Profanity,
            mode: Mode::Apply,
            action: Action::Mute,
            target: Target::Audio,
            span: Span::from_millis(start_ms, end_ms),
            status: Status::Pending,
            confidence: 1.0,
            sources: vec![],
            reasons: vec![],
            evidence: vec![],
            missed_live: false,
        }
    }

    #[test]
    fn mono_44k_becomes_stereo_48k() {
        let frames = 22_050;
        let samples: Vec<f32> = (0..frames).map(|n| (n as f32).sin()).collect();
        let pcm = Pcm {
            start: Ts::from_millis(10),
            sample_rate: 44_100,
            channels: 1,
            samples,
        }
        .into_studio()
        .unwrap();
        assert_eq!(pcm.sample_rate, 48_000);
        assert_eq!(pcm.channels, 2);
        assert_eq!(pcm.frames(), 24_000);
        assert_eq!(pcm.start, Ts::from_millis(10));
        assert_eq!(pcm.samples[0], pcm.samples[1]);
    }

    #[test]
    fn hold_releases_one_chunk_when_the_playhead_arrives() {
        let mut hold = AudioHold::new(Dur::from_millis(300));
        hold.push(tone(500)).unwrap();
        assert!(hold.release(Ts::from_millis(200), []).is_empty());
        let first = hold.release(Ts::from_millis(300), []);
        assert_eq!(first.len(), STUDIO_CHUNK_FRAMES * STUDIO_CHANNELS);
        let rest = hold.release(Ts::from_millis(800), []);
        assert_eq!(
            first.len() + rest.len(),
            500 * STUDIO_RATE_HZ as usize / 1000 * STUDIO_CHANNELS
        );
        assert!(hold.release(Ts::from_millis(2_000), []).is_empty());
    }

    #[test]
    fn released_audio_is_muted_inside_the_span() {
        let mut hold = AudioHold::new(Dur::from_millis(100));
        hold.push(tone(800)).unwrap();
        let out = hold.release(Ts::from_millis(900), [&mute(200, 400)]);
        let at = |frame: usize| out[frame * 2].abs();
        // 440 Hz lands on a zero crossing at exact 100 ms boundaries; step off it.
        assert!(at(27) > 0.4, "outside the mute");
        assert!(at(300 * 48 + 27) < 1e-3, "inside the mute");
        assert!(at(600 * 48 + 27) > 0.4, "after the mute");
    }

    fn ffmpeg_on_path() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn write_wav_44k(path: &Path, ms: usize) {
        let rate = 44_100u32;
        let frames = rate as usize * ms / 1000;
        let mut pcm = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let s = ((2.0 * std::f64::consts::PI * 440.0 * n as f64 / rate as f64).sin() * 16_000.0)
                as i16;
            pcm.push(s);
            pcm.push(s);
        }
        let data_bytes = pcm.len() * 2;
        let mut bytes = Vec::with_capacity(44 + data_bytes);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_bytes as u32).to_le_bytes());
        for s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }

    fn window_rms(samples: &[f32]) -> Vec<f32> {
        let step = STUDIO_CHUNK_FRAMES * STUDIO_CHANNELS;
        samples
            .chunks(step)
            .map(|w| {
                let n = w.len() as f32;
                (w.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
            })
            .collect()
    }

    #[test]
    fn stretch_lengthens_a_slower_chunk() {
        let tone: Vec<f32> = (0..480).map(|i| (i as f32 * 0.05).sin()).collect();
        let slower = time_stretch(&tone, 0.95);
        let faster = time_stretch(&tone, 1.05);
        assert!(slower.len() > tone.len());
        assert!(faster.len() < tone.len());
        assert_eq!(time_stretch(&tone, 1.0).len(), tone.len());
    }

    #[test]
    fn decode_delay_mute_and_aac_roundtrip() {
        if !ffmpeg_on_path() {
            eprintln!("skipping aac roundtrip: ffmpeg not on PATH");
            return;
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let dir = std::env::temp_dir().join(format!("airlock-audio-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("in.wav");
        let m4a = dir.join("out.m4a");
        write_wav_44k(&wav, 800);

        let decoded = decode_studio_pcm(&wav).unwrap();
        assert_eq!(decoded.sample_rate, STUDIO_RATE_HZ);
        assert_eq!(decoded.channels, STUDIO_CHANNELS);
        let frames = decoded.frames();
        assert!(
            (frames as i64 - 38_400).abs() < 50,
            "expected ~800ms at 48 kHz, got {frames} frames"
        );

        let mut hold = AudioHold::new(Dur::from_millis(200));
        hold.push(decoded).unwrap();
        assert!(hold.release(Ts::ZERO, []).is_empty());
        let clean = hold.release(Ts::from_millis(1_200), [&mute(200, 400)]);
        encode_aac(
            &Pcm {
                start: Ts::ZERO,
                sample_rate: STUDIO_RATE_HZ,
                channels: STUDIO_CHANNELS,
                samples: clean,
            },
            &m4a,
        )
        .unwrap();
        let back = decode_studio_pcm(&m4a).unwrap();
        let env = window_rms(&back.samples);
        let loud = |r: f32| r > 0.05;
        let quiet = |r: f32| r < 0.02;
        let quiet_at = env.iter().position(|r| quiet(*r));
        assert!(quiet_at.is_some(), "mute did not survive AAC: {env:?}");
        let q = quiet_at.unwrap();
        assert!(env[..q].iter().any(|r| loud(*r)), "no tone before the mute");
        assert!(
            env[q..].iter().any(|r| loud(*r)),
            "no tone after the mute: {env:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
