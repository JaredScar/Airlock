//! Judge providers. Both are stubs: wire HTTP in PLAN.md Phase 3.
//!
//! - `OllamaJudge` (default, local): POST {base_url}/api/chat with `format: "json"`,
//!   `stream: false`, `options.temperature: 0`. Suggested models: qwen3 8B/14B or
//!   llama3.1 8B instruct; measure p95 latency against `deadline_budget`.
//! - `AnthropicJudge` (opt-in cloud): Messages API. Look up current model ids and
//!   request format at https://docs.claude.com before implementing; don't hardcode
//!   from memory. Requires explicit user consent in settings, since transcript text
//!   leaves the machine.
//!
//! Both: build the prompt with `crate::build_prompt`, parse with `crate::parse_response`,
//! and enforce the deadline with an HTTP client timeout.

use crate::{ContextJudge, JudgeError, JudgeRequest};
use airlock_core::Detection;

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
    fn judge(&self, _req: &JudgeRequest<'_>) -> Result<Vec<Detection>, JudgeError> {
        Err(JudgeError::NotImplemented("OllamaJudge::judge"))
    }
}

#[derive(Debug, Clone)]
pub struct AnthropicJudge {
    /// Loaded from the OS keychain at runtime, never from config files.
    pub api_key: String,
    pub model: String,
}

impl ContextJudge for AnthropicJudge {
    fn name(&self) -> &str {
        "anthropic"
    }
    fn judge(&self, _req: &JudgeRequest<'_>) -> Result<Vec<Detection>, JudgeError> {
        Err(JudgeError::NotImplemented("AnthropicJudge::judge"))
    }
}
