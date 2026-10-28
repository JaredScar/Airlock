//! Joins ASR words into one string so regexes can run across word boundaries, while
//! remembering which byte range came from which word (to map matches back to time).

use airlock_core::{Span, Word};
use std::ops::Range;

pub struct TranscriptIndex<'a> {
    pub words: &'a [Word],
    /// Words joined by single spaces.
    pub text: String,
    /// Byte range of each word inside `text`.
    ranges: Vec<Range<usize>>,
}

impl<'a> TranscriptIndex<'a> {
    pub fn new(words: &'a [Word]) -> Self {
        let mut text = String::new();
        let mut ranges = Vec::with_capacity(words.len());
        for (i, w) in words.iter().enumerate() {
            if i > 0 {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(w.text.trim());
            ranges.push(start..text.len());
        }
        TranscriptIndex {
            words,
            text,
            ranges,
        }
    }

    /// Indices of the words a byte range touches.
    pub fn words_in(&self, bytes: Range<usize>) -> Range<usize> {
        let first = self.ranges.iter().position(|r| r.end > bytes.start);
        let last = self.ranges.iter().rposition(|r| r.start < bytes.end);
        match (first, last) {
            (Some(f), Some(l)) if f <= l => f..l + 1,
            _ => 0..0,
        }
    }

    /// Media-time span covered by a byte range, plus the minimum ASR confidence in it.
    pub fn span_of(&self, bytes: Range<usize>) -> Option<(Span, f32)> {
        let idx = self.words_in(bytes);
        if idx.is_empty() {
            return None;
        }
        let ws = &self.words[idx];
        let span = Span::new(ws[0].span.start, ws[ws.len() - 1].span.end);
        let conf = ws.iter().map(|w| w.confidence).fold(1.0f32, f32::min);
        Some((span, conf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_bytes_to_words() {
        let words = vec![
            Word::new("I", Span::from_millis(0, 100), 0.9),
            Word::new("live", Span::from_millis(100, 300), 0.8),
            Word::new("here", Span::from_millis(300, 500), 0.95),
        ];
        let t = TranscriptIndex::new(&words);
        assert_eq!(t.text, "I live here");
        let s = t.text.find("live here").unwrap();
        let (span, conf) = t.span_of(s..t.text.len()).unwrap();
        assert_eq!(span, Span::from_millis(100, 500));
        assert!((conf - 0.8).abs() < 1e-6);
    }
}
