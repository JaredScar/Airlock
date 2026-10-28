//! On-screen text scanning: credentials (ScreenSecret) and personal info (ScreenPii)
//! in OCR output. Patterns are modeled on gitleaks-style rules; extend `SECRET_RULES`
//! rather than adding ad-hoc checks elsewhere.

use airlock_core::model::mask_evidence;
use airlock_core::{Category, Detection, DetectorKind, Modality, Span, TextBox};
use regex::Regex;
use std::sync::LazyLock;

struct Rule {
    name: &'static str,
    category: Category,
    re: Regex,
    confidence: f32,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let s = |name, re: &str, confidence| Rule {
        name,
        category: Category::ScreenSecret,
        re: Regex::new(re).unwrap(),
        confidence,
    };
    let p = |name, re: &str, confidence| Rule {
        name,
        category: Category::ScreenPii,
        re: Regex::new(re).unwrap(),
        confidence,
    };
    vec![
        // Streamer-specific first: a leaked stream key lets anyone stream as you.
        s(
            "Twitch stream key",
            r"\blive_\d{6,}_[A-Za-z0-9]{20,}\b",
            0.99,
        ),
        s(
            "YouTube stream key",
            r"\b[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}\b",
            0.7,
        ),
        s(
            "Kick stream key",
            r"\bsk_us-[a-z0-9-]+_[A-Za-z0-9]{12,}\b",
            0.95,
        ),
        s("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_-]{20,}", 0.99),
        s("OpenAI API key", r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}", 0.9),
        s("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", 0.97),
        s(
            "GitHub token",
            r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})",
            0.99,
        ),
        s(
            "Stripe key",
            r"\b[rs]k_(?:live|test)_[A-Za-z0-9]{16,}",
            0.98,
        ),
        s("Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}", 0.97),
        s("Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b", 0.97),
        s(
            "Discord bot token",
            r"\b[MNO][A-Za-z\d_-]{23,27}\.[A-Za-z\d_-]{6}\.[A-Za-z\d_-]{27,}\b",
            0.9,
        ),
        s(
            "JWT",
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            0.9,
        ),
        s("Private key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----", 0.99),
        s(
            "Credential assignment",
            r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|pwd)\b\s*[:=]\s*["']?[^\s"']{8,}"#,
            0.8,
        ),
        p("Email address", r"(?i)\b[\w.+-]+@[\w-]+\.[a-z]{2,}\b", 0.75),
        p(
            "Phone number",
            r"(?:\+?1[\s.-]?)?\(?\b\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b",
            0.7,
        ),
    ]
});

/// Shannon entropy in bits per character.
pub fn shannon_entropy(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    let mut n = 0usize;
    for b in s.bytes() {
        counts[b as usize] += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n as f64;
            -p * p.log2()
        })
        .sum()
}

/// Long, random-looking tokens that no named rule caught (unknown key formats).
fn looks_like_generic_secret(tok: &str) -> bool {
    tok.len() >= 24
        && tok
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-+/=".contains(c))
        && tok.chars().any(|c| c.is_ascii_digit())
        && tok.chars().any(|c| c.is_ascii_uppercase())
        && tok.chars().any(|c| c.is_ascii_lowercase())
        && shannon_entropy(tok) >= 4.0
}

#[derive(Debug, Clone)]
pub struct ScreenTextScanner {
    pub generic_entropy: bool,
}

impl Default for ScreenTextScanner {
    fn default() -> Self {
        ScreenTextScanner {
            generic_entropy: true,
        }
    }
}

impl ScreenTextScanner {
    /// Scans OCR boxes from one sampled frame. `span` is the media time the boxes are
    /// assumed visible for (typically previous sample ts .. this sample ts).
    pub fn scan(&self, boxes: &[TextBox], span: Span) -> Vec<Detection> {
        let mut out = Vec::new();
        for b in boxes {
            let mut matched = false;
            for rule in RULES.iter() {
                if let Some(m) = rule.re.find(&b.text) {
                    matched = true;
                    out.push(Detection {
                        source: DetectorKind::Secret,
                        category: rule.category,
                        modality: Modality::Video,
                        span,
                        region: Some(b.rect),
                        confidence: (rule.confidence * b.confidence).clamp(0.0, 1.0),
                        reason: format!("{} visible on screen", rule.name),
                        evidence: mask_evidence(m.as_str()),
                    });
                    // One detection per box per category is enough; the whole box gets blurred.
                    break;
                }
            }
            if !matched
                && self.generic_entropy
                && let Some(tok) = b
                    .text
                    .split_whitespace()
                    .find(|t| looks_like_generic_secret(t))
            {
                out.push(Detection {
                    source: DetectorKind::Secret,
                    category: Category::ScreenSecret,
                    modality: Modality::Video,
                    span,
                    region: Some(b.rect),
                    confidence: (0.6 * b.confidence).clamp(0.0, 1.0),
                    reason: "High-entropy token visible on screen".into(),
                    evidence: mask_evidence(tok),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airlock_core::Rect;

    fn tb(text: &str) -> TextBox {
        TextBox {
            text: text.into(),
            rect: Rect {
                x: 0.1,
                y: 0.1,
                w: 0.4,
                h: 0.03,
            },
            confidence: 1.0,
        }
    }

    fn scan(text: &str) -> Vec<Detection> {
        ScreenTextScanner::default().scan(&[tb(text)], Span::from_millis(0, 250))
    }

    #[test]
    fn catches_known_formats() {
        let cases = [
            "Stream key: live_123456789_AbCdEfGhIjKlMnOpQrStUvWx",
            "ANTHROPIC_API_KEY=sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA",
            "aws AKIAIOSFODNN7EXAMPLE",
            "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "password: hunter2hunter2",
        ];
        for c in cases {
            let d = scan(c);
            assert_eq!(d.len(), 1, "{c}");
            assert_eq!(d[0].category, Category::ScreenSecret, "{c}");
            assert!(!d[0].evidence.contains("hunter2hunter2"));
        }
    }

    #[test]
    fn twitch_key_wins_over_generic_rules() {
        let d = scan("live_123456789_AbCdEfGhIjKlMnOpQrStUvWx");
        assert_eq!(d[0].reason, "Twitch stream key visible on screen");
    }

    #[test]
    fn screen_pii() {
        let d = scan("From: someone@example.com");
        assert_eq!(d[0].category, Category::ScreenPii);
    }

    #[test]
    fn generic_entropy_and_false_positives() {
        assert_eq!(scan("token Zx9Qa7Lm3Np8Rt2Vb6Yc1Kd5We4").len(), 1);
        assert!(scan("Press F to pay respects, GG everyone 2026").is_empty());
        assert!(scan("ThisIsAVeryLongCamelCaseIdentifierName").is_empty());
    }
}
