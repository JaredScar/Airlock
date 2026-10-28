//! Labeled spans for the offline eval corpus (PLAN 6.1).
//!
//! One JSON object per line. Media files stay out of git; a line only names the clip.
//!
//! ```json
//! {"clip":"spoken-phone-01","category":"spoken_pii","start_us":1000000,"end_us":2400000}
//! {"clip":"screen-key-01","category":"screen_secret","start_us":500000,"end_us":900000,"region":{"x":0.6,"y":0.8,"w":0.3,"h":0.05}}
//! ```

use airlock_core::{Category, Rect};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LabeledSpan {
    pub clip: String,
    pub category: Category,
    /// Inclusive start, microseconds of media time.
    pub start_us: i64,
    /// Exclusive end, microseconds of media time.
    pub end_us: i64,
    #[serde(default)]
    pub region: Option<Rect>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Parses a JSONL label file. Blank lines are skipped. A bad line names its number.
pub fn parse_labels(jsonl: &str) -> Result<Vec<LabeledSpan>, String> {
    let mut out = Vec::new();
    for (i, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row: LabeledSpan =
            serde_json::from_str(line).map_err(|e| format!("evals label line {}: {e}", i + 1))?;
        if row.end_us < row.start_us {
            return Err(format!(
                "evals label line {}: end_us is before start_us",
                i + 1
            ));
        }
        if row.clip.is_empty() {
            return Err(format!("evals label line {}: clip name is empty", i + 1));
        }
        out.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../../evals/v1/labels.jsonl");

    #[test]
    fn sample_corpus_parses() {
        let rows = parse_labels(SAMPLE).unwrap();
        assert!(rows.len() >= 3, "{rows:?}");
        assert!(rows.iter().any(|r| r.category == Category::SpokenPii));
        assert!(rows.iter().any(|r| r.region.is_some()));
        assert!(rows.iter().all(|r| r.end_us >= r.start_us));
    }

    #[test]
    fn rejects_a_backwards_span() {
        let text = "{\"clip\":\"x\",\"category\":\"profanity\",\"start_us\":5,\"end_us\":1}\n";
        assert!(parse_labels(text).unwrap_err().contains("line 1"));
    }
}
