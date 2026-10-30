//! Passthrough video: encoded packets wait on the same media clock as the audio.
//! Nothing is decoded, so keyframes stay valid.

use airlock_core::{DelayLine, Dur, Ts};

/// One encoded video packet. `pts` is media time. The bytes are not inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacket {
    pub pts: Ts,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct VideoHold {
    line: DelayLine<VideoPacket>,
}

impl VideoHold {
    pub fn new(delay: Dur) -> Self {
        VideoHold {
            line: DelayLine::new(delay),
        }
    }

    pub fn push(&mut self, packet: VideoPacket) {
        let pts = packet.pts;
        self.line.push(pts, packet);
    }

    /// Packets whose timestamp has reached the playhead, in order.
    pub fn release(&mut self, live: Ts) -> Vec<VideoPacket> {
        let playhead = self.line.playhead(live);
        self.line
            .drain_due(playhead)
            .into_iter()
            .map(|(_, packet)| packet)
            .collect()
    }
}

/// One audio chunk timestamped the same way as a video packet.
#[derive(Debug)]
pub struct AudioStamp {
    line: DelayLine<Ts>,
}

impl AudioStamp {
    pub fn new(delay: Dur) -> Self {
        AudioStamp {
            line: DelayLine::new(delay),
        }
    }

    pub fn push(&mut self, ts: Ts) {
        self.line.push(ts, ts);
    }

    pub fn release(&mut self, live: Ts) -> Vec<Ts> {
        let playhead = self.line.playhead(live);
        self.line
            .drain_due(playhead)
            .into_iter()
            .map(|(_, ts)| ts)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    const FRAME: i64 = 1_000_000 / 30;

    #[test]
    fn an_hour_of_packets_stays_within_one_frame_of_the_audio() {
        let delay = Dur::from_millis(8_000);
        let mut video = VideoHold::new(delay);
        let mut audio = AudioStamp::new(delay);
        let frames = 30 * 3_600usize;
        for n in 0..frames {
            let ts = Ts((n as i64) * FRAME);
            video.push(VideoPacket {
                pts: ts,
                keyframe: n % 60 == 0,
                data: vec![],
            });
            audio.push(ts);
        }
        let end = Ts((frames as i64 - 1) * FRAME);
        let live = end + delay;
        let pictures = video.release(live);
        let sounds = audio.release(live);
        assert_eq!(pictures.len(), frames);
        assert_eq!(sounds.len(), frames);
        assert!(
            pictures
                .iter()
                .zip(&sounds)
                .all(|(v, a)| { (v.pts.0 - a.0).abs() < FRAME })
        );
        let keys: Vec<_> = pictures
            .iter()
            .filter(|p| p.keyframe)
            .map(|p| p.pts)
            .collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(pictures[0].pts, Ts::ZERO);
        assert_eq!(
            pictures.last().unwrap().pts,
            Ts((frames as i64 - 1) * FRAME)
        );
    }

    #[test]
    fn a_generated_file_has_valid_timestamps() {
        let ffmpeg = Command::new("ffmpeg").arg("-version").status();
        if ffmpeg.map(|s| !s.success()).unwrap_or(true) {
            return;
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let dir = std::env::temp_dir().join(format!("airlock-video-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sync.mp4");
        let made = Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=30:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(made.success());
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,start_time",
                "-of",
                "csv=p=0",
                path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let text = String::from_utf8_lossy(&probe.stdout);
        let mut video_start = None;
        let mut audio_start = None;
        for line in text.lines() {
            let mut parts = line.split(',');
            let kind = parts.next().unwrap_or("");
            let start: f64 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
            if kind == "video" {
                video_start = Some(start);
            }
            if kind == "audio" {
                audio_start = Some(start);
            }
        }
        let video_start = video_start.expect("video stream");
        let audio_start = audio_start.expect("audio stream");
        assert!(
            (video_start - audio_start).abs() < 1.0 / 30.0,
            "starts {video_start} vs {audio_start}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
