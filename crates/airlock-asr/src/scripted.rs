//! A fake recognizer that replays a script. Used by tests, the demo mode in the
//! desktop app, and the eval harness (to isolate detector quality from ASR quality).

use crate::{ASR_SAMPLE_RATE, AsrError, SpeechRecognizer};
use airlock_core::{Dur, Ts, Word};
use std::collections::VecDeque;

pub struct ScriptedRecognizer {
    pending: VecDeque<Word>,
    /// Simulated recognition latency: a word is emitted once audio this far past its end arrives.
    pub latency: Dur,
}

impl ScriptedRecognizer {
    pub fn new(mut words: Vec<Word>) -> Self {
        words.sort_by_key(|w| w.span.start);
        ScriptedRecognizer {
            pending: words.into(),
            latency: Dur::from_millis(300),
        }
    }

    /// Builds a script from text, spacing words evenly at `ms_per_word` from `start`.
    pub fn from_text(text: &str, start: Ts, ms_per_word: i64) -> Self {
        let words = text
            .split_whitespace()
            .enumerate()
            .map(|(i, t)| {
                let s = start + Dur::from_millis(i as i64 * ms_per_word);
                Word::new(
                    t,
                    airlock_core::Span::new(s, s + Dur::from_millis(ms_per_word - 40)),
                    0.92,
                )
            })
            .collect();
        Self::new(words)
    }
}

impl SpeechRecognizer for ScriptedRecognizer {
    fn name(&self) -> &str {
        "scripted"
    }

    fn push(&mut self, samples: &[f32], start: Ts) -> Result<Vec<Word>, AsrError> {
        let heard_until = start + Dur::from_secs_f64(samples.len() as f64 / ASR_SAMPLE_RATE as f64);
        let mut out = Vec::new();
        while let Some(w) = self.pending.front() {
            if w.span.end + self.latency <= heard_until {
                out.push(self.pending.pop_front().unwrap());
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        Ok(self.pending.drain(..).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_words_as_audio_arrives() {
        let mut r = ScriptedRecognizer::from_text("one two three", Ts::ZERO, 500);
        let half_second = vec![0.0; 8_000];
        assert!(r.push(&half_second, Ts::ZERO).unwrap().is_empty());
        let got = r.push(&half_second, Ts::from_millis(500)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(r.flush().unwrap().len(), 2);
    }
}
