//! A fixed delay of interleaved `f32` samples.
//!
//! Output is silence until the delay has filled. Dump replaces what is held with
//! silence and leaves the delay length alone. This is the hold an OBS audio
//! filter can run without the rest of Airlock.

use std::collections::VecDeque;

/// 60 seconds at 192 kHz. Longer settings are clamped.
const MAX_DELAY_FRAMES: usize = 192_000 * 60;

#[derive(Debug)]
pub struct SampleDelay {
    delay_frames: usize,
    channels: usize,
    pending: VecDeque<f32>,
}

impl SampleDelay {
    pub fn new(delay_frames: usize, channels: usize) -> Self {
        SampleDelay {
            delay_frames: delay_frames.min(MAX_DELAY_FRAMES),
            channels: channels.clamp(1, 8),
            pending: VecDeque::new(),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn delay_frames(&self) -> usize {
        self.delay_frames
    }

    /// Shortening the delay drops the oldest held samples. Lengthening it
    /// outputs silence until the new length has filled.
    pub fn set_delay_frames(&mut self, frames: usize) {
        let frames = frames.min(MAX_DELAY_FRAMES);
        if frames < self.delay_frames {
            let drop_n = (self.delay_frames - frames) * self.channels;
            for _ in 0..drop_n {
                if self.pending.pop_front().is_none() {
                    break;
                }
            }
        }
        self.delay_frames = frames;
    }

    /// Replaces held samples with silence. The delay length stays the same.
    pub fn dump(&mut self) {
        for sample in &mut self.pending {
            *sample = 0.0;
        }
    }

    /// Reads interleaved samples and writes the delayed samples back in place.
    /// A buffer whose length is not a whole number of frames is left unchanged.
    pub fn process_in_place(&mut self, samples: &mut [f32]) {
        if self.channels == 0 || !samples.len().is_multiple_of(self.channels) {
            return;
        }
        let primed = self.delay_frames.saturating_mul(self.channels);
        for sample in samples.iter_mut() {
            self.pending.push_back(*sample);
            if self.pending.len() > primed
                && let Some(old) = self.pending.pop_front()
            {
                *sample = old;
            } else {
                *sample = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_holds_then_releases_and_dump_silences_the_hold() {
        let mut delay = SampleDelay::new(4, 1);
        let mut first = [1.0, 1.0, 1.0, 1.0];
        delay.process_in_place(&mut first);
        assert_eq!(first, [0.0, 0.0, 0.0, 0.0]);

        let mut second = [0.25, 0.25, 0.25, 0.25];
        delay.process_in_place(&mut second);
        assert_eq!(second, [1.0, 1.0, 1.0, 1.0]);

        delay.dump();
        let mut third = [0.5, 0.5, 0.5, 0.5];
        delay.process_in_place(&mut third);
        assert_eq!(third, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(delay.delay_frames(), 4);
    }

    #[test]
    fn stereo_frames_stay_paired() {
        let mut delay = SampleDelay::new(1, 2);
        let mut first = [1.0, 2.0];
        delay.process_in_place(&mut first);
        assert_eq!(first, [0.0, 0.0]);
        let mut second = [3.0, 4.0];
        delay.process_in_place(&mut second);
        assert_eq!(second, [1.0, 2.0]);
    }
}
