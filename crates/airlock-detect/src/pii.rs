//! Spoken self-doxxing: the streamer saying identifying info out loud.
//!
//! Two layers:
//! 1. Pattern PII (phone numbers, emails, street addresses) via regex over the joined transcript.
//!    Phone numbers also match when the digits are spaced ("631 555 0123") or spelled
//!    ("six three one five five five zero one two three"). See PLAN 2.5.
//! 2. Private terms: things only the streamer knows are sensitive (real name, town,
//!    school, employer, family names). Configured locally; never sent to cloud providers.

use crate::transcript::TranscriptIndex;
use airlock_core::model::mask_evidence;
use airlock_core::{Category, Detection, DetectorKind, Modality, Span, Word};
use regex::Regex;
use std::sync::LazyLock;

static PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let r = |s: &str| Regex::new(s).expect("valid PII regex");
    vec![
        (
            "Phone number",
            r(r"(?:\+?1[\s.-]?)?\(?\b\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b"),
        ),
        ("Email address", r(r"(?i)\b[\w.+-]+@[\w-]+\.[a-z]{2,}\b")),
        // How ASR usually writes a spoken email: "jared at gmail dot com".
        (
            "Email address",
            r(r"(?i)\b[\w.]+ at [\w]+ dot (?:com|net|org|io|gg|tv|co|edu|us)\b"),
        ),
        (
            "Street address",
            r(
                r"(?i)\b\d{1,6}\s+(?:[a-z]+\s+){1,3}(?:street|st|avenue|ave|road|rd|lane|ln|drive|dr|court|ct|boulevard|blvd|place|pl|way|terrace|circle|cir|highway|hwy|parkway|pkwy)\b",
            ),
        ),
        // "my zip is 11787" / "zip code 11787"
        ("ZIP code", r(r"(?i)\bzip(?:\s+code)?(?:\s+is)?\s+\d{5}\b")),
    ]
});

/// Ten digits, or a leading 1 plus ten, each digit its own word: "6 3 1 5 5 5 0 1 2 3".
/// Grouped forms like "631 555 0123" are handled by `PATTERNS` instead.
static SPOKEN_DIGIT_PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:1\s+)?(?:\d\s+){9}\d\b").expect("valid spoken-digit phone regex")
});

#[derive(Debug, Default, Clone)]
pub struct SpokenPiiDetector {
    /// Lowercased phrases, each matched on word boundaries.
    private_terms: Vec<String>,
}

impl SpokenPiiDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_private_terms<I, S>(terms: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let private_terms = terms
            .into_iter()
            .map(|t| t.as_ref().trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        SpokenPiiDetector { private_terms }
    }

    pub fn scan(&self, words: &[Word]) -> Vec<Detection> {
        let idx = TranscriptIndex::new(words);
        let mut out = Vec::new();

        for (label, re) in PATTERNS.iter() {
            for m in re.find_iter(&idx.text) {
                if let Some((span, conf)) = idx.span_of(m.range()) {
                    out.push(Detection {
                        source: DetectorKind::Pii,
                        category: Category::SpokenPii,
                        modality: Modality::Audio,
                        span,
                        region: None,
                        confidence: conf,
                        reason: format!("{label} said on stream"),
                        evidence: mask_evidence(m.as_str()),
                    });
                }
            }
        }

        if !self.private_terms.is_empty() {
            // Lowercasing ASCII-only keeps byte offsets aligned with `idx.text`.
            let hay = idx.text.to_ascii_lowercase();
            let hay = hay.as_str();
            for term in &self.private_terms {
                let mut from = 0;
                while let Some(pos) = hay[from..].find(term.as_str()) {
                    let start = from + pos;
                    let end = start + term.len();
                    let boundary_ok = !hay[..start]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric)
                        && !hay[end..].chars().next().is_some_and(char::is_alphanumeric);
                    if boundary_ok && let Some((span, conf)) = idx.span_of(start..end) {
                        out.push(Detection {
                            source: DetectorKind::PrivateTerms,
                            category: Category::SpokenPii,
                            modality: Modality::Audio,
                            span,
                            region: None,
                            confidence: conf,
                            reason: "Private term said on stream".into(),
                            evidence: mask_evidence(term),
                        });
                    }
                    from = end;
                }
            }
        }

        self.push_spoken_digit_phones(words, &mut out);
        out
    }

    /// Spelled-out and one-digit-per-word phone numbers.
    ///
    /// "six" becomes "6" in a parallel string, then the same 10-digit shape is
    /// matched and mapped back onto the original words. Ordinary numbers such
    /// as "2500" or "2019" never contain ten separated digits, so they stay quiet.
    fn push_spoken_digit_phones(&self, words: &[Word], out: &mut Vec<Detection>) {
        let (folded, owner) = fold_spelled_digits(words);
        for m in SPOKEN_DIGIT_PHONE.find_iter(&folded) {
            let range = m.range();
            let Some(&first) = owner.get(range.start) else {
                continue;
            };
            let Some(&last) = owner.get(range.end.saturating_sub(1)) else {
                continue;
            };
            if first > last || last >= words.len() {
                continue;
            }
            let ws = &words[first..=last];
            let span = Span::new(ws[0].span.start, ws[ws.len() - 1].span.end);
            if out
                .iter()
                .any(|d| d.category == Category::SpokenPii && d.span.overlaps(&span))
            {
                continue;
            }
            let conf = ws.iter().map(|w| w.confidence).fold(1.0f32, f32::min);
            let spoken: Vec<&str> = ws.iter().map(|w| w.text.trim()).collect();
            out.push(Detection {
                source: DetectorKind::Pii,
                category: Category::SpokenPii,
                modality: Modality::Audio,
                span,
                region: None,
                confidence: conf,
                reason: "Phone number said on stream".into(),
                evidence: mask_evidence(&spoken.join(" ")),
            });
        }
    }
}

/// "oh" counts as zero, which is how people read a phone aloud.
fn spelled_digit(word: &str) -> Option<char> {
    let key = word
        .trim_matches(|c: char| !c.is_ascii_alphabetic())
        .to_ascii_lowercase();
    Some(match key.as_str() {
        "zero" | "oh" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        _ => return None,
    })
}

/// Spelled digits become numerals. Every other word is kept, so a byte offset
/// in the result still points at a source word via `owner`.
fn fold_spelled_digits(words: &[Word]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut owner = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            text.push(' ');
            owner.push(i);
        }
        let raw = w.text.trim();
        let piece = match spelled_digit(raw) {
            Some(digit) => digit.to_string(),
            None => raw.to_string(),
        };
        owner.extend(std::iter::repeat_n(i, piece.len()));
        text.push_str(&piece);
    }
    (text, owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::Span;

    fn words(s: &str) -> Vec<Word> {
        s.split_whitespace()
            .enumerate()
            .map(|(i, t)| {
                Word::new(
                    t,
                    Span::from_millis(i as i64 * 300, i as i64 * 300 + 250),
                    0.9,
                )
            })
            .collect()
    }

    #[test]
    fn finds_pattern_pii() {
        let d = SpokenPiiDetector::new();
        let hits = d.scan(&words(
            "call me at 631-555-0123 or email jared at gmail dot com",
        ));
        assert_eq!(hits.len(), 2, "{hits:?}");
        let hits = d.scan(&words("yeah I live at 42 Old Willow Lane lol"));
        assert_eq!(hits.len(), 1);
        // "42 Old Willow Lane" = words 4..8
        assert_eq!(hits[0].span, Span::from_millis(1200, 2350));
    }

    #[test]
    fn finds_private_terms_on_boundaries() {
        let d = SpokenPiiDetector::with_private_terms(["Springfield", "Jane Doe"]);
        let hits = d.scan(&words("I grew up near springfield with jane doe"));
        assert_eq!(hits.len(), 2);
        assert!(d.scan(&words("springfielders unite")).is_empty());
    }

    #[test]
    fn ignores_normal_numbers() {
        let d = SpokenPiiDetector::new();
        assert!(
            d.scan(&words("I have 2500 hours in this game since 2019"))
                .is_empty()
        );
        assert!(d.scan(&words("one more try and then two more")).is_empty());
    }

    #[test]
    fn finds_spaced_and_spelled_phone_numbers() {
        let d = SpokenPiiDetector::new();
        let spaced = d.scan(&words("call 631 555 0123 tonight"));
        assert_eq!(spaced.len(), 1, "{spaced:?}");
        assert_eq!(spaced[0].span, Span::from_millis(300, 1150));
        assert!(!spaced[0].evidence.contains("0123"));

        let spelled = d.scan(&words(
            "it is six three one five five five zero one two three thanks",
        ));
        assert_eq!(spelled.len(), 1, "{spelled:?}");
        assert_eq!(spelled[0].span, Span::from_millis(600, 3550));
        assert_eq!(spelled[0].category, Category::SpokenPii);

        let with_oh = d.scan(&words(
            "dial six three one five five five oh one two three please",
        ));
        assert_eq!(with_oh.len(), 1, "{with_oh:?}");
        assert_eq!(with_oh[0].span, Span::from_millis(300, 3250));
    }
}
