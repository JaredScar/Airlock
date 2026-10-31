//! RTMP ingest. ffmpeg listens for one publisher at a time and copies the
//! stream. When that publisher hangs up, the caller listens again, so the
//! process does not have to restart. PTS values are rebased onto `Ts`.

use std::io::{ErrorKind, Read};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use airlock_core::Ts;

use crate::audio_path::{MediaError, Pcm, STUDIO_CHANNELS, STUDIO_RATE_HZ};
use crate::forward::drain_video_tags;
use crate::video_hold::VideoPacket;

/// Local listen port from the setup guide.
pub const INGEST_PORT: u16 = 1935;
/// Path key for the local hop. This is not a platform stream key.
pub const INGEST_KEY: &str = "airlock";

/// `rtmp://127.0.0.1:{port}/live/airlock`. OBS and Streamlabs put the server
/// and this key in separate fields; together they are this URL.
pub fn ingest_url(port: u16) -> String {
    format!("rtmp://127.0.0.1:{port}/live/{INGEST_KEY}")
}

/// Turns publisher PTS (microseconds) into media time.
/// The first packet of a connection continues just after the previous
/// connection. A timestamp that moves backwards is treated the same way.
#[derive(Debug, Default)]
pub struct PtsClock {
    origin: Option<i64>,
    carry: i64,
    last: i64,
}

impl PtsClock {
    pub fn stamp(&mut self, pts_us: i64) -> Ts {
        if let Some(origin) = self.origin {
            let delta = pts_us.saturating_sub(origin);
            if delta < 0 {
                self.disconnected();
                return self.stamp(pts_us);
            }
            self.last = self.carry.saturating_add(delta);
        } else {
            self.origin = Some(pts_us);
            self.last = self.carry;
        }
        Ts(self.last)
    }

    /// The publisher hung up. The next packet starts a new origin.
    pub fn disconnected(&mut self) {
        if self.origin.is_some() {
            self.carry = self.last.saturating_add(1);
            self.origin = None;
        }
    }

    /// Media time where the next connection begins.
    pub fn carry(&self) -> i64 {
        self.carry
    }
}

/// Copies one publisher into `output`. `on_connection` is called with `true`
/// once bytes arrive and `false` when that publisher hangs up.
/// A demux error after the file has media is still success.
pub fn accept_publisher(
    port: u16,
    output: &Path,
    timeout_secs: u32,
    mut on_connection: impl FnMut(bool),
) -> Result<(), MediaError> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    }
    let url = ingest_url(port);
    let timeout = timeout_secs.max(1).to_string();
    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-rtmp_listen",
            "1",
            "-timeout",
            &timeout,
            "-i",
            &url,
            "-c",
            "copy",
            "-avoid_negative_ts",
            "make_zero",
        ])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_error)?;
    let mut connected = false;
    loop {
        if !connected && file_len(output) > 100 {
            connected = true;
            on_connection(true);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(err) => return Err(MediaError::Ffmpeg(err.to_string())),
        }
    }
    if connected {
        on_connection(false);
    }
    if file_len(output) > 100 {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg("ingest failed".into()))
    }
}

/// Accepts publishers until `stop` is set. Each connection is one file from
/// `output_for`. A timeout with nobody connected listens again.
pub fn serve(
    port: u16,
    stop: &AtomicBool,
    mut output_for: impl FnMut(u32) -> std::path::PathBuf,
    mut on_connection: impl FnMut(bool),
) -> Result<(), MediaError> {
    let mut n = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let path = output_for(n);
        match accept_publisher(port, &path, 2, &mut on_connection) {
            Ok(()) => n = n.saturating_add(1),
            Err(MediaError::FfmpegMissing) => return Err(MediaError::FfmpegMissing),
            Err(_) if stop.load(Ordering::SeqCst) => break,
            Err(_) => continue,
        }
    }
    Ok(())
}

/// Listens until `stop`, copying each publisher to an flv and decoding its audio
/// to 48 kHz stereo. PCM timestamps continue across a reconnect.
pub fn serve_capture(
    port: u16,
    directory: &Path,
    stop: &AtomicBool,
    mut on_connection: impl FnMut(bool),
    mut on_video: impl FnMut(VideoPacket),
    mut on_pcm: impl FnMut(Pcm),
) -> Result<(), MediaError> {
    std::fs::create_dir_all(directory).map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    let mut clock = PtsClock::default();
    let mut n = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let path = directory.join(format!("ingest-{n}.flv"));
        match capture_one(
            port,
            &path,
            stop,
            &mut clock,
            &mut on_connection,
            &mut on_video,
            &mut on_pcm,
        ) {
            Ok(()) => n = n.saturating_add(1),
            Err(MediaError::FfmpegMissing) => return Err(MediaError::FfmpegMissing),
            Err(_) if stop.load(Ordering::SeqCst) => break,
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
    Ok(())
}

fn capture_one(
    port: u16,
    flv: &Path,
    stop: &AtomicBool,
    clock: &mut PtsClock,
    on_connection: &mut impl FnMut(bool),
    on_video: &mut impl FnMut(VideoPacket),
    on_pcm: &mut impl FnMut(Pcm),
) -> Result<(), MediaError> {
    let url = ingest_url(port);
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    let video_port = listener
        .local_addr()
        .map_err(|err| MediaError::Ffmpeg(err.to_string()))?
        .port();
    let video_url = format!("tcp://127.0.0.1:{video_port}");
    let origin_us = clock.carry();
    let (video_tx, video_rx) = mpsc::channel();
    let mut child = Command::new("ffmpeg")
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
            "-map",
            "0:a:0",
            "-ac",
            "2",
            "-ar",
            "48000",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "pipe:1",
            "-map",
            "0",
            "-c",
            "copy",
            "-flush_packets",
            "1",
            "-f",
            "flv",
            &video_url,
            "-map",
            "0",
            "-c",
            "copy",
            "-avoid_negative_ts",
            "make_zero",
        ])
        .arg(flv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io_error)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| MediaError::Ffmpeg("ingest stdout closed".into()))?;
    thread::scope(|scope| {
        scope.spawn(|| {
            loop {
                if stop.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    break;
                }
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            let _ = child.wait();
        });
        let video_done = Arc::new(AtomicBool::new(false));
        let video_stop = Arc::clone(&video_done);
        scope.spawn(move || {
            read_video_stream(listener, origin_us, video_tx, stop, &video_stop);
        });
        let read = read_pcm(
            &mut stdout,
            stop,
            clock,
            &video_rx,
            on_connection,
            on_video,
            on_pcm,
        );
        video_done.store(true, Ordering::SeqCst);
        read
    })?;
    if file_len(flv) > 100 {
        clock.disconnected();
        Ok(())
    } else if stop.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(MediaError::Ffmpeg("ingest failed".into()))
    }
}

fn read_video_stream(
    listener: TcpListener,
    origin_us: i64,
    tx: mpsc::Sender<VideoPacket>,
    stop: &AtomicBool,
    done: &AtomicBool,
) {
    let mut stream = None;
    while stream.is_none() && !stop.load(Ordering::SeqCst) && !done.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((sock, _)) => stream = Some(sock),
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(_) => return,
        }
    }
    let Some(mut stream) = stream else {
        return;
    };
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let mut packets = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                drain_video_tags(&mut buf, origin_us, &mut packets);
                for packet in packets.drain(..) {
                    if tx.send(packet).is_err() {
                        return;
                    }
                }
            }
            Err(err)
                if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut =>
            {
                if done.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn read_pcm(
    stdout: &mut impl Read,
    stop: &AtomicBool,
    clock: &mut PtsClock,
    video_rx: &Receiver<VideoPacket>,
    on_connection: &mut impl FnMut(bool),
    on_video: &mut impl FnMut(VideoPacket),
    on_pcm: &mut impl FnMut(Pcm),
) -> Result<(), MediaError> {
    let chunk_bytes = 960 * STUDIO_CHANNELS * 4;
    let mut pending = Vec::new();
    let mut tmp = [0u8; 8192];
    let mut connected = false;
    let mut frames = 0i64;
    loop {
        if stop.load(Ordering::SeqCst) && pending.len() < chunk_bytes {
            break;
        }
        let n = stdout
            .read(&mut tmp)
            .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
        if n == 0 {
            break;
        }
        while let Ok(packet) = video_rx.try_recv() {
            on_video(packet);
        }
        if !connected {
            connected = true;
            on_connection(true);
        }
        pending.extend_from_slice(&tmp[..n]);
        while pending.len() >= chunk_bytes {
            let bytes: Vec<u8> = pending.drain(..chunk_bytes).collect();
            emit_pcm(&bytes, &mut frames, clock, on_pcm);
        }
    }
    let tail = pending.len() - pending.len() % (STUDIO_CHANNELS * 4);
    if tail >= STUDIO_CHANNELS * 4 {
        emit_pcm(&pending[..tail], &mut frames, clock, on_pcm);
    }
    while let Ok(packet) = video_rx.try_recv() {
        on_video(packet);
    }
    if connected {
        on_connection(false);
    }
    if frames == 0 {
        Err(MediaError::Ffmpeg("ingest failed".into()))
    } else {
        Ok(())
    }
}

fn emit_pcm(bytes: &[u8], frames: &mut i64, clock: &mut PtsClock, on_pcm: &mut impl FnMut(Pcm)) {
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let start_us = frames.saturating_mul(1_000_000) / i64::from(STUDIO_RATE_HZ);
    let start = clock.stamp(start_us);
    *frames = frames.saturating_add((samples.len() / STUDIO_CHANNELS) as i64);
    on_pcm(Pcm {
        start,
        sample_rate: STUDIO_RATE_HZ,
        channels: STUDIO_CHANNELS,
        samples,
    });
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn io_error(err: std::io::Error) -> MediaError {
    if err.kind() == std::io::ErrorKind::NotFound {
        MediaError::FfmpegMissing
    } else {
        MediaError::Ffmpeg(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    fn ffmpeg_present() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn a_new_connection_continues_media_time() {
        let mut clock = PtsClock::default();
        assert_eq!(clock.stamp(1_000_000), Ts(0));
        assert_eq!(clock.stamp(1_500_000), Ts(500_000));
        clock.disconnected();
        assert_eq!(clock.stamp(0), Ts(500_001));
        assert_eq!(clock.stamp(200_000), Ts(700_001));
        let mut restarted = PtsClock::default();
        assert_eq!(restarted.stamp(5_000_000), Ts(0));
        assert_eq!(restarted.stamp(5_400_000), Ts(400_000));
        assert_eq!(restarted.stamp(100), Ts(400_001));
    }

    #[test]
    fn one_publisher_is_copied_and_the_next_is_accepted_after_hangup() {
        if !ffmpeg_present() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-ingest-{}", std::process::id()));
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
        let port = free_port();
        let url = ingest_url(port);
        let states = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let states_thread = Arc::clone(&states);
        let stop_thread = Arc::clone(&stop);
        let dir_thread = dir.clone();
        let server = thread::spawn(move || {
            serve(
                port,
                &stop_thread,
                |n| dir_thread.join(format!("conn-{n}.flv")),
                |up| states_thread.lock().unwrap().push(up),
            )
        });
        thread::sleep(Duration::from_millis(700));
        let mut held = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-c:a",
                "aac",
                "-f",
                "flv",
                &url,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(800));
        assert!(
            held.try_wait().unwrap().is_none(),
            "the holding publisher exited before the overlap check"
        );
        let second = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&src)
            .args(["-c", "copy", "-f", "flv", &url])
            .output()
            .unwrap();
        assert!(
            !second.status.success(),
            "a second publisher was accepted: {}",
            String::from_utf8_lossy(&second.stderr)
        );
        let _ = held.wait();
        thread::sleep(Duration::from_millis(700));
        crate::link::publish_file(&src, &url).expect("reconnect");
        thread::sleep(Duration::from_millis(400));
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().expect("serve");
        let first = std::fs::metadata(dir.join("conn-0.flv"))
            .map(|m| m.len())
            .unwrap_or(0);
        let next = std::fs::metadata(dir.join("conn-1.flv"))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(first > 100, "first connection was {first} bytes");
        assert!(next > 100, "reconnect was {next} bytes");
        let seen = states.lock().unwrap().clone();
        assert!(
            seen.windows(2).any(|pair| pair == [true, false]),
            "connection states were {seen:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_publisher_becomes_studio_audio_and_a_copy() {
        if !ffmpeg_present() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("airlock-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let port = free_port();
        let url = ingest_url(port);
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(0usize));
        let states = Arc::new(Mutex::new(Vec::new()));
        let stop_thread = Arc::clone(&stop);
        let samples_thread = Arc::clone(&samples);
        let states_thread = Arc::clone(&states);
        let dir_thread = dir.clone();
        let server = thread::spawn(move || {
            serve_capture(
                port,
                &dir_thread,
                &stop_thread,
                |up| states_thread.lock().unwrap().push(up),
                |_| {},
                |pcm| {
                    assert_eq!(pcm.sample_rate, 48_000);
                    assert_eq!(pcm.channels, 2);
                    *samples_thread.lock().unwrap() += pcm.samples.len();
                },
            )
        });
        thread::sleep(Duration::from_millis(700));
        let published = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-c:a",
                "aac",
                "-f",
                "flv",
                &url,
            ])
            .status()
            .unwrap();
        assert!(published.success(), "publisher failed");
        thread::sleep(Duration::from_millis(400));
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().expect("capture");
        let got = *samples.lock().unwrap();
        assert!(got > 40_000, "decoded {got} samples");
        let copy = std::fs::metadata(dir.join("ingest-0.flv"))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(copy > 100, "ingest copy was {copy} bytes");
        let seen = states.lock().unwrap().clone();
        assert!(
            seen.windows(2).any(|pair| pair == [true, false]),
            "connection states were {seen:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
