//! Replaces held video packets while a video redaction is active.
//!
//! The broadcast stays a packet copy. A sampled frame is pixelated and encoded
//! only for a span the decision engine is actually applying. That keyframe is
//! repeated until the next clean keyframe, and the publisher's sequence header
//! is sent again so the decoder can return to the original stream.

use std::io::Write;
use std::process::{Command, Stdio};

use airlock_core::{Rect, Redaction, Span, Ts};

use crate::audio_path::MediaError;
use crate::forward::drain_video_tags;
use crate::video_fx::{blackout_bgra, pixelate_bgra};
use crate::video_hold::VideoPacket;

/// One pixelated picture that stands in for the publisher's video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameCover {
    pub span: Span,
    pub sequence: Vec<u8>,
    pub keyframe: Vec<u8>,
}

/// Remembers the publisher's sequence header and whether a substitute is on air.
#[derive(Debug, Default)]
pub struct VideoCover {
    original_header: Option<Vec<u8>>,
    holding: Option<Held>,
}

#[derive(Debug, Clone)]
struct Held {
    sequence: Vec<u8>,
    keyframe: Vec<u8>,
    header_sent: bool,
}

impl VideoCover {
    pub fn apply(
        &mut self,
        packets: Vec<VideoPacket>,
        redactions: &[Redaction],
        covers: &[FrameCover],
    ) -> Vec<VideoPacket> {
        let mut out = Vec::with_capacity(packets.len());
        for packet in packets {
            if is_sequence(&packet.data) {
                self.original_header = Some(packet.data.clone());
            }
            let covered = redactions.iter().any(|redaction| {
                redaction.is_active()
                    && !redaction.is_audio()
                    && redaction.span.contains(packet.pts)
            });
            if covered
                && let Some(cover) = cover_for(covers, packet.pts, redactions)
                && self.holding.as_ref().map(|held| held.sequence.as_slice())
                    != Some(cover.sequence.as_slice())
            {
                self.holding = Some(Held {
                    sequence: cover.sequence.clone(),
                    keyframe: cover.keyframe.clone(),
                    header_sent: false,
                });
            }
            let clean = !covered && packet.keyframe && !is_sequence(&packet.data);
            if clean {
                if self.holding.take().is_some()
                    && let Some(header) = &self.original_header
                {
                    out.push(VideoPacket {
                        pts: packet.pts,
                        keyframe: true,
                        data: header.clone(),
                    });
                }
                out.push(packet);
                continue;
            }
            if let Some(held) = &mut self.holding {
                if !held.header_sent {
                    out.push(VideoPacket {
                        pts: packet.pts,
                        keyframe: true,
                        data: held.sequence.clone(),
                    });
                    held.header_sent = true;
                }
                out.push(VideoPacket {
                    pts: packet.pts,
                    keyframe: true,
                    data: held.keyframe.clone(),
                });
                continue;
            }
            out.push(packet);
        }
        out
    }
}

/// Pixelates `regions` (or blacks out `blackouts`) and encodes one keyframe.
pub fn cover_frame(
    bgra: &[u8],
    width: u32,
    height: u32,
    regions: &[Rect],
    blackouts: &[Rect],
    span: Span,
) -> Result<FrameCover, MediaError> {
    if width == 0 || height == 0 || bgra.len() < (width * height * 4) as usize {
        return Err(MediaError::Ffmpeg("frame is empty".into()));
    }
    let mut painted = bgra.to_vec();
    let stride = width * 4;
    for region in blackouts {
        blackout_bgra(&mut painted, width, height, stride, *region);
    }
    for region in regions {
        pixelate_bgra(&mut painted, width, height, stride, *region, 32);
    }
    let (sequence, keyframe) = encode_keyframe(&painted, width, height)?;
    Ok(FrameCover {
        span,
        sequence,
        keyframe,
    })
}

fn cover_for<'a>(
    covers: &'a [FrameCover],
    pts: Ts,
    redactions: &[Redaction],
) -> Option<&'a FrameCover> {
    let active: Vec<Span> = redactions
        .iter()
        .filter(|redaction| {
            redaction.is_active() && !redaction.is_audio() && redaction.span.contains(pts)
        })
        .map(|redaction| redaction.span)
        .collect();
    let overlapping: Vec<_> = covers
        .iter()
        .filter(|cover| active.iter().any(|span| span.overlaps(&cover.span)))
        .collect();
    if let Some(hit) = overlapping
        .iter()
        .copied()
        .filter(|cover| cover.span.contains(pts))
        .max_by_key(|cover| cover.span.start)
    {
        return Some(hit);
    }
    overlapping
        .into_iter()
        .min_by_key(|cover| distance(cover.span, pts))
}

fn distance(span: Span, pts: Ts) -> i64 {
    if pts < span.start {
        span.start.0 - pts.0
    } else if pts >= span.end {
        pts.0 - span.end.0
    } else {
        0
    }
}

fn is_sequence(body: &[u8]) -> bool {
    body.len() > 1 && body[0] & 0x0F == 7 && body[1] == 0
}

fn encode_keyframe(bgra: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, Vec<u8>), MediaError> {
    let size = format!("{width}x{height}");
    let mut child = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
            "-s",
            &size,
            "-r",
            "4",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-g",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "flv",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                MediaError::FfmpegMissing
            } else {
                MediaError::Ffmpeg(err.to_string())
            }
        })?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| MediaError::Ffmpeg("encoder stdin closed".into()))?;
        stdin
            .write_all(bgra)
            .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| MediaError::Ffmpeg(err.to_string()))?;
    if !output.status.success() {
        return Err(MediaError::Ffmpeg("pixelate encode failed".into()));
    }
    let mut buf = output.stdout;
    let mut packets = Vec::new();
    drain_video_tags(&mut buf, 0, &mut packets);
    let sequence = packets
        .iter()
        .find(|packet| is_sequence(&packet.data))
        .map(|packet| packet.data.clone());
    let keyframe = packets
        .iter()
        .find(|packet| packet.keyframe && !is_sequence(&packet.data))
        .map(|packet| packet.data.clone());
    match (sequence, keyframe) {
        (Some(sequence), Some(keyframe)) => Ok((sequence, keyframe)),
        _ => Err(MediaError::Ffmpeg(
            "pixelate encode produced no keyframe".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::{Action, Category, Mode, Status, Target};

    fn redaction(start: i64, end: i64, mode: Mode, active: bool) -> Redaction {
        Redaction {
            id: 1,
            category: Category::ScreenSecret,
            mode,
            action: Action::Pixelate,
            target: Target::Video { region: Rect::FULL },
            span: Span::new(Ts(start), Ts(end)),
            status: if active {
                Status::Approved
            } else {
                Status::Pending
            },
            confidence: 1.0,
            sources: vec![],
            reasons: vec![],
            evidence: vec![],
            missed_live: false,
        }
    }

    fn packet(us: i64, keyframe: bool, data: &[u8]) -> VideoPacket {
        VideoPacket {
            pts: Ts(us),
            keyframe,
            data: data.to_vec(),
        }
    }

    #[test]
    fn an_active_span_is_replaced_until_the_next_clean_keyframe() {
        let sequence = [0x17, 0x00, 0x11];
        let key = [0x17, 0x01, 0x22];
        let inter = [0x27, 0x01, 0x33];
        let substitute_sequence = vec![0x17, 0x00, 0x99];
        let substitute_key = vec![0x17, 0x01, 0x88];
        let covers = vec![FrameCover {
            span: Span::new(Ts(150), Ts(400)),
            sequence: substitute_sequence.clone(),
            keyframe: substitute_key.clone(),
        }];
        let packets = vec![
            packet(0, true, &sequence),
            packet(0, true, &key),
            packet(100, false, &inter),
            packet(200, false, &inter),
            packet(300, false, &inter),
            packet(500, false, &inter),
            packet(600, true, &key),
        ];
        let mut cover = VideoCover::default();
        let out = cover.apply(packets, &[redaction(150, 400, Mode::Apply, false)], &covers);
        let bodies: Vec<&[u8]> = out.iter().map(|packet| packet.data.as_slice()).collect();
        assert_eq!(
            bodies,
            vec![
                sequence.as_slice(),
                key.as_slice(),
                inter.as_slice(),
                substitute_sequence.as_slice(),
                substitute_key.as_slice(),
                substitute_key.as_slice(),
                substitute_key.as_slice(),
                sequence.as_slice(),
                key.as_slice(),
            ]
        );
        assert_eq!(out[8].pts, Ts(600));
    }

    #[test]
    fn a_suggestion_leaves_the_packets_alone() {
        let key = [0x17, 0x01, 0x22];
        let packets = vec![packet(200, true, &key)];
        let covers = vec![FrameCover {
            span: Span::new(Ts(0), Ts(1_000)),
            sequence: vec![0x17, 0x00, 0x99],
            keyframe: vec![0x17, 0x01, 0x88],
        }];
        let mut cover = VideoCover::default();
        let out = cover.apply(
            packets.clone(),
            &[redaction(0, 1_000, Mode::Suggest, false)],
            &covers,
        );
        assert_eq!(out, packets);
    }

    #[test]
    fn a_pixelated_frame_becomes_the_keyframe_that_airs() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let width = 16u32;
        let height = 16u32;
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[0, 0, 255, 255]);
        }
        let span = Span::new(Ts(0), Ts(250_000));
        let covered = cover_frame(&bgra, width, height, &[Rect::FULL], &[], span).expect("encode");
        assert!(is_sequence(&covered.sequence));
        assert!(!is_sequence(&covered.keyframe));
        let original = vec![0x27, 0x01, 0x44];
        let mut stitch = VideoCover::default();
        let out = stitch.apply(
            vec![packet(10, false, &original)],
            &[redaction(0, 250_000, Mode::Apply, false)],
            std::slice::from_ref(&covered),
        );
        assert!(out.iter().any(|packet| packet.data == covered.keyframe));
        assert!(out.iter().all(|packet| packet.data != original));
    }
}
