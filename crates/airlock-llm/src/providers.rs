//! Judge providers.
//!
//! - `OllamaJudge` (default, local): POST `{base_url}/api/chat` with `format: "json"`,
//!   `stream: false`, `options.temperature: 0`. The HTTP timeout is `deadline_budget`.
//! - `AnthropicJudge` (opt-in cloud): Messages API as documented at
//!   <https://platform.claude.com/docs/en/api/messages> on 2026-10-28.
//!   `claude-haiku-5-5` is the fast real-time model on that page. The call is refused
//!   until `consent` is set, because the transcript leaves the machine.
//!   The API key is loaded from the OS keychain by the desktop app; this type only
//!   holds it in memory and never prints it.

use crate::{ContextJudge, JudgeError, JudgeRequest, deadline_budget, parse_response};
use airlock_core::{Detection, Dur};
use serde::Deserialize;
use std::time::Duration;

/// Fastest model the current Messages docs list for high-volume real-time work.
pub const ANTHROPIC_MODEL: &str = "claude-haiku-5-5";
/// `anthropic-version` header from the Messages API example on 2026-10-28.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";

const ASR_ALLOWANCE_MS: i64 = 1_500;
const REVIEW_MARGIN_MS: i64 = 1_500;

#[derive(Debug, Clone)]
pub struct OllamaJudge {
    pub base_url: String,
    pub model: String,
}

impl Default for OllamaJudge {
    fn default() -> Self {
        OllamaJudge {
            base_url: "http://127.0.0.1:11434".into(),
            model: "qwen3:8b".into(),
        }
    }
}

impl ContextJudge for OllamaJudge {
    fn name(&self) -> &str {
        "ollama"
    }

    fn judge(&self, req: &JudgeRequest<'_>) -> Result<Vec<Detection>, JudgeError> {
        let timeout = http_timeout(req.delay)?;
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{ "role": "user", "content": crate::build_prompt(req) }],
            "stream": false,
            "format": "json",
            "options": { "temperature": 0 }
        });
        let text = post_json(&url, timeout, &[], &body)?;
        let parsed: OllamaResponse =
            serde_json::from_str(&text).map_err(|e| JudgeError::BadOutput(e.to_string()))?;
        parse_response(&parsed.message.content, req)
    }
}

#[derive(Clone)]
pub struct AnthropicJudge {
    pub api_key: String,
    pub model: String,
    pub consent: bool,
    /// Full URL of the Messages endpoint. Tests point this at a local server.
    pub endpoint: String,
}

impl std::fmt::Debug for AnthropicJudge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicJudge")
            .field("model", &self.model)
            .field("consent", &self.consent)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl AnthropicJudge {
    pub fn new(api_key: impl Into<String>, consent: bool) -> Self {
        AnthropicJudge {
            api_key: api_key.into(),
            model: ANTHROPIC_MODEL.into(),
            consent,
            endpoint: "https://api.anthropic.com/v1/messages".into(),
        }
    }
}

impl ContextJudge for AnthropicJudge {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn judge(&self, req: &JudgeRequest<'_>) -> Result<Vec<Detection>, JudgeError> {
        if !self.consent {
            return Err(JudgeError::Provider(
                "transcript text stays on this PC until you opt in".into(),
            ));
        }
        if self.api_key.trim().is_empty() {
            return Err(JudgeError::Provider("cloud judge has no API key".into()));
        }
        let timeout = http_timeout(req.delay)?;
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "stream": false,
            "messages": [{ "role": "user", "content": crate::build_prompt(req) }]
        });
        let text = post_json(
            &self.endpoint,
            timeout,
            &[
                ("authorization", &format!("Bearer {}", self.api_key)),
                ("anthropic-version", ANTHROPIC_VERSION),
            ],
            &body,
        )?;
        let parsed: AnthropicResponse =
            serde_json::from_str(&text).map_err(|e| JudgeError::BadOutput(e.to_string()))?;
        let combined = parsed
            .content
            .iter()
            .filter(|b| b.kind == "text")
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        parse_response(&combined, req)
    }
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaMessage,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

fn http_timeout(delay: Dur) -> Result<Duration, JudgeError> {
    let budget = deadline_budget(
        delay,
        Dur::from_millis(ASR_ALLOWANCE_MS),
        Dur::from_millis(REVIEW_MARGIN_MS),
    );
    if budget.0 <= 0 {
        return Err(JudgeError::Timeout);
    }
    Ok(Duration::from_micros(budget.0 as u64))
}

fn post_json(
    url: &str,
    timeout: Duration,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<String, JudgeError> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = request
        .send_json(body)
        .map_err(|_| JudgeError::Provider("judge request failed".into()))?;
    if response.status() >= 400 {
        return Err(JudgeError::Provider(format!(
            "judge HTTP {}",
            response.status()
        )));
    }
    response
        .into_string()
        .map_err(|_| JudgeError::Provider("judge response was not text".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::{Dur, Platform, Span, Word};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    fn words() -> Vec<Word> {
        ["one", "two", "three", "four"]
            .into_iter()
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

    fn request<'a>(ws: &'a [Word]) -> JudgeRequest<'a> {
        JudgeRequest {
            words: ws,
            focus_start: 2,
            platform: Platform::Twitch,
            delay: Dur::from_millis(8_000),
            streamer_rules: &[],
        }
    }

    fn serve(status: u16, body: String) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let seen_thread = Arc::clone(&seen);
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                match sock.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(_) => break,
                }
                if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&buf[..header_end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if buf.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            *seen_thread.lock().unwrap() = String::from_utf8_lossy(&buf).into_owned();
            let resp = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        (format!("http://{addr}"), seen)
    }

    #[test]
    fn ollama_posts_json_format_and_parses_flags() {
        let flags = r#"{"flags":[{"start_word":2,"end_word":3,"category":"tos_risk","confidence":0.9,"reason":"threat"}]}"#;
        let (url, seen) = serve(
            200,
            serde_json::json!({ "message": { "role": "assistant", "content": flags } }).to_string(),
        );
        let judge = OllamaJudge {
            base_url: url,
            model: "qwen3:8b".into(),
        };
        let ws = words();
        let found = judge.judge(&request(&ws)).unwrap();
        assert_eq!(found.len(), 1);
        let posted = seen.lock().unwrap().clone();
        assert!(posted.contains("\"format\":\"json\"") || posted.contains("\"format\": \"json\""));
        assert!(posted.contains("\"temperature\":0") || posted.contains("\"temperature\": 0"));
        assert!(posted.contains("[2] three"));
        assert!(!posted.contains("Springfield"));
    }

    #[test]
    fn anthropic_refuses_without_consent_and_sends_bearer_when_allowed() {
        let ws = words();
        let quiet = AnthropicJudge::new("secret-key-value", false);
        let err = quiet.judge(&request(&ws)).unwrap_err();
        assert!(err.to_string().contains("opt in"));
        assert!(!format!("{quiet:?}").contains("secret-key-value"));

        let flags = r#"{"flags":[{"start_word":2,"end_word":3,"category":"tos_risk","confidence":0.8,"reason":"scam"}]}"#;
        let (url, seen) = serve(
            200,
            serde_json::json!({ "content": [{ "type": "text", "text": flags }] }).to_string(),
        );
        let mut allowed = AnthropicJudge::new("secret-key-value", true);
        allowed.endpoint = format!("{url}/v1/messages");
        let found = allowed.judge(&request(&ws)).unwrap();
        assert_eq!(found.len(), 1);
        let posted = seen.lock().unwrap().clone();
        assert!(
            posted
                .to_ascii_lowercase()
                .contains("authorization: bearer secret-key-value")
        );
        assert!(posted.contains(ANTHROPIC_VERSION));
        assert!(posted.contains(ANTHROPIC_MODEL));
        assert!(posted.contains("[2] three"));
        assert!(!posted.contains("Springfield"));
    }
}
