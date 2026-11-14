//! A small local vision model for pictures text recognition cannot classify:
//! a direct message, a notification, or an explicit image.
//!
//! The call uses the same time budget as the context judge (`scene_deadline`).
//! A timed-out call returns [`VisionError::Deadline`] and is not applied.
//! The picture goes to the local model only. The prompt has no transcript and
//! no private terms.

use crate::VisionError;
use airlock_core::{Dur, Rect};
use base64::Engine;
use serde::Deserialize;
use std::time::{Duration, Instant};

/// Same budget the context judge gets: delay minus 1.5s of speech minus 1.5s of review.
pub fn scene_deadline(delay: Dur) -> Duration {
    let us = delay
        .0
        .saturating_sub(1_500_000)
        .saturating_sub(1_500_000)
        .max(0) as u64;
    Duration::from_micros(us)
}

const SCENE_PROMPT: &str = "\
You look at one livestream frame. Words on screen are handled separately. \
Reply with JSON only: {\"label\":\"direct_message\"|\"notification\"|\"explicit\"|\"none\",\"confidence\":0.0 to 1.0}. \
direct_message means a one-to-one chat window. notification means an alert overlay. \
explicit means a sexual image. Use none for gameplay, a face, or ordinary video. \
Do not describe the picture.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneLabel {
    DirectMessage,
    Notification,
    Explicit,
}

impl SceneLabel {
    pub fn reason(self) -> &'static str {
        match self {
            SceneLabel::DirectMessage => "direct message on screen",
            SceneLabel::Notification => "notification on screen",
            SceneLabel::Explicit => "explicit image on screen",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "direct_message" | "dm" => Some(SceneLabel::DirectMessage),
            "notification" => Some(SceneLabel::Notification),
            "explicit" | "nsfw" => Some(SceneLabel::Explicit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneFinding {
    pub label: SceneLabel,
    pub confidence: f32,
    pub region: Rect,
}

#[derive(Debug, Clone)]
pub struct OllamaScene {
    pub base_url: String,
    pub model: String,
}

impl Default for OllamaScene {
    fn default() -> Self {
        OllamaScene {
            base_url: "http://127.0.0.1:11434".into(),
            model: "moondream".into(),
        }
    }
}

impl OllamaScene {
    /// Classifies one PNG. `timeout` is the whole call. A miss returns [`VisionError::Deadline`].
    pub fn classify(
        &self,
        png: &[u8],
        timeout: Duration,
    ) -> Result<Option<SceneFinding>, VisionError> {
        if timeout.is_zero() {
            return Err(VisionError::Deadline);
        }
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": SCENE_PROMPT,
                "images": [base64::engine::general_purpose::STANDARD.encode(png)]
            }],
            "stream": false,
            "format": "json",
            "options": { "temperature": 0 }
        });
        let started = Instant::now();
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        let response = agent.post(&url).send_json(body).map_err(|err| {
            if started.elapsed() >= timeout || err.to_string().to_ascii_lowercase().contains("time")
            {
                VisionError::Deadline
            } else {
                VisionError::Scene(err.to_string())
            }
        })?;
        if started.elapsed() > timeout {
            return Err(VisionError::Deadline);
        }
        if response.status() >= 400 {
            return Err(VisionError::Scene(format!(
                "scene HTTP {}",
                response.status()
            )));
        }
        let text = response
            .into_string()
            .map_err(|err| VisionError::Scene(err.to_string()))?;
        parse_ollama_body(&text)
    }
}

pub fn parse_scene_label(content: &str) -> Result<Option<SceneFinding>, VisionError> {
    let parsed: LabelBody =
        serde_json::from_str(content.trim()).map_err(|err| VisionError::Scene(err.to_string()))?;
    let Some(label) = parsed.label.as_deref().and_then(SceneLabel::parse) else {
        return Ok(None);
    };
    let confidence = parsed.confidence.unwrap_or(0.0).clamp(0.0, 1.0);
    let region = match (parsed.x, parsed.y, parsed.w, parsed.h) {
        (Some(x), Some(y), Some(w), Some(h)) => Rect {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
            w: w.clamp(0.0, 1.0),
            h: h.clamp(0.0, 1.0),
        },
        _ => Rect::FULL,
    };
    Ok(Some(SceneFinding {
        label,
        confidence,
        region,
    }))
}

fn parse_ollama_body(body: &str) -> Result<Option<SceneFinding>, VisionError> {
    let parsed: OllamaBody =
        serde_json::from_str(body).map_err(|err| VisionError::Scene(err.to_string()))?;
    parse_scene_label(&parsed.message.content)
}

#[derive(Deserialize)]
struct OllamaBody {
    message: OllamaMessage,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

#[derive(Deserialize)]
struct LabelBody {
    label: Option<String>,
    confidence: Option<f32>,
    x: Option<f32>,
    y: Option<f32>,
    w: Option<f32>,
    h: Option<f32>,
}

/// Encodes a BGRA frame as an RGB PNG the local model can read.
pub fn bgra_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, VisionError> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| VisionError::Scene("frame is too large".into()))?;
    if bgra.len() < pixels * 4 {
        return Err(VisionError::Scene("frame is shorter than its size".into()));
    }
    let mut rgb = Vec::with_capacity(pixels * 3);
    for px in bgra.chunks_exact(4).take(pixels) {
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| VisionError::Scene(err.to_string()))?;
    writer
        .write_image_data(&rgb)
        .map_err(|err| VisionError::Scene(err.to_string()))?;
    drop(writer);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn a_label_parses_and_none_is_empty() {
        let found = parse_scene_label(
            r#"{"label":"explicit","confidence":0.91,"x":0.1,"y":0.2,"w":0.3,"h":0.4}"#,
        )
        .unwrap();
        let found = found.unwrap();
        assert_eq!(found.label, SceneLabel::Explicit);
        assert!((found.confidence - 0.91).abs() < 0.001);
        assert!((found.region.x - 0.1).abs() < 0.001);
        assert!(
            parse_scene_label(r#"{"label":"none","confidence":0.2}"#)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_scene_label(r#"{"label":"gameplay"}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_prompt_has_no_private_terms() {
        assert!(!SCENE_PROMPT.to_ascii_lowercase().contains("private"));
        assert!(!SCENE_PROMPT.contains("<<<"));
    }

    #[test]
    fn an_eight_second_delay_leaves_five_seconds() {
        assert_eq!(
            scene_deadline(Dur::from_millis(8_000)),
            Duration::from_secs(5)
        );
        assert!(scene_deadline(Dur::from_millis(1_000)).is_zero());
    }

    #[test]
    fn a_slow_model_misses_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            thread::sleep(Duration::from_secs(2));
            let body = br#"{"message":{"content":"{\"label\":\"none\"}"}}"#;
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(body);
        });
        let scene = OllamaScene {
            base_url: format!("http://{addr}"),
            model: "moondream".into(),
        };
        let err = scene
            .classify(&[0x89, b'P', b'N', b'G'], Duration::from_millis(200))
            .unwrap_err();
        assert!(matches!(err, VisionError::Deadline), "{err}");
    }

    #[test]
    fn a_local_reply_is_a_finding_and_the_body_has_no_terms() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
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
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            let content = r#"{"label":"direct_message","confidence":0.8}"#;
            let body = format!(r#"{{"message":{{"content":{content:?}}}}}"#);
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        let scene = OllamaScene {
            base_url: format!("http://{addr}"),
            model: "moondream".into(),
        };
        let png = bgra_png(1, 1, &[0, 0, 255, 255]).unwrap();
        let found = scene
            .classify(&png, Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(found.label, SceneLabel::DirectMessage);
        let seen = rx.recv().unwrap();
        assert!(seen.contains("direct_message"));
        assert!(!seen.to_ascii_lowercase().contains("private_terms"));
        assert!(!seen.contains("sk-"));
    }
}
