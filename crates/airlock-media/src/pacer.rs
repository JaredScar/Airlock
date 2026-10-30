//! Output pacing. Wall time is allowed here: it decides when held media may air.
//! The timestamps on the media stay in media time.

use airlock_core::{Dur, Ts};
use std::io::{self, Write};

/// Anchors the playhead to the wall clock of the first packet.
#[derive(Debug, Clone)]
pub struct Pacer {
    delay: Dur,
    origin_wall_us: Option<u64>,
    origin_media: Ts,
}

impl Pacer {
    pub fn new(delay: Dur) -> Self {
        Pacer {
            delay,
            origin_wall_us: None,
            origin_media: Ts::ZERO,
        }
    }

    /// `wall_us` is a monotonic clock in microseconds, not media time.
    pub fn observe(&mut self, wall_us: u64, media: Ts) {
        if self.origin_wall_us.is_none() {
            self.origin_wall_us = Some(wall_us);
            self.origin_media = media;
        }
    }

    /// Media time that may air at this wall time. `None` until the first packet.
    pub fn playhead(&self, wall_us: u64) -> Option<Ts> {
        let origin = self.origin_wall_us?;
        let elapsed = Dur((wall_us.saturating_sub(origin)) as i64);
        Some(self.origin_media + elapsed - self.delay)
    }
}

/// A tiny FLV writer. Tags carry the playhead timestamp. The payload is copied through.
pub struct FlvMux<W> {
    writer: W,
    started: bool,
}

impl<W: Write> FlvMux<W> {
    pub fn new(writer: W) -> Self {
        FlvMux {
            writer,
            started: false,
        }
    }

    pub fn audio_tag(&mut self, pts: Ts, payload: &[u8]) -> io::Result<()> {
        self.tag(8, pts, payload)
    }

    pub fn video_tag(&mut self, pts: Ts, payload: &[u8]) -> io::Result<()> {
        self.tag(9, pts, payload)
    }

    fn tag(&mut self, kind: u8, pts: Ts, payload: &[u8]) -> io::Result<()> {
        if !self.started {
            self.writer.write_all(b"FLV")?;
            self.writer.write_all(&[1, 0x05])?;
            self.writer.write_all(&9u32.to_be_bytes())?;
            self.writer.write_all(&0u32.to_be_bytes())?;
            self.started = true;
        }
        let size = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        let ts = (pts.0.max(0) / 1_000) as u32;
        let mut header = [0u8; 11];
        header[0] = kind;
        header[1..4].copy_from_slice(&size.to_be_bytes()[1..]);
        header[4..7].copy_from_slice(&ts.to_be_bytes()[1..]);
        header[7] = (ts >> 24) as u8;
        self.writer.write_all(&header)?;
        self.writer.write_all(payload)?;
        let prev = 11 + size;
        self.writer.write_all(&prev.to_be_bytes())?;
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

/// Writes the FLV body to `sink`. The stream key is a keychain name, never the bytes.
pub fn push_flv(sink: &mut impl Write, body: &[u8], stream_key_ref: &str) -> io::Result<()> {
    let _ = stream_key_ref;
    sink.write_all(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn playhead_waits_out_the_delay() {
        let mut pacer = Pacer::new(Dur::from_millis(8_000));
        assert!(pacer.playhead(0).is_none());
        pacer.observe(1_000_000, Ts::ZERO);
        assert_eq!(pacer.playhead(1_000_000), Some(Ts::from_millis(-8_000)));
        assert_eq!(pacer.playhead(1_000_000 + 8_000_000), Some(Ts::ZERO));
        assert_eq!(
            pacer.playhead(1_000_000 + 9_000_000),
            Some(Ts::from_millis(1_000))
        );
    }

    #[test]
    fn flv_tags_use_the_playhead_and_the_key_is_not_written() {
        let mut mux = FlvMux::new(Vec::new());
        mux.audio_tag(Ts::from_millis(1_500), b"aac").unwrap();
        mux.video_tag(Ts::from_millis(1_500), &[0xAB, 0xCD])
            .unwrap();
        let bytes = mux.into_inner();
        assert_eq!(&bytes[..3], b"FLV");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            std::io::Read::read_to_end(&mut sock, &mut got).unwrap();
            got
        });
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        push_flv(&mut client, &bytes, "airlock/twitch").unwrap();
        drop(client);
        let got = server.join().unwrap();
        assert_eq!(got, bytes);
        assert!(!got.windows(14).any(|w| w == b"airlock/twitch"));
        let ts =
            u32::from_be_bytes([0, bytes[17], bytes[18], bytes[19]]) | (u32::from(bytes[20]) << 24);
        assert_eq!(ts, 1_500);
    }
}
