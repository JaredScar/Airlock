//! Applying audio redactions to delayed PCM right before it's encoded.

use airlock_core::{Action, Dur, Redaction, Span, Ts};

/// Interleaved f32 PCM with its media start time.
pub struct AudioChunk<'a> {
    pub start: Ts,
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: &'a mut [f32],
}

impl AudioChunk<'_> {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
    pub fn span(&self) -> Span {
        Span::new(
            self.start,
            self.start + Dur::from_secs_f64(self.frames() as f64 / self.sample_rate as f64),
        )
    }
}

pub const BLEEP_HZ: f32 = 1_000.0;
/// -12 dBFS: clearly audible, not painful.
pub const BLEEP_GAIN: f32 = 0.25;
/// Ramp in/out so redactions don't click.
pub const FADE_MS: f64 = 5.0;

/// Applies every redaction in `redactions` that overlaps the chunk.
/// Callers pass `engine.audio_redactions_in(chunk.span())`.
pub fn apply_audio_redactions<'r>(
    chunk: &mut AudioChunk<'_>,
    redactions: impl IntoIterator<Item = &'r Redaction>,
) {
    for r in redactions {
        let tone = match r.action {
            Action::Bleep => true,
            Action::Mute => false,
            _ => continue,
        };
        apply_one(chunk, r.span, tone);
    }
}

fn apply_one(chunk: &mut AudioChunk<'_>, span: Span, tone: bool) {
    let sr = chunk.sample_rate as f64;
    let ch = chunk.channels.max(1);
    let fade = (FADE_MS / 1000.0 * sr).max(1.0);
    let span_len = (span.end - span.start).as_secs_f64() * sr;

    for frame in 0..chunk.frames() {
        let t = chunk.start + Dur::from_secs_f64(frame as f64 / sr);
        if !span.contains(t) {
            continue;
        }
        // Position inside the redaction, in samples. Computed from absolute time so the
        // bleep's phase stays continuous across chunk boundaries.
        let pos = (t - span.start).as_secs_f64() * sr;
        let edge = pos.min(span_len - pos).max(0.0);
        let wet = (edge / fade).min(1.0) as f32; // 0 at edges -> 1 inside
        let bleep = if tone {
            (2.0 * std::f64::consts::PI * BLEEP_HZ as f64 * pos / sr).sin() as f32 * BLEEP_GAIN
        } else {
            0.0
        };
        for c in 0..ch {
            let s = &mut chunk.samples[frame * ch + c];
            *s = *s * (1.0 - wet) + bleep * wet;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::{Category, Mode, Status, Target};

    fn redaction(action: Action, start_ms: i64, end_ms: i64) -> Redaction {
        Redaction {
            id: 1,
            category: Category::Profanity,
            mode: Mode::Apply,
            action,
            target: Target::Audio,
            span: Span::from_millis(start_ms, end_ms),
            status: Status::Pending,
            confidence: 1.0,
            sources: vec![],
            reasons: vec![],
            evidence: vec![],
        }
    }

    #[test]
    fn mute_silences_only_the_span() {
        let mut buf = vec![0.5f32; 48_000 * 2]; // 1s stereo
        let mut chunk = AudioChunk {
            start: Ts::ZERO,
            sample_rate: 48_000,
            channels: 2,
            samples: &mut buf,
        };
        apply_audio_redactions(&mut chunk, [&redaction(Action::Mute, 400, 600)]);
        let at = |ms: usize| buf[(ms * 48) * 2];
        assert_eq!(at(100), 0.5);
        assert_eq!(at(500), 0.0);
        assert_eq!(at(900), 0.5);
        // Fade edge is partial, not a hard cut.
        let edge = at(401);
        assert!(edge > 0.0 && edge < 0.5, "{edge}");
    }

    #[test]
    fn bleep_replaces_with_tone() {
        let mut buf = vec![0.0f32; 48_000];
        let mut chunk = AudioChunk {
            start: Ts::ZERO,
            sample_rate: 48_000,
            channels: 1,
            samples: &mut buf,
        };
        apply_audio_redactions(&mut chunk, [&redaction(Action::Bleep, 200, 800)]);
        let peak = buf[300 * 48..700 * 48]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - BLEEP_GAIN).abs() < 0.01, "{peak}");
        assert!(buf[..190 * 48].iter().all(|s| *s == 0.0));
    }
}
