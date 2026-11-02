//! Remote moderator. During the delay, a computer that has the password can
//! approve or veto. The queue is the category, the time, and masked evidence.
//! Private terms, the transcript, and stream keys are not on this socket.

use crate::Session;
use airlock_core::RedactionId;
use serde::Serialize;
use serde_json::Value;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tungstenite::Message;

const MAX_CLIENTS: usize = 4;
const AUTH_REJECTED: &str = "moderator password was not accepted";
const NOT_UNDERSTOOD: &str = "moderator command was not understood";

#[derive(Debug, Clone, Copy)]
pub enum ModeratorCommand {
    Approve(RedactionId),
    Veto(RedactionId),
}

#[derive(Debug, Serialize)]
pub struct ModeratorQueue {
    pub live_us: i64,
    pub playhead_us: i64,
    pub items: Vec<ModeratorItem>,
}

#[derive(Debug, Serialize)]
pub struct ModeratorItem {
    pub id: u64,
    pub category: String,
    pub status: String,
    pub mode: String,
    pub action: String,
    pub start_us: i64,
    pub end_us: i64,
    pub evidence: Vec<String>,
    pub reasons: Vec<String>,
}

pub fn moderator_queue(session: &Session) -> ModeratorQueue {
    let playhead = session.playhead();
    let items = session
        .engine()
        .all()
        .iter()
        .filter(|item| !item.missed_live && item.span.end > playhead)
        .map(|item| ModeratorItem {
            id: item.id,
            category: snake(&item.category),
            status: snake(&item.status),
            mode: snake(&item.mode),
            action: snake(&item.action),
            start_us: item.span.start.0,
            end_us: item.span.end.0,
            evidence: item.evidence.clone(),
            reasons: item.reasons.clone(),
        })
        .collect();
    ModeratorQueue {
        live_us: session.live_edge().0,
        playhead_us: playhead.0,
        items,
    }
}

pub fn apply_moderator(session: &mut Session, command: ModeratorCommand) -> Result<(), String> {
    let id = match command {
        ModeratorCommand::Approve(id) | ModeratorCommand::Veto(id) => id,
    };
    let playhead = session.playhead();
    let Some(item) = session.engine().all().iter().find(|item| item.id == id) else {
        return Err("that item is no longer in the hold".into());
    };
    if item.missed_live || item.span.end <= playhead {
        return Err("that item has already aired".into());
    }
    let result = match command {
        ModeratorCommand::Approve(_) => session.approve(id),
        ModeratorCommand::Veto(_) => session.veto(id),
    };
    result.map_err(|_| "that item is no longer in the hold".to_string())
}

/// Approve or veto. Any other text, including an auth frame, is refused with a
/// fixed sentence that does not repeat the text.
pub fn parse_command(text: &str) -> Result<ModeratorCommand, String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Err(NOT_UNDERSTOOD.into());
    };
    let op = value.get("op").and_then(|item| item.as_str()).unwrap_or("");
    let id = value.get("id").and_then(|item| item.as_u64()).unwrap_or(0);
    match op {
        "approve" => Ok(ModeratorCommand::Approve(id)),
        "veto" => Ok(ModeratorCommand::Veto(id)),
        _ => Err(NOT_UNDERSTOOD.into()),
    }
}

/// Serves one listener until `generation` is no longer `mine`. An empty password
/// returns without accepting. The caller owns joining this thread.
pub fn serve_moderator(
    listener: TcpListener,
    slot: Arc<Mutex<Option<Session>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
    token: String,
) {
    let token = token.trim().to_string();
    if token.is_empty() {
        return;
    }
    let _ = listener.set_nonblocking(true);
    let mut children: Vec<JoinHandle<()>> = Vec::new();
    while generation.load(Ordering::SeqCst) == mine {
        reap(&mut children);
        match listener.accept() {
            Ok((stream, _)) => {
                let running = children.iter().filter(|child| !child.is_finished()).count();
                if running >= MAX_CLIENTS {
                    drop(stream);
                    continue;
                }
                let slot = slot.clone();
                let generation = generation.clone();
                let token = token.clone();
                children.push(thread::spawn(move || {
                    let _ = stream.set_nonblocking(false);
                    talk(stream, &slot, &generation, mine, &token);
                }));
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    for child in children {
        let _ = child.join();
    }
}

fn reap(children: &mut Vec<JoinHandle<()>>) {
    let pending = std::mem::take(children);
    for child in pending {
        if child.is_finished() {
            let _ = child.join();
        } else {
            children.push(child);
        }
    }
}

fn talk(
    stream: std::net::TcpStream,
    slot: &Mutex<Option<Session>>,
    generation: &AtomicU64,
    mine: u64,
    token: &str,
) {
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    let _ = socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(8)));
    let Ok(message) = socket.read() else {
        return;
    };
    let Message::Text(text) = message else {
        return;
    };
    if !password_accepted(&text, token) {
        let _ = socket.send(Message::Text(error_body(AUTH_REJECTED).into()));
        let _ = socket.close(None);
        return;
    }
    if socket.send(Message::Text(queue_body(slot).into())).is_err() {
        return;
    }
    let _ = socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(400)));
    loop {
        if generation.load(Ordering::SeqCst) != mine {
            break;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let reply = match parse_command(&text) {
                    Ok(command) => apply_locked(slot, command),
                    Err(detail) => error_body(&detail),
                };
                if socket.send(Message::Text(reply.into())).is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(err) if timed_out(&err) => {
                if generation.load(Ordering::SeqCst) != mine {
                    break;
                }
                if socket.send(Message::Text(queue_body(slot).into())).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn password_accepted(text: &str, token: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return tokens_match(token, "");
    };
    if value.get("op").and_then(|item| item.as_str()) != Some("auth") {
        return tokens_match(token, "");
    }
    let given = value
        .get("token")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    tokens_match(token, given.trim())
}

fn tokens_match(stored: &str, given: &str) -> bool {
    let left = stored.as_bytes();
    let right = given.as_bytes();
    let mut diff = left.len() ^ right.len();
    let n = left.len().max(right.len());
    for i in 0..n {
        let a = left.get(i).copied().unwrap_or(0);
        let b = right.get(i).copied().unwrap_or(0);
        diff |= usize::from(a ^ b);
    }
    diff == 0 && !left.is_empty()
}

fn apply_locked(slot: &Mutex<Option<Session>>, command: ModeratorCommand) -> String {
    let Ok(mut guard) = slot.lock() else {
        return error_body("the broadcast is not ready");
    };
    let Some(session) = guard.as_mut() else {
        return error_body("the broadcast is not ready");
    };
    if let Err(detail) = apply_moderator(session, command) {
        return error_body(&detail);
    }
    queue_json(session)
}

fn queue_body(slot: &Mutex<Option<Session>>) -> String {
    let Ok(guard) = slot.lock() else {
        return empty_queue();
    };
    let Some(session) = guard.as_ref() else {
        return empty_queue();
    };
    queue_json(session)
}

fn queue_json(session: &Session) -> String {
    let mut value = serde_json::to_value(moderator_queue(session))
        .unwrap_or_else(|_| Value::Object(Default::default()));
    if let Some(obj) = value.as_object_mut() {
        obj.insert("ok".into(), Value::Bool(true));
    }
    value.to_string()
}

fn empty_queue() -> String {
    serde_json::json!({
        "ok": true,
        "live_us": 0,
        "playhead_us": 0,
        "items": []
    })
    .to_string()
}

fn error_body(detail: &str) -> String {
    serde_json::json!({ "ok": false, "detail": detail }).to_string()
}

fn snake<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(text)) => text,
        _ => String::new(),
    }
}

fn timed_out(err: &tungstenite::Error) -> bool {
    match err {
        tungstenite::Error::Io(io) => {
            matches!(io.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionConfig, StudioSettings};
    use airlock_core::{Span, Status, Ts, Word};
    use std::net::TcpStream;
    use tungstenite::stream::MaybeTlsStream;

    const TOKEN: &str = "moderator-secret-token";
    const TERM: &str = "super-secret-town";

    fn session_with_hit() -> (Session, u64) {
        let cfg = SessionConfig {
            profanity_list: "airlocktest\n".into(),
            private_terms: vec![TERM.into()],
            ..SessionConfig::default()
        };
        let mut session = Session::new(cfg);
        session.on_words(vec![Word::new(
            "airlocktest",
            Span::from_millis(0, 400),
            0.99,
        )]);
        let id = session.engine().all()[0].id;
        (session, id)
    }

    #[test]
    fn the_queue_hides_a_private_term_and_a_veto_after_air_is_refused() {
        let (mut session, id) = session_with_hit();
        let queue = moderator_queue(&session);
        let json = serde_json::to_string(&queue).unwrap();
        assert!(json.contains("profanity"));
        assert!(!json.contains(TERM));
        assert!(!json.contains("transcript"));
        apply_moderator(&mut session, ModeratorCommand::Veto(id)).unwrap();
        assert_eq!(session.engine().all()[0].status, Status::Vetoed);
        assert!(!session.engine().all()[0].is_active());

        session.on_live_time(Ts::from_millis(30_000));
        let err = apply_moderator(&mut session, ModeratorCommand::Veto(id)).unwrap_err();
        assert_eq!(err, "that item is no longer in the hold");
        assert_eq!(
            parse_command(TERM).unwrap_err(),
            "moderator command was not understood"
        );
    }

    #[test]
    fn an_old_settings_file_leaves_the_moderator_off() {
        let mut value = serde_json::to_value(StudioSettings::default()).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("moderator_enabled");
        obj.remove("moderator_port");
        let loaded: StudioSettings = serde_json::from_value(value).unwrap();
        assert!(!loaded.moderator_enabled);
        assert_eq!(loaded.moderator_port, 1940);
    }

    #[test]
    fn a_wrong_password_is_not_echoed_and_the_right_one_can_veto() {
        let (session, id) = session_with_hit();
        let slot = Arc::new(Mutex::new(Some(session)));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let generation = Arc::new(AtomicU64::new(1));
        let generation_thread = generation.clone();
        let slot_thread = slot.clone();
        let server = thread::spawn(move || {
            serve_moderator(listener, slot_thread, generation_thread, 1, TOKEN.into());
        });

        let mut rejected = connect_retry(port);
        rejected
            .send(Message::Text(
                r#"{"op":"auth","token":"nope"}"#.to_string().into(),
            ))
            .unwrap();
        let refusal = next_text(&mut rejected);
        assert!(refusal.contains("not accepted"));
        assert!(!refusal.contains(TOKEN));
        assert!(!refusal.contains(TERM));
        drop(rejected);

        let mut socket = connect_retry(port);
        socket
            .send(Message::Text(
                format!(r#"{{"op":"auth","token":"{TOKEN}"}}"#).into(),
            ))
            .unwrap();
        let hello = next_text(&mut socket);
        assert!(hello.contains("profanity"));
        assert!(!hello.contains(TOKEN));
        assert!(!hello.contains(TERM));
        assert!(!hello.contains("transcript"));
        socket
            .send(Message::Text(
                format!(r#"{{"op":"veto","id":{id}}}"#).into(),
            ))
            .unwrap();
        let mut vetoed = false;
        for _ in 0..8 {
            let text = next_text(&mut socket);
            if text.contains("vetoed") {
                assert!(!text.contains(TOKEN));
                assert!(!text.contains(TERM));
                vetoed = true;
                break;
            }
        }
        assert!(vetoed);
        {
            let guard = slot.lock().unwrap();
            let session = guard.as_ref().unwrap();
            assert_eq!(session.engine().all()[0].status, Status::Vetoed);
        }
        let _ = socket.close(None);
        drop(socket);
        generation.fetch_add(1, Ordering::SeqCst);
        server.join().expect("moderator server stopped");
    }

    #[test]
    fn password_compare_rejects_a_prefix_and_an_empty_password() {
        assert!(tokens_match("correct-horse", "correct-horse"));
        assert!(!tokens_match("correct-horse", "correct"));
        assert!(!tokens_match("correct-horse", "correct-horse-extra"));
        assert!(!tokens_match("", ""));
        assert!(!tokens_match("correct-horse", ""));
    }

    #[test]
    fn an_empty_moderator_password_does_not_listen() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        serve_moderator(
            listener,
            Arc::new(Mutex::new(None)),
            Arc::new(AtomicU64::new(1)),
            1,
            "  ".into(),
        );
        assert!(
            TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(200),
            )
            .is_err()
        );
    }

    fn connect_retry(port: u16) -> tungstenite::WebSocket<MaybeTlsStream<TcpStream>> {
        let url = format!("ws://127.0.0.1:{port}");
        for _ in 0..50 {
            if let Ok((socket, _)) = tungstenite::connect(&url) {
                return socket;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("moderator socket did not open");
    }

    fn next_text(socket: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>) -> String {
        for _ in 0..20 {
            match socket.read() {
                Ok(Message::Text(text)) => return text.to_string(),
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Close(_)) => continue,
                Ok(_) => continue,
                Err(err) if timed_out(&err) => continue,
                Err(_) => return String::new(),
            }
        }
        String::new()
    }
}
