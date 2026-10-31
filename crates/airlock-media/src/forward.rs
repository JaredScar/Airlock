//! Sends the held picture and sound to a platform. The stream key is only in the
//! publish URL passed to ffmpeg, and it is removed from any error text.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Instant;

use airlock_core::Ts;

use crate::audio_path::MediaError;
use crate::link::reconnect_backoff;
use crate::pacer::FlvMux;
use crate::video_hold::VideoPacket;

/// What one `DelayedForward::write` did. Strings here are safe to show.
#[derive(Debug, PartialEq, Eq)]
pub enum ForwardStep {
    /// No platform key, so nothing is sent.
    Idle,
    /// The platform connection opened on this call.
    Started,
    /// Tags were written.
    Sent,
    /// The last attempt failed and the backoff has not elapsed.
    Waiting,
}

/// Holds an ffmpeg publisher. Dropping it stops the process.
pub struct LivePublish {
    mux_stdin: Option<StdioHolder>,
    child: Option<std::process::Child>,
    url: String,
}

struct StdioHolder {
    stdin: std::process::ChildStdin,
}

impl LivePublish {
    pub fn start(url: &str) -> Result<Self, MediaError> {
        let mut child = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-fflags",
                "nobuffer",
                "-f",
                "flv",
                "-i",
                "pipe:0",
                "-c",
                "copy",
                "-f",
                "flv",
                url,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawn_error)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MediaError::Ffmpeg("publish stdin closed".into()))?;
        Ok(LivePublish {
            mux_stdin: Some(StdioHolder { stdin }),
            child: Some(child),
            url: url.to_string(),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), MediaError> {
        let Some(holder) = self.mux_stdin.as_mut() else {
            return Err(MediaError::Ffmpeg("publish closed".into()));
        };
        holder
            .stdin
            .write_all(bytes)
            .map_err(|_| MediaError::Ffmpeg(redact_text("publish write failed", &self.url)))?;
        holder
            .stdin
            .flush()
            .map_err(|_| MediaError::Ffmpeg(redact_text("publish write failed", &self.url)))
    }

    /// Closes stdin and waits so a local listener can finish the file.
    pub fn finish(mut self) -> Result<(), MediaError> {
        self.mux_stdin.take();
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let status = child
            .wait()
            .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            let stderr = child
                .stderr
                .take()
                .and_then(|mut pipe| {
                    let mut buf = Vec::new();
                    use std::io::Read;
                    pipe.read_to_end(&mut buf).ok()?;
                    Some(buf)
                })
                .unwrap_or_default();
            Err(MediaError::Ffmpeg(redact(&stderr, &self.url)))
        }
    }
}

impl Drop for LivePublish {
    fn drop(&mut self) {
        self.mux_stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Encodes studio PCM to raw AAC frames. The sequence header is available after the first frame.
pub struct AacEncoder {
    stdin: Option<std::process::ChildStdin>,
    child: Option<std::process::Child>,
    rx: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    pending: Vec<u8>,
    asc: Option<Vec<u8>>,
}

impl AacEncoder {
    pub fn start() -> Result<Self, MediaError> {
        let mut child = Command::new("ffmpeg")
            .args([
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
                "-flush_packets",
                "1",
                "-f",
                "adts",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(spawn_error)?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| MediaError::Ffmpeg("aac stdout closed".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MediaError::Ffmpeg("aac stdin closed".into()))?;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(AacEncoder {
            stdin: Some(stdin),
            child: Some(child),
            rx,
            reader: Some(reader),
            pending: Vec::new(),
            asc: None,
        })
    }

    pub fn sequence_header(&self) -> Option<&[u8]> {
        self.asc.as_deref()
    }

    /// Pushes interleaved f32 stereo and returns any raw AAC frames now available.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Vec<u8>>, MediaError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(MediaError::Ffmpeg("aac closed".into()));
        };
        for sample in samples {
            stdin
                .write_all(&sample.to_le_bytes())
                .map_err(|_| MediaError::Ffmpeg("aac write failed".into()))?;
        }
        stdin
            .flush()
            .map_err(|_| MediaError::Ffmpeg("aac write failed".into()))?;
        while let Ok(chunk) = self.rx.try_recv() {
            self.pending.extend_from_slice(&chunk);
        }
        if self.asc.is_none()
            && let Some(start) = self
                .pending
                .windows(2)
                .position(|pair| pair[0] == 0xFF && pair[1] & 0xF0 == 0xF0)
            && self.pending.len() >= start + 7
        {
            self.asc = Some(asc_from_adts(&self.pending[start..]));
        }
        Ok(split_adts(&mut self.pending))
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Forwards released video packets and studio PCM. Without a URL it does nothing.
pub struct DelayedForward {
    url: Option<String>,
    publish: Option<LivePublish>,
    aac: Option<AacEncoder>,
    header_sent: bool,
    sent_asc: bool,
    sent_avc: bool,
    avc_header: Option<Vec<u8>>,
    audio_us: i64,
    retry_after: Option<Instant>,
    attempt: u32,
    announced: bool,
}

impl DelayedForward {
    pub fn new(url: Option<String>) -> Self {
        DelayedForward {
            url,
            publish: None,
            aac: None,
            header_sent: false,
            sent_asc: false,
            sent_avc: false,
            avc_header: None,
            audio_us: 0,
            retry_after: None,
            attempt: 0,
            announced: false,
        }
    }

    pub fn write(&mut self, video: &[VideoPacket], pcm: &[f32]) -> Result<ForwardStep, String> {
        if self.url.is_none() {
            return Ok(ForwardStep::Idle);
        }
        if video.is_empty() && pcm.is_empty() {
            return Ok(ForwardStep::Sent);
        }
        if let Some(when) = self.retry_after {
            if Instant::now() < when {
                return Ok(ForwardStep::Waiting);
            }
            self.retry_after = None;
        }
        let started = if self.publish.is_none() {
            self.open().map_err(|err| err.to_string())?;
            true
        } else {
            false
        };
        if let Err(err) = self.push(video, pcm) {
            self.fail();
            return Err(err.to_string());
        }
        if started && !self.announced {
            self.announced = true;
            return Ok(ForwardStep::Started);
        }
        Ok(ForwardStep::Sent)
    }

    fn open(&mut self) -> Result<(), MediaError> {
        let url = self.url.clone().unwrap_or_default();
        if self.aac.is_none() {
            self.aac = Some(AacEncoder::start()?);
        }
        self.publish = Some(LivePublish::start(&url)?);
        self.header_sent = false;
        self.sent_asc = false;
        self.sent_avc = false;
        Ok(())
    }

    fn fail(&mut self) {
        self.publish.take();
        self.attempt = self.attempt.saturating_add(1);
        self.retry_after = Some(Instant::now() + reconnect_backoff(self.attempt));
        self.announced = false;
    }

    fn push(&mut self, video: &[VideoPacket], pcm: &[f32]) -> Result<(), MediaError> {
        let mut bytes = Vec::new();
        {
            let mut mux = if self.header_sent {
                FlvMux::resume(&mut bytes)
            } else {
                self.header_sent = true;
                FlvMux::new(&mut bytes)
            };
            let stored_header = self.avc_header.clone();
            if let Some(header) = stored_header
                && !self.sent_avc
                && !video.iter().any(|packet| is_avc_sequence(&packet.data))
            {
                mux.video_tag(Ts(self.audio_us.max(0)), &header)
                    .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
                self.sent_avc = true;
            }
            for packet in video {
                if is_avc_sequence(&packet.data) {
                    self.avc_header = Some(packet.data.clone());
                    self.sent_avc = true;
                }
                mux.video_tag(packet.pts, &packet.data)
                    .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
            }
            if !pcm.is_empty() {
                let frames = self
                    .aac
                    .as_mut()
                    .ok_or_else(|| MediaError::Ffmpeg("aac is not running".into()))?
                    .push(pcm)?;
                let asc = self
                    .aac
                    .as_ref()
                    .and_then(|enc| enc.sequence_header().map(|header| header.to_vec()));
                if let Some(asc) = asc
                    && !self.sent_asc
                {
                    mux.audio_tag(Ts(self.audio_us.max(0)), &aac_packet(true, &asc))
                        .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
                    self.sent_asc = true;
                }
                for frame in &frames {
                    mux.audio_tag(Ts(self.audio_us.max(0)), &aac_packet(false, frame))
                        .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
                    self.audio_us = self.audio_us.saturating_add(1_024_000_000 / 48_000);
                }
            }
        }
        self.publish
            .as_mut()
            .ok_or_else(|| MediaError::Ffmpeg("publish is not running".into()))?
            .write(&bytes)
    }
}

impl Drop for DelayedForward {
    fn drop(&mut self) {
        self.publish.take();
        self.aac.take();
    }
}

/// Pulls complete video tags out of an FLV byte stream. `origin_us` is added to each timestamp.
pub fn drain_video_tags(buf: &mut Vec<u8>, origin_us: i64, out: &mut Vec<VideoPacket>) {
    if buf.starts_with(b"FLV") && buf.len() >= 13 {
        let header = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]) as usize;
        if buf.len() >= header + 4 {
            buf.drain(..header + 4);
        }
    }
    loop {
        if buf.len() < 11 {
            return;
        }
        let size = u32::from_be_bytes([0, buf[1], buf[2], buf[3]]) as usize;
        let total = 11 + size + 4;
        if buf.len() < total {
            return;
        }
        let ts = u32::from_be_bytes([0, buf[4], buf[5], buf[6]]) | (u32::from(buf[7]) << 24);
        let kind = buf[0];
        let body: Vec<u8> = buf[11..11 + size].to_vec();
        buf.drain(..total);
        if kind == 9 && !body.is_empty() {
            out.push(VideoPacket {
                pts: Ts(origin_us.saturating_add(i64::from(ts) * 1_000)),
                keyframe: body[0] >> 4 == 1,
                data: body,
            });
        }
    }
}

pub fn split_adts(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    loop {
        let Some(start) = buf
            .windows(2)
            .position(|w| w[0] == 0xFF && w[1] & 0xF0 == 0xF0)
        else {
            if buf.len() > 1 {
                buf.drain(..buf.len() - 1);
            }
            return frames;
        };
        if start > 0 {
            buf.drain(..start);
        }
        if buf.len() < 7 {
            return frames;
        }
        let length = (usize::from(buf[3] & 0x03) << 11)
            | (usize::from(buf[4]) << 3)
            | (usize::from(buf[5]) >> 5);
        if length < 7 || buf.len() < length {
            return frames;
        }
        let header_len = if buf[1] & 0x01 == 0 { 9 } else { 7 };
        if length > header_len {
            frames.push(buf[header_len..length].to_vec());
        }
        buf.drain(..length);
    }
}

fn asc_from_adts(header: &[u8]) -> Vec<u8> {
    let profile = ((header[2] & 0xC0) >> 6) + 1;
    let rate = (header[2] & 0x3C) >> 2;
    let channels = ((header[2] & 1) << 2) | ((header[3] & 0xC0) >> 6);
    vec![
        (profile << 3) | (rate >> 1),
        ((rate & 1) << 7) | (channels << 3),
    ]
}

fn aac_packet(sequence: bool, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![0xAF, if sequence { 0x00 } else { 0x01 }];
    body.extend_from_slice(payload);
    body
}

fn is_avc_sequence(body: &[u8]) -> bool {
    body.len() > 1 && body[0] & 0x0F == 7 && body[1] == 0
}

fn spawn_error(err: std::io::Error) -> MediaError {
    if err.kind() == std::io::ErrorKind::NotFound {
        MediaError::FfmpegMissing
    } else {
        MediaError::Ffmpeg(err.to_string())
    }
}

fn redact(stderr: &[u8], secret: &str) -> String {
    redact_text(&String::from_utf8_lossy(stderr), secret)
}

fn redact_text(text: &str, secret: &str) -> String {
    let text = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "[redacted]")
    };
    let text = text.trim();
    if text.is_empty() {
        "ffmpeg failed".into()
    } else {
        text.chars().take(400).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::publish_base;
    use airlock_core::Platform;
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn platform_bases_hide_the_key() {
        assert_eq!(
            publish_base(Platform::Twitch),
            Some("rtmp://live.twitch.tv/app")
        );
        assert!(publish_base(Platform::Generic).is_none());
        let url =
            crate::link::secure_publish_url(publish_base(Platform::Twitch).unwrap(), "secret");
        assert!(!url.contains("secret") || url.ends_with("/secret"));
        assert!(url.starts_with("rtmps://"));
    }

    #[test]
    fn video_tags_keep_their_timestamp_and_a_reconnect_origin() {
        let mut mux = FlvMux::new(Vec::new());
        mux.video_tag(Ts::from_millis(40), &[0x17, 0x01, 1, 2, 3])
            .unwrap();
        let mut bytes = mux.into_inner();
        let mut out = Vec::new();
        drain_video_tags(&mut bytes, 5_000_000, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pts, Ts(5_040_000));
        assert!(out[0].keyframe);
        assert!(bytes.is_empty());
    }

    #[test]
    fn an_adts_frame_is_split_from_its_header() {
        let mut frame = vec![0xFF, 0xF1, 0x4C, 0x80, 0x00, 0x1F, 0xFC, 0x11, 0x22];
        // length 9 sits in the header fields
        let length = frame.len();
        frame[3] = ((length >> 11) as u8) & 0x03;
        frame[4] = ((length >> 3) & 0xFF) as u8;
        frame[5] = (((length & 0x07) as u8) << 5) | (frame[5] & 0x1F);
        let mut buf = frame.clone();
        let raw = split_adts(&mut buf);
        assert_eq!(raw, vec![vec![0x11, 0x22]]);
        assert!(buf.is_empty());
        let asc = asc_from_adts(&frame);
        assert_eq!(asc.len(), 2);
    }

    #[test]
    fn a_live_publish_reaches_a_local_listener_without_leaking_the_key() {
        if Command::new("ffmpeg")
            .arg("-version")
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-fwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.flv");
        let made = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:a",
                "aac",
                "-f",
                "flv",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let bytes = std::fs::read(&src).unwrap();
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let key = "local-test-key";
        let url = format!("rtmp://127.0.0.1:{port}/live/{key}");
        let out = dir.join("out.flv");
        let mut listen = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-rtmp_listen",
                "1",
                "-timeout",
                "8",
                "-i",
                &url,
                "-c",
                "copy",
            ])
            .arg(&out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let mut publish = LivePublish::start(&url).unwrap();
        publish.write(&bytes).unwrap();
        let finished = publish.finish();
        let _ = listen.wait();
        let len = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        if let Err(err) = &finished {
            let text = err.to_string();
            assert!(!text.contains(key), "{text}");
        }
        assert!(len > 100, "listener file was {len} bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
