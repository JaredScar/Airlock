//! The context judge: an LLM pass over a rolling transcript window that flags
//! ToS-risk speech wordlists can't see.
//!
//! Design notes:
//! - Words are numbered in the prompt and the model answers with word indices, so
//!   flags map back to exact media timestamps without fuzzy text matching.
//! - The judge runs on its own thread with a hard deadline (see `deadline_budget`).
//!   A late answer is still logged for VOD cleanup but can't redact live audio.
//! - Private terms are never put in the prompt; they're handled locally by airlock-detect.

pub mod providers;

use airlock_core::model::mask_evidence;
use airlock_core::{Category, Detection, DetectorKind, Dur, Modality, Platform, Span, Word};
use serde::Deserialize;
use thiserror::Error;

pub use providers::{AnthropicJudge, OllamaJudge};

const PROMPT_TEMPLATE: &str = include_str!("../prompts/context_judge.md");

#[derive(Debug, Error)]
pub enum JudgeError {
    #[error("{0} is not implemented yet (see PLAN.md)")]
    NotImplemented(&'static str),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("bad model output: {0}")]
    BadOutput(String),
    #[error("deadline exceeded")]
    Timeout,
}

pub struct JudgeRequest<'a> {
    /// Context + focus words, in order.
    pub words: &'a [Word],
    /// Index into `words` where the not-yet-judged focus region starts.
    pub focus_start: usize,
    pub platform: Platform,
    pub delay: Dur,
    pub streamer_rules: &'a [String],
}

pub trait ContextJudge: Send {
    fn name(&self) -> &str;
    fn judge(&self, req: &JudgeRequest<'_>) -> Result<Vec<Detection>, JudgeError>;
}

/// Time the judge may spend on a request: delay minus ASR latency minus a safety margin
/// for the decision engine and the streamer's review. Returns zero if there's no budget.
pub fn deadline_budget(delay: Dur, asr_latency: Dur, margin: Dur) -> Dur {
    Dur((delay.0 - asr_latency.0 - margin.0).max(0))
}

pub fn build_prompt(req: &JudgeRequest<'_>) -> String {
    let numbered: String = req
        .words
        .iter()
        .enumerate()
        .map(|(i, w)| format!("[{i}] {}\n", w.text))
        .collect();
    let rules = if req.streamer_rules.is_empty() {
        "(none)".to_string()
    } else {
        req.streamer_rules
            .iter()
            .map(|r| format!("- {r}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let platform = match req.platform {
        Platform::Twitch => "Twitch",
        Platform::Youtube => "YouTube",
        Platform::Kick => "Kick",
        Platform::Generic => "a streaming platform",
    };
    PROMPT_TEMPLATE
        .replace("{platform}", platform)
        .replace(
            "{delay_seconds}",
            &format!("{:.0}", req.delay.as_secs_f64()),
        )
        .replace("{streamer_rules}", &rules)
        .replace("{numbered_words}", numbered.trim_end())
        .replace("{focus_start}", &req.focus_start.to_string())
        .replace(
            "{focus_end}",
            &req.words.len().saturating_sub(1).to_string(),
        )
}

#[derive(Deserialize)]
struct RawResponse {
    flags: Vec<RawFlag>,
}

#[derive(Deserialize)]
struct RawFlag {
    start_word: usize,
    end_word: usize,
    #[serde(default)]
    category: Option<String>,
    confidence: f32,
    reason: String,
}

/// Parses model output into detections. Tolerates code fences and leading prose
/// (models do that), drops flags outside the focus range or with bad indices.
pub fn parse_response(raw: &str, req: &JudgeRequest<'_>) -> Result<Vec<Detection>, JudgeError> {
    let start = raw
        .find('{')
        .ok_or_else(|| JudgeError::BadOutput("no JSON object".into()))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| JudgeError::BadOutput("no JSON object".into()))?;
    let parsed: RawResponse = serde_json::from_str(&raw[start..=end])
        .map_err(|e| JudgeError::BadOutput(e.to_string()))?;

    let n = req.words.len();
    Ok(parsed
        .flags
        .into_iter()
        .filter(|f| f.start_word <= f.end_word && f.end_word < n && f.end_word >= req.focus_start)
        .filter(|f| f.category.as_deref().is_none_or(|c| c == "tos_risk"))
        .map(|f| {
            let s = f.start_word.max(req.focus_start);
            let ws = &req.words[s..=f.end_word];
            let text: Vec<&str> = ws.iter().map(|w| w.text.as_str()).collect();
            Detection {
                source: DetectorKind::ContextLlm,
                category: Category::TosRisk,
                modality: Modality::Audio,
                span: Span::new(ws[0].span.start, ws[ws.len() - 1].span.end),
                region: None,
                confidence: f.confidence.clamp(0.0, 1.0),
                reason: f.reason.chars().take(140).collect(),
                evidence: mask_evidence(&text.join(" ")),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::Ts;

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

    fn req(ws: &[Word]) -> JudgeRequest<'_> {
        JudgeRequest {
            words: ws,
            focus_start: 2,
            platform: Platform::Twitch,
            delay: Dur::from_millis(8000),
            streamer_rules: &[],
        }
    }

    #[test]
    fn prompt_numbers_words_and_fills_slots() {
        let ws = words("hello chat welcome back");
        let p = build_prompt(&req(&ws));
        assert!(p.contains("[0] hello\n[1] chat"));
        assert!(p.contains("FOCUS range: [2..3]"));
        assert!(p.contains("on Twitch"));
        for slot in [
            "{platform}",
            "{delay_seconds}",
            "{streamer_rules}",
            "{numbered_words}",
            "{focus_start}",
            "{focus_end}",
        ] {
            assert!(!p.contains(slot), "unfilled template slot {slot}");
        }
    }

    #[test]
    fn parses_and_clamps_to_focus() {
        let ws = words("a b c d e f");
        let raw = "Sure!\n```json\n{\"flags\":[{\"start_word\":1,\"end_word\":3,\"category\":\"tos_risk\",\"confidence\":0.9,\"reason\":\"Harassment\"},{\"start_word\":0,\"end_word\":9,\"confidence\":0.9,\"reason\":\"out of range\"}]}\n```";
        let d = parse_response(raw, &req(&ws)).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].span,
            Span::new(Ts::from_millis(600), Ts::from_millis(1150))
        );
    }

    #[test]
    fn rejects_garbage() {
        let ws = words("a b c");
        assert!(parse_response("no json here", &req(&ws)).is_err());
    }

    #[test]
    fn budget() {
        let b = deadline_budget(
            Dur::from_millis(8000),
            Dur::from_millis(1500),
            Dur::from_millis(1500),
        );
        assert_eq!(b, Dur::from_millis(5000));
    }
}
