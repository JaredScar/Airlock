//! Offline scoring for scripted clips. Media stays out of git; a script is the
//! words the session would have heard.

use airlock_core::{Category, Span, Ts, Word};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

use crate::eval_label::LabeledSpan;
use crate::{Session, SessionConfig};

#[derive(Debug, Clone, Deserialize)]
pub struct ScriptWord {
    pub text: String,
    pub start_us: i64,
    pub end_us: i64,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    0.95
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClipScript {
    pub clip: String,
    pub words: Vec<ScriptWord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CategoryScore {
    pub category: Category,
    pub hits: u32,
    pub misses: u32,
    pub extras: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalReport {
    pub scores: Vec<CategoryScore>,
    pub skipped: Vec<String>,
    /// Wall time spent inside the session, per clip, in microseconds.
    pub latency_us: Vec<i64>,
}

impl EvalReport {
    pub fn to_json(&self) -> String {
        let scores: Vec<_> = self
            .scores
            .iter()
            .map(|s| {
                serde_json::json!({
                    "category": s.category,
                    "hits": s.hits,
                    "misses": s.misses,
                    "extras": s.extras,
                    "precision": ratio(s.hits, s.hits + s.extras),
                    "recall": ratio(s.hits, s.hits + s.misses),
                })
            })
            .collect();
        serde_json::json!({
            "scores": scores,
            "skipped": self.skipped,
            "latency_us": self.latency_us,
            "latency_p50_us": percentile(&self.latency_us, 0.50),
            "latency_p95_us": percentile(&self.latency_us, 0.95),
        })
        .to_string()
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::from("| Category | Hits | Misses | Extras |\n|---|---:|---:|---:|\n");
        for s in &self.scores {
            out.push_str(&format!(
                "| {:?} | {} | {} | {} |\n",
                s.category, s.hits, s.misses, s.extras
            ));
        }
        if !self.skipped.is_empty() {
            out.push_str("\nSkipped (no script): ");
            out.push_str(&self.skipped.join(", "));
            out.push('\n');
        }
        out.push_str(&format!(
            "\nLatency p50 {} µs, p95 {} µs.\n",
            percentile(&self.latency_us, 0.50),
            percentile(&self.latency_us, 0.95)
        ));
        out
    }
}

/// Runs each script through a fresh session and scores span overlap against labels.
/// Clips with no script are listed as skipped, not as misses.
pub fn score_scripts(labels: &[LabeledSpan], scripts: &[ClipScript]) -> EvalReport {
    let scripted: BTreeSet<_> = scripts.iter().map(|s| s.clip.as_str()).collect();
    let mut skipped: Vec<_> = labels
        .iter()
        .filter(|l| !scripted.contains(l.clip.as_str()))
        .map(|l| l.clip.clone())
        .collect();
    skipped.sort();
    skipped.dedup();

    let mut scores: BTreeMap<Category, CategoryScore> = BTreeMap::new();
    let mut latency_us = Vec::new();
    for script in scripts {
        let labels_for: Vec<_> = labels.iter().filter(|l| l.clip == script.clip).collect();
        let started = std::time::Instant::now();
        let preds = predict(script);
        latency_us.push(started.elapsed().as_micros().min(i64::MAX as u128) as i64);
        let mut used = vec![false; preds.len()];
        for label in &labels_for {
            let score = scores.entry(label.category).or_insert(CategoryScore {
                category: label.category,
                hits: 0,
                misses: 0,
                extras: 0,
            });
            let hit = preds.iter().enumerate().find(|(i, (cat, start, end))| {
                !used[*i]
                    && *cat == label.category
                    && overlaps(*start, *end, label.start_us, label.end_us)
            });
            if let Some((i, _)) = hit {
                used[i] = true;
                score.hits += 1;
            } else {
                score.misses += 1;
            }
        }
        for (i, (cat, _, _)) in preds.iter().enumerate() {
            if used[i] {
                continue;
            }
            let score = scores.entry(*cat).or_insert(CategoryScore {
                category: *cat,
                hits: 0,
                misses: 0,
                extras: 0,
            });
            score.extras += 1;
        }
    }
    EvalReport {
        scores: scores.into_values().collect(),
        skipped,
        latency_us,
    }
}

fn predict(script: &ClipScript) -> Vec<(Category, i64, i64)> {
    let words: Vec<Word> = script
        .words
        .iter()
        .map(|w| {
            Word::new(
                w.text.clone(),
                Span::new(Ts(w.start_us), Ts(w.end_us)),
                w.confidence,
            )
        })
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut session = Session::new(SessionConfig::default());
    let end = words.iter().map(|w| w.span.end).max().unwrap_or(Ts::ZERO);
    session.on_words(words);
    session.on_live_time(end);
    session
        .engine()
        .all()
        .iter()
        .map(|r| (r.category, r.span.start.0, r.span.end.0))
        .collect()
}

fn overlaps(a0: i64, a1: i64, b0: i64, b1: i64) -> bool {
    a0 < b1 && b0 < a1
}

fn ratio(num: u32, den: u32) -> f64 {
    if den == 0 {
        1.0
    } else {
        f64::from(num) / f64::from(den)
    }
}

fn percentile(samples: &[i64], p: f64) -> i64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_labels;

    #[test]
    fn scripted_profanity_and_phone_score_as_hits() {
        let labels = parse_labels(include_str!("../../../evals/v1/labels.jsonl")).unwrap();
        let scripts: Vec<ClipScript> =
            serde_json::from_str(include_str!("../../../evals/v1/scripts.json")).unwrap();
        let report = score_scripts(&labels, &scripts);
        let profanity = report
            .scores
            .iter()
            .find(|s| s.category == Category::Profanity)
            .unwrap();
        let phone = report
            .scores
            .iter()
            .find(|s| s.category == Category::SpokenPii)
            .unwrap();
        assert!(profanity.hits >= 1 && profanity.misses == 0);
        assert!(phone.hits >= 1 && phone.misses == 0);
        assert!(report.skipped.iter().any(|c| c == "screen-key-01"));
        assert!(report.to_markdown().contains("Profanity"));
        assert!(report.to_json().contains("spoken_pii"));
    }
}
