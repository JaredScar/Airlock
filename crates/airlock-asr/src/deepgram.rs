//! Opt-in Deepgram streaming speech recognition.
//!
//! Audio leaves this PC only when the streamer turns that on and a key is in the
//! keychain. The key is sent as an `Authorization` header and is never copied
//! into an error. Interim results are ignored. A word is emitted once, when a
//! final result includes it.
//!
//! Protocol checked against Deepgram's live listen websocket on 2026-10-31:
//! `wss://api.deepgram.com/v1/listen`, linear16 at 16 kHz, `{"type":"Finalize"}`.

use crate::{AsrError, SpeechRecognizer};
use airlock_core::{Dur, Span, Ts, Word};
use serde::Deserialize;
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const DEFAULT_URL: &str = "wss://api.deepgram.com/v1/listen?model=nova-3&language=en&encoding=linear16&sample_rate=16000&channels=1&interim_results=true";

pub struct DeepgramConfig {
    pub api_key: String,
    pub url: String,
    pub read_timeout: Duration,
}

impl Default for DeepgramConfig {
    fn default() -> Self {
        DeepgramConfig {
            api_key: String::new(),
            url: DEFAULT_URL.into(),
            read_timeout: Duration::from_millis(15),
        }
    }
}

pub struct DeepgramRecognizer {
    config: DeepgramConfig,
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    origin: Option<Ts>,
    committed_until: Ts,
}

impl std::fmt::Debug for DeepgramRecognizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramRecognizer")
            .field("url", &self.config.url)
            .finish_non_exhaustive()
    }
}

impl DeepgramRecognizer {
    pub fn new(config: DeepgramConfig) -> Result<Self, AsrError> {
        if config.api_key.trim().is_empty() {
            return Err(AsrError::Inference("cloud speech key is empty".into()));
        }
        Ok(DeepgramRecognizer {
            config,
            socket: None,
            origin: None,
            committed_until: Ts::ZERO,
        })
    }

    fn ensure(&mut self) -> Result<(), AsrError> {
        if self.socket.is_some() {
            return Ok(());
        }
        let mut request = IntoClientRequest::into_client_request(self.config.url.as_str())
            .map_err(|_| unreachable_speech())?;
        let header = format!("Token {}", self.config.api_key.trim());
        let value = HeaderValue::from_str(&header).map_err(|_| unreachable_speech())?;
        request.headers_mut().insert("Authorization", value);
        let (mut socket, _) = tungstenite::connect(request).map_err(|_| unreachable_speech())?;
        set_read_timeout(&mut socket, self.config.read_timeout)?;
        self.socket = Some(socket);
        Ok(())
    }

    fn drain(&mut self) -> Result<Vec<Word>, AsrError> {
        let Some(socket) = self.socket.as_mut() else {
            return Ok(Vec::new());
        };
        let origin = self.origin.unwrap_or(Ts::ZERO);
        let mut out = Vec::new();
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    out.extend(finals_from_json(&text, origin, self.committed_until));
                }
                Ok(Message::Binary(bytes)) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        out.extend(finals_from_json(text, origin, self.committed_until));
                    }
                }
                Ok(Message::Ping(payload)) => {
                    socket
                        .send(Message::Pong(payload))
                        .map_err(|_| unreachable_speech())?;
                }
                Ok(Message::Close(_)) => break,
                Err(tungstenite::Error::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(tungstenite::Error::ConnectionClosed) => break,
                Err(_) => return Err(unreachable_speech()),
                _ => {}
            }
        }
        if let Some(end) = out.iter().map(|word| word.span.end).max() {
            self.committed_until = self.committed_until.max(end);
        }
        Ok(out)
    }
}

impl SpeechRecognizer for DeepgramRecognizer {
    fn name(&self) -> &str {
        "deepgram"
    }

    fn push(&mut self, samples: &[f32], start: Ts) -> Result<Vec<Word>, AsrError> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        if self.origin.is_none() {
            self.origin = Some(start);
        }
        self.ensure()?;
        let socket = self.socket.as_mut().ok_or_else(unreachable_speech)?;
        socket
            .send(Message::Binary(linear16(samples).into()))
            .map_err(|_| unreachable_speech())?;
        self.drain()
    }

    fn flush(&mut self) -> Result<Vec<Word>, AsrError> {
        let Some(socket) = self.socket.as_mut() else {
            return Ok(Vec::new());
        };
        let _ = socket.send(Message::Text("{\"type\":\"Finalize\"}".into()));
        if let Some(socket) = self.socket.as_mut() {
            let _ = set_read_timeout(socket, Duration::from_millis(400));
        }
        let words = self.drain()?;
        if let Some(socket) = self.socket.as_mut() {
            let _ = socket.send(Message::Text("{\"type\":\"CloseStream\"}".into()));
            let _ = socket.close(None);
        }
        self.socket = None;
        Ok(words)
    }
}

fn unreachable_speech() -> AsrError {
    AsrError::Inference("cloud speech recognition is not reachable".into())
}

fn linear16(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&scaled.to_le_bytes());
    }
    bytes
}

fn set_read_timeout(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<(), AsrError> {
    let tcp = match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => tcp,
        MaybeTlsStream::Rustls(tls) => tls.get_ref(),
        _ => return Ok(()),
    };
    tcp.set_read_timeout(Some(timeout))
        .map_err(|_| unreachable_speech())
}

fn finals_from_json(text: &str, origin: Ts, committed_until: Ts) -> Vec<Word> {
    let Ok(message) = serde_json::from_str::<Results>(text) else {
        return Vec::new();
    };
    if message
        .kind
        .as_deref()
        .is_some_and(|kind| kind != "Results")
        || !message.is_final
    {
        return Vec::new();
    }
    let Some(words) = message
        .channel
        .and_then(|channel| channel.alternatives.into_iter().next())
        .map(|alt| alt.words)
    else {
        return Vec::new();
    };
    words
        .into_iter()
        .filter_map(|word| {
            let text = word.word.trim();
            if text.is_empty() {
                return None;
            }
            let start_s = placed(message.start, word.start);
            let end_s = placed(message.start, word.end).max(start_s);
            let start = at(origin, start_s);
            let mut end = at(origin, end_s);
            if end <= start {
                end = start + Dur(10_000);
            }
            if end <= committed_until {
                return None;
            }
            Some(Word::new(
                text,
                Span::new(start, end),
                word.confidence as f32,
            ))
        })
        .collect()
}

/// Deepgram word times are seconds from the start of the socket. Some payloads
/// instead report them inside the current message, whose `start` is the offset.
fn placed(message_start: f64, word_time: f64) -> f64 {
    if word_time + 0.001 < message_start {
        message_start + word_time
    } else {
        word_time
    }
}

fn at(origin: Ts, seconds: f64) -> Ts {
    let micros = (seconds.max(0.0) * 1_000_000.0).round() as i64;
    origin + Dur(micros)
}

#[derive(Deserialize)]
struct Results {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    start: f64,
    channel: Option<Channel>,
}

#[derive(Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Deserialize)]
struct Alternative {
    #[serde(default)]
    words: Vec<HeardWord>,
}

#[derive(Deserialize)]
struct HeardWord {
    word: String,
    start: f64,
    end: f64,
    #[serde(default)]
    confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn an_empty_key_is_refused() {
        let err = DeepgramRecognizer::new(DeepgramConfig {
            api_key: "  ".into(),
            ..DeepgramConfig::default()
        })
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "inference failed: cloud speech key is empty"
        );
    }

    #[test]
    fn interim_results_are_ignored_and_a_final_word_is_not_emitted_twice() {
        let raw = r#"{"type":"Results","is_final":false,"start":0,"channel":{"alternatives":[{"words":[{"word":"hello","start":0.1,"end":0.4,"confidence":0.4}]}]}}"#;
        assert!(finals_from_json(raw, Ts::ZERO, Ts::ZERO).is_empty());
        let raw = r#"{"type":"Results","is_final":true,"start":1.0,"channel":{"alternatives":[{"words":[{"word":"there","start":0.2,"end":0.5,"confidence":0.8}]}]}}"#;
        let words = finals_from_json(raw, Ts::from_millis(5_000), Ts::ZERO);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "there");
        assert_eq!(words[0].span.start, Ts::from_millis(6_200));
        assert!(finals_from_json(raw, Ts::from_millis(5_000), words[0].span.end).is_empty());
    }

    #[test]
    fn a_local_socket_returns_a_word_and_hides_the_key() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            loop {
                match socket.read() {
                    Ok(Message::Binary(_)) => {
                        let body = r#"{"type":"Results","is_final":true,"start":0,"channel":{"alternatives":[{"words":[{"word":"hello","start":0.1,"end":0.4,"confidence":0.91}]}]}}"#;
                        socket.send(Message::Text(body.into())).unwrap();
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(Message::Text(_)) => break,
                    _ => {}
                }
            }
        });
        let mut recognizer = DeepgramRecognizer::new(DeepgramConfig {
            api_key: "local-test-key".into(),
            url: format!("ws://127.0.0.1:{port}/v1/listen"),
            read_timeout: Duration::from_millis(500),
        })
        .unwrap();
        let words = recognizer
            .push(&[0.2; 1_600], Ts::from_millis(1_000))
            .unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "hello");
        assert_eq!(words[0].span.start, Ts::from_millis(1_100));
        let mut closed = DeepgramRecognizer::new(DeepgramConfig {
            api_key: "local-test-key".into(),
            url: "ws://127.0.0.1:1/v1/listen".into(),
            read_timeout: Duration::from_millis(50),
        })
        .unwrap();
        let err = closed.push(&[0.2; 160], Ts::ZERO).unwrap_err();
        assert!(!err.to_string().contains("local-test-key"));
    }
}
