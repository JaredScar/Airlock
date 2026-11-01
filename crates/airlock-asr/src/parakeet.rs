//! Local Parakeet TDT. The desktop GPU build enables this behind `parakeet`.
//!
//! A two-second window is transcribed with word times. The tail of an open
//! window waits for the next hop. Time is sealed only through the last word
//! written, and the same word is not written twice.

use crate::whisper::{SampleWindow, phrase_is_ready};
use crate::{ASR_SAMPLE_RATE, AsrError, SpeechRecognizer};
use airlock_core::{Dur, Span, Ts, Word};
use std::path::PathBuf;

#[cfg(feature = "parakeet")]
use parakeet_rs::Transcriber;

pub struct ParakeetRecognizer {
    window: SampleWindow,
    commit: WordCommit,
    phrase_sent: bool,
    #[cfg(feature = "parakeet")]
    model: parakeet_rs::ParakeetTDT,
}

impl ParakeetRecognizer {
    pub fn new(model_dir: PathBuf) -> Result<Self, AsrError> {
        #[cfg(not(feature = "parakeet"))]
        {
            let _ = model_dir;
            Err(AsrError::NotImplemented(
                "ParakeetRecognizer (build the desktop app with the parakeet feature)",
            ))
        }
        #[cfg(feature = "parakeet")]
        {
            let model = load_model(&model_dir)?;
            Ok(ParakeetRecognizer {
                window: SampleWindow::new(ASR_SAMPLE_RATE, 2_000, 250),
                commit: WordCommit::default(),
                phrase_sent: false,
                model,
            })
        }
    }
}

#[cfg(feature = "parakeet")]
fn load_model(model_dir: &std::path::Path) -> Result<parakeet_rs::ParakeetTDT, AsrError> {
    let env = parakeet_rs::ort::init()
        .build()
        .map_err(|err| AsrError::ModelLoad(err.to_string()))?;
    #[cfg(feature = "cuda")]
    {
        let gpu = parakeet_rs::ExecutionConfig::new()
            .with_environment(env.clone())
            .with_execution_provider(parakeet_rs::ExecutionProvider::Cuda);
        if let Ok(model) = parakeet_rs::ParakeetTDT::from_pretrained(model_dir, Some(gpu)) {
            return Ok(model);
        }
    }
    let cpu = parakeet_rs::ExecutionConfig::new().with_environment(env);
    parakeet_rs::ParakeetTDT::from_pretrained(model_dir, Some(cpu))
        .map_err(|err| AsrError::ModelLoad(err.to_string()))
}

impl SpeechRecognizer for ParakeetRecognizer {
    fn name(&self) -> &str {
        "parakeet"
    }

    fn push(&mut self, samples: &[f32], start: Ts) -> Result<Vec<Word>, AsrError> {
        let mut out = Vec::new();
        if !crate::audio::is_near_silence(samples) {
            self.phrase_sent = false;
        }
        if self.window.is_gap(start) {
            out.extend(self.commit.flush());
            self.window.reset();
            self.phrase_sent = false;
        }
        let ready = self.window.push(samples, start);
        for (at, audio) in ready {
            if crate::audio::is_near_silence(&audio) {
                out.extend(self.commit.flush());
                continue;
            }
            self.phrase_sent = true;
            out.extend(self.transcribe(&audio, at, false)?);
        }
        if !self.phrase_sent {
            let rate = self.window.rate();
            let early = self
                .window
                .pending()
                .and_then(|(at, audio)| phrase_is_ready(audio, rate).then(|| (at, audio.to_vec())));
            if let Some((at, audio)) = early {
                self.phrase_sent = true;
                out.extend(self.transcribe(&audio, at, true)?);
            }
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        let mut out = Vec::new();
        if let Some((at, audio)) = self.window.finish()
            && audio.len() >= (ASR_SAMPLE_RATE / 2) as usize
        {
            out.extend(self.transcribe(&audio, at, true)?);
        }
        out.extend(self.commit.flush());
        Ok(out)
    }
}

impl ParakeetRecognizer {
    fn transcribe(&mut self, audio: &[f32], at: Ts, complete: bool) -> Result<Vec<Word>, AsrError> {
        #[cfg(not(feature = "parakeet"))]
        {
            let _ = (audio, at, complete);
            Err(AsrError::NotImplemented("ParakeetRecognizer::push"))
        }
        #[cfg(feature = "parakeet")]
        {
            let result = self
                .model
                .transcribe_samples(
                    audio.to_vec(),
                    ASR_SAMPLE_RATE,
                    1,
                    Some(parakeet_rs::TimestampMode::Words),
                )
                .map_err(|err| AsrError::Inference(err.to_string()))?;
            let window_end =
                at + Dur((audio.len() as i64) * 1_000_000 / i64::from(ASR_SAMPLE_RATE));
            let heard = result
                .tokens
                .into_iter()
                .filter_map(|token| {
                    heard_from_token(token.text, token.start, token.end, at, window_end)
                })
                .collect();
            Ok(self.commit.push(heard, window_end, complete))
        }
    }
}

#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
fn heard_from_token(
    text: String,
    start_s: f32,
    end_s: f32,
    origin: Ts,
    limit: Ts,
) -> Option<Heard> {
    let text = text.trim().to_string();
    if normalize_word(&text).is_empty() {
        return None;
    }
    let start = origin + Dur((start_s.max(0.0) as f64 * 1_000_000.0) as i64);
    if start >= limit {
        return None;
    }
    let mut end = origin + Dur((end_s.max(start_s) as f64 * 1_000_000.0) as i64);
    if end > limit {
        end = limit;
    }
    if end <= start {
        end = start + Dur(10_000);
    }
    Some(Heard { text, start, end })
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
struct Heard {
    text: String,
    start: Ts,
    end: Ts,
}

#[derive(Debug, Default)]
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
struct WordCommit {
    committed_until: Ts,
    said: Vec<Said>,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
struct Said {
    norm: String,
    start: Ts,
    end: Ts,
}

#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
const TAIL_US: i64 = 400_000;
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
const REPEAT_GAP_US: i64 = 2_500_000;

#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
impl WordCommit {
    fn push(&mut self, heard: Vec<Heard>, window_end: Ts, complete: bool) -> Vec<Word> {
        let stable_until = if complete {
            window_end
        } else {
            Ts(window_end.0.saturating_sub(TAIL_US))
        };
        let mut out = Vec::new();
        for word in heard {
            if word.end <= self.committed_until {
                continue;
            }
            if word.start < self.committed_until {
                let mid = word.start.0 + (word.end.0 - word.start.0) / 2;
                if mid < self.committed_until.0 {
                    continue;
                }
            }
            if word.start >= stable_until {
                continue;
            }
            self.commit_heard(word, &mut out);
        }
        out
    }

    fn commit_heard(&mut self, word: Heard, out: &mut Vec<Word>) {
        let norm = normalize_word(&word.text);
        if norm.is_empty() {
            return;
        }
        self.said
            .retain(|prev| word.end.0.saturating_sub(prev.end.0) < 30_000_000);
        let repeated = self.said.iter().any(|prev| {
            prev.norm == norm && within_repeat(prev.start, prev.end, word.start, word.end)
        });
        if repeated {
            return;
        }
        self.committed_until = self.committed_until.max(word.end);
        self.said.push(Said {
            norm,
            start: word.start,
            end: word.end,
        });
        out.push(Word::new(word.text, Span::new(word.start, word.end), 0.9));
    }

    fn flush(&mut self) -> Vec<Word> {
        Vec::new()
    }
}

#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
fn within_repeat(a0: Ts, a1: Ts, b0: Ts, b1: Ts) -> bool {
    b0.0 < a1.0 + REPEAT_GAP_US && b1.0 > a0.0 - REPEAT_GAP_US
}

#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
fn normalize_word(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '\'')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(text: &str, start_ms: i64) -> Heard {
        Heard {
            text: text.into(),
            start: Ts::from_millis(start_ms),
            end: Ts::from_millis(start_ms + 200),
        }
    }

    #[test]
    fn a_window_keeps_stable_words_and_the_next_window_fills_the_gap() {
        let mut commit = WordCommit::default();
        let first = commit.push(
            vec![heard("Hello", 100), heard("there", 400)],
            Ts::from_millis(2_000),
            false,
        );
        assert_eq!(
            first
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Hello", "there"]
        );
        let again = commit.push(
            vec![heard("hello", 120), heard("world", 1_200)],
            Ts::from_millis(2_250),
            false,
        );
        assert_eq!(
            again
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["world"]
        );
        let held = commit.push(vec![heard("later", 1_900)], Ts::from_millis(2_000), false);
        assert!(held.is_empty());
    }

    #[test]
    fn a_token_past_the_audio_is_dropped() {
        assert!(
            heard_from_token("nope".into(), 3.0, 3.2, Ts::ZERO, Ts::from_millis(2_000)).is_none()
        );
        let word = heard_from_token(
            "yes".into(),
            0.1,
            0.4,
            Ts::from_millis(1_000),
            Ts::from_millis(3_000),
        )
        .expect("word");
        assert_eq!(word.text, "yes");
        assert_eq!(word.start, Ts::from_millis(1_100));
    }
}
