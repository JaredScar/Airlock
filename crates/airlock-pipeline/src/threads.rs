//! Channels into the one thread that owns `Session`.
//!
//! Workers send [`SessionInbox`] values. They never share the engine. After each
//! message the session thread publishes an immutable [`OutputSnapshot`] the output
//! loop can read. The mutex only swaps that snapshot.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use airlock_core::{Detection, Rect, Redaction, RedactionId, Span, TextBox, Ts, Word};

use crate::{JudgeWindow, Session, UiEvent};

#[derive(Debug)]
pub enum SessionInbox {
    Words(Vec<Word>),
    Screen {
        boxes: Vec<TextBox>,
        span: Span,
    },
    /// A vision-model finding for one sampled frame. `reason` is a short label,
    /// never image bytes or private terms.
    Scene {
        span: Span,
        region: Rect,
        confidence: f32,
        reason: String,
    },
    Judge {
        window: JudgeWindow,
        detections: Vec<Detection>,
    },
    Live(Ts),
    Energy(bool),
    Approve(RedactionId),
    Veto(RedactionId),
    Dump {
        blackout_video: bool,
    },
    SafeMode(bool),
    Shutdown,
}

/// Redactions the output loop may apply without touching the session.
#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    pub live: Ts,
    pub playhead: Ts,
    pub redactions: Vec<Redaction>,
}

pub struct SessionBus {
    tx: Sender<SessionInbox>,
    published: Arc<Mutex<Arc<OutputSnapshot>>>,
    events: Mutex<Receiver<Vec<UiEvent>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl SessionBus {
    pub fn start(session: Session) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        let published = Arc::new(Mutex::new(Arc::new(snapshot_of(&session))));
        let published_thread = Arc::clone(&published);
        let join = thread::spawn(move || {
            let mut session = session;
            while let Ok(msg) = rx.recv() {
                if !dispatch(&mut session, msg) {
                    break;
                }
                let events = session.drain_events();
                if !events.is_empty() {
                    let _ = events_tx.send(events);
                }
                // Poison means this thread already panicked. It is the only writer.
                *published_thread.lock().expect("session snapshot lock") =
                    Arc::new(snapshot_of(&session));
            }
        });
        SessionBus {
            tx,
            published,
            events: Mutex::new(events_rx),
            join: Mutex::new(Some(join)),
        }
    }

    pub fn send(&self, msg: SessionInbox) -> bool {
        self.tx.send(msg).is_ok()
    }

    pub fn snapshot(&self) -> Arc<OutputSnapshot> {
        Arc::clone(&self.published.lock().expect("session snapshot lock"))
    }

    pub fn drain_events(&self) -> Vec<UiEvent> {
        let rx = self.events.lock().expect("session event lock");
        let mut out = Vec::new();
        while let Ok(batch) = rx.try_recv() {
            out.extend(batch);
        }
        out
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(SessionInbox::Shutdown);
        if let Some(handle) = self.join.lock().expect("session join lock").take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SessionBus {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn snapshot_of(session: &Session) -> OutputSnapshot {
    OutputSnapshot {
        live: session.playhead() + session.hold(),
        playhead: session.playhead(),
        redactions: session.engine().all().to_vec(),
    }
}

fn dispatch(session: &mut Session, msg: SessionInbox) -> bool {
    match msg {
        SessionInbox::Words(words) => session.on_words(words),
        SessionInbox::Screen { boxes, span } => session.on_screen_text(&boxes, span),
        SessionInbox::Scene {
            span,
            region,
            confidence,
            reason,
        } => session.on_scene(span, region, confidence, &reason),
        SessionInbox::Judge { window, detections } => session.on_judge_result(&window, detections),
        SessionInbox::Live(ts) => session.on_live_time(ts),
        SessionInbox::Energy(energetic) => session.note_audio_energy(energetic),
        SessionInbox::Approve(id) => {
            let _ = session.approve(id);
        }
        SessionInbox::Veto(id) => {
            let _ = session.veto(id);
        }
        SessionInbox::Dump { blackout_video } => session.dump(blackout_video),
        SessionInbox::SafeMode(on) => session.set_safe_mode(on),
        SessionInbox::Shutdown => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionConfig;
    use airlock_core::{Category, Span, Word};
    use std::time::Duration;

    #[test]
    fn a_worker_message_publishes_a_redaction_without_sharing_the_engine() {
        let bus = SessionBus::start(Session::new(SessionConfig::default()));
        assert!(bus.send(SessionInbox::Words(vec![Word::new(
            "shit",
            Span::from_millis(1_000, 1_300),
            0.99,
        )])));
        let mut seen = None;
        for _ in 0..50 {
            let snap = bus.snapshot();
            if let Some(r) = snap
                .redactions
                .iter()
                .find(|r| r.category == Category::Profanity)
            {
                seen = Some(r.span);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(seen, Some(Span::from_millis(920, 1_380)));
        assert!(!bus.drain_events().is_empty());
        bus.shutdown();
    }

    #[test]
    fn safe_mode_mute_covers_audio_as_time_advances() {
        let mut session = Session::new(SessionConfig::default());
        session.on_live_time(Ts::from_millis(1_000));
        session.set_safe_mode(true);
        session.on_live_time(Ts::from_millis(4_000));
        let covering: Vec<_> = session
            .engine()
            .audio_redactions_in(Span::from_millis(3_900, 4_000))
            .collect();
        assert!(!covering.is_empty());
    }
}
