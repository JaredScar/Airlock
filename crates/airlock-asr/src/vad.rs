//! Gate in front of a recognizer. Silence is not forwarded, so a model does not
//! run on a quiet mix and cannot hallucinate words from it.
//!
//! This is an energy gate with a short hangover, not Silero. A neural VAD waits
//! until the Whisper feature exists; the hangover keeps the end of a word.

use airlock_core::{Ts, Word};

use crate::audio::has_speech_energy;
use crate::{AsrError, SpeechRecognizer};

pub struct VadGate {
    hangover: u32,
    left: u32,
}

impl Default for VadGate {
    fn default() -> Self {
        VadGate {
            hangover: 8,
            left: 0,
        }
    }
}

impl VadGate {
    /// True when this chunk should be sent to the recognizer.
    pub fn allow(&mut self, samples: &[f32]) -> bool {
        if has_speech_energy(samples) {
            self.left = self.hangover;
            return true;
        }
        if self.left > 0 {
            self.left -= 1;
            return true;
        }
        false
    }
}

/// Drops silent chunks before they reach `inner`.
pub struct GatedRecognizer<R> {
    inner: R,
    gate: VadGate,
}

impl<R> GatedRecognizer<R> {
    pub fn new(inner: R) -> Self {
        GatedRecognizer {
            inner,
            gate: VadGate::default(),
        }
    }
}

impl<R: SpeechRecognizer> SpeechRecognizer for GatedRecognizer<R> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn push(&mut self, samples: &[f32], start: Ts) -> Result<Vec<Word>, AsrError> {
        if !self.gate.allow(samples) {
            return Ok(Vec::new());
        }
        self.inner.push(samples, start)
    }

    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Count {
        pushes: u32,
    }

    impl SpeechRecognizer for Count {
        fn name(&self) -> &str {
            "count"
        }
        fn push(&mut self, _samples: &[f32], _start: Ts) -> Result<Vec<Word>, AsrError> {
            self.pushes += 1;
            Ok(Vec::new())
        }
        fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn silence_is_not_forwarded_and_a_word_ending_is() {
        let mut gated = GatedRecognizer::new(Count { pushes: 0 });
        let quiet = vec![0.0f32; 320];
        let loud = vec![0.2f32; 320];
        gated.push(&quiet, Ts::ZERO).unwrap();
        assert_eq!(gated.inner.pushes, 0);
        gated.push(&loud, Ts::from_millis(20)).unwrap();
        assert_eq!(gated.inner.pushes, 1);
        for i in 0..8 {
            gated.push(&quiet, Ts::from_millis(40 + i * 20)).unwrap();
        }
        assert_eq!(gated.inner.pushes, 9);
        gated.push(&quiet, Ts::from_millis(200)).unwrap();
        assert_eq!(gated.inner.pushes, 9);
    }
}
