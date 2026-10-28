//! Token-level wordlist matching for profanity and slurs.
//!
//! List format (one entry per line, `#` comments):
//!   `word`   exact token match
//!   `word*`  prefix match ("fuck*" catches "fucking", "fucker")
//! Token-based matching avoids the Scunthorpe problem: "class" never matches "ass".

use crate::normalize::{collapse_runs, is_elongated, normalize_token};
use airlock_core::model::mask_evidence;
use airlock_core::{Category, Detection, DetectorKind, Modality, Word};
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct Entry {
    stem: String,
    prefix: bool,
    category: Category,
}

#[derive(Debug, Default, Clone)]
pub struct WordlistDetector {
    entries: Vec<Entry>,
    /// Tokens that must never match, even if a prefix entry would ("cocktail", "assassin").
    allow: HashSet<String>,
}

impl WordlistDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds every entry in a list file's contents under `category`.
    pub fn add_list(&mut self, category: Category, contents: &str) -> &mut Self {
        for line in contents.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (raw, prefix) = match line.strip_suffix('*') {
                Some(s) => (s, true),
                None => (line, false),
            };
            let stem = normalize_token(raw);
            if !stem.is_empty() {
                self.entries.push(Entry {
                    stem,
                    prefix,
                    category,
                });
            }
        }
        self
    }

    /// Adds allowlist entries (one per line).
    pub fn add_allowlist(&mut self, contents: &str) -> &mut Self {
        for line in contents.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if !line.is_empty() {
                self.allow.insert(normalize_token(line));
            }
        }
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn match_token(&self, token: &str) -> Option<Category> {
        if token.is_empty() || self.allow.contains(token) {
            return None;
        }
        let elongated = is_elongated(token);
        let collapsed = collapse_runs(token);
        // Slurs win over profanity if both match.
        let mut best: Option<Category> = None;
        for e in &self.entries {
            let hit = if e.prefix {
                token.starts_with(&e.stem)
                    || (elongated && collapsed.starts_with(&collapse_runs(&e.stem)))
            } else {
                token == e.stem || (elongated && collapsed == collapse_runs(&e.stem))
            };
            if hit {
                best = match (best, e.category) {
                    (Some(Category::Slur), _) => Some(Category::Slur),
                    (_, c) => Some(c),
                };
            }
        }
        best
    }

    /// Scans ASR words. One detection per matching word; the decision engine merges.
    pub fn scan(&self, words: &[Word]) -> Vec<Detection> {
        words
            .iter()
            .filter_map(|w| {
                let token = normalize_token(&w.text);
                let category = self.match_token(&token)?;
                Some(Detection {
                    source: DetectorKind::Wordlist,
                    category,
                    modality: Modality::Audio,
                    span: w.span,
                    region: None,
                    confidence: w.confidence,
                    reason: match category {
                        Category::Slur => "Slur".to_string(),
                        _ => "Profanity".to_string(),
                    },
                    evidence: mask_evidence(&token),
                })
            })
            .collect()
    }
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

    fn detector() -> WordlistDetector {
        let mut d = WordlistDetector::new();
        d.add_list(Category::Profanity, "# test list\nfuck*\nshit*\nass\n");
        d.add_allowlist("shitake\n");
        d
    }

    #[test]
    fn matches_variants() {
        let d = detector();
        let hits = d.scan(&words("what the FUCKING hell, $h1t! fuuuuck"));
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].span, Span::from_millis(600, 850));
        assert_eq!(hits[0].evidence, "fu•••••");
    }

    #[test]
    fn avoids_scunthorpe() {
        let d = detector();
        assert!(
            d.scan(&words("this class is passive, pass the shitake"))
                .is_empty()
        );
        // Exact entries don't prefix-match.
        assert!(d.scan(&words("assess the assets")).is_empty());
        assert_eq!(d.scan(&words("you ass")).len(), 1);
        assert_eq!(d.scan(&words("you asssss")).len(), 1);
    }
}
