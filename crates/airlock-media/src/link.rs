//! Localhost SRT ingest and the RTMP(S) publish step.
//! Both call the `ffmpeg` binary. The stream key is part of the publish URL and is
//! removed from any error text before it leaves this module.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::audio_path::MediaError;

/// Turns `rtmp://live.twitch.tv/app` plus a key into `rtmps://live.twitch.tv/app/<key>`.
/// A base that is already `rtmps://` is left on that scheme.
pub fn secure_publish_url(base: &str, key: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("rtmp://") {
        format!("rtmps://{rest}")
    } else {
        base.to_string()
    };
    if key.is_empty() {
        base
    } else {
        format!("{base}/{key}")
    }
}

/// Sends a media file to an RTMP or RTMPS URL. The URL is not included in the error.
pub fn publish_file(input: &Path, url: &str) -> Result<(), MediaError> {
    run_ffmpeg(
        &[
            "-y",
            "-loglevel",
            "error",
            "-i",
            &input.to_string_lossy(),
            "-c",
            "copy",
            "-f",
            "flv",
            url,
        ],
        url,
    )
}

/// Listens for one SRT sender and writes the audio to a wav file.
/// A hang-up after the bytes land is still success: the sender closing the socket
/// makes ffmpeg report a demux error on an otherwise complete file.
pub fn receive_srt_wav(port: u16, wav: &Path, seconds: u32) -> Result<(), MediaError> {
    let url = format!("srt://0.0.0.0:{port}?mode=listener");
    let result = run_ffmpeg(
        &[
            "-y",
            "-loglevel",
            "error",
            "-f",
            "mpegts",
            "-i",
            &url,
            "-t",
            &seconds.to_string(),
            "-c:a",
            "pcm_s16le",
            &wav.to_string_lossy(),
        ],
        "",
    );
    if result.is_ok() {
        return result;
    }
    let landed = std::fs::metadata(wav).map(|meta| meta.len()).unwrap_or(0) > 1_000;
    if landed { Ok(()) } else { result }
}

fn run_ffmpeg(args: &[&str], redact: &str) -> Result<(), MediaError> {
    let output = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| {
            if err.kind() == ErrorKind::NotFound {
                MediaError::FfmpegMissing
            } else {
                MediaError::Ffmpeg(err.to_string())
            }
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg(redacted_tail(&output.stderr, redact)))
    }
}

fn redacted_tail(stderr: &[u8], redact: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = if redact.is_empty() {
        text.to_string()
    } else {
        text.replace(redact, "[redacted]")
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
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    fn ffmpeg_present() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn platform_url_uses_rtmps_and_hides_the_key_on_failure() {
        let url = secure_publish_url("rtmp://live.twitch.tv/app", "test-key");
        assert_eq!(url, "rtmps://live.twitch.tv/app/test-key");
        assert_eq!(
            secure_publish_url("rtmps://live.example/app/", "test-key"),
            "rtmps://live.example/app/test-key"
        );
        let message = redacted_tail(b"connect failed rtmp://secret/live", "rtmp://secret/live");
        assert!(!message.contains("secret"));
        assert!(message.contains("[redacted]"));
    }

    #[test]
    fn local_rtmp_publish_writes_a_file() {
        if !ffmpeg_present() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-rtmp-{}", std::process::id()));
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
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let url = format!("rtmp://127.0.0.1:{port}/live/test-key");
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
        thread::sleep(Duration::from_millis(500));
        let published = publish_file(&src, &url);
        let _ = listen.wait();
        published.expect("publish");
        let len = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        assert!(len > 100, "listener file was {len} bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn srt_listener_receives_a_tone() {
        if !ffmpeg_present() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-srt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("in.wav");
        let port = {
            let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            socket.local_addr().unwrap().port()
        };
        let wav_thread = wav.clone();
        let listen = thread::spawn(move || receive_srt_wav(port, &wav_thread, 1));
        thread::sleep(Duration::from_millis(600));
        let send = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-c:a",
                "aac",
                "-f",
                "mpegts",
                &format!("srt://127.0.0.1:{port}?mode=caller"),
            ])
            .status();
        let received = listen.join().unwrap();
        assert!(send.unwrap().success(), "srt sender failed");
        received.expect("srt listener");
        let len = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
        assert!(len > 1000, "wav was {len} bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
