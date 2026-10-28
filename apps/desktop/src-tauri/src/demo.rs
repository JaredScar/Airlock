//! Demo mode: a scripted 30-second "stream" pushed through the real engine in real
//! time. Exercises every category so the review UI has something to show.
//! The `DemoJudge` here is a keyword stand-in for the LLM; it is NOT the real judge.

use crate::EVENT_CHANNEL;
use airlock_asr::{ScriptedRecognizer, SpeechRecognizer};
use airlock_core::{Category, Detection, DetectorKind, Modality, Rect, Span, TextBox, Ts};
use airlock_pipeline::{Session, SessionConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const SCRIPT: &str = "alright chat we are so back today welcome in everybody \
    oh my god what the fuck was that boss fight honestly \
    if you want to join the squad text me at 631-555-0123 and I'll add you \
    shoutout to my old school Springfield High by the way \
    and honestly that guy in chat should get his server ddosed lmao \
    okay let me grab my API key real quick for the bot";

pub fn start(app: AppHandle, slot: Arc<Mutex<Option<Session>>>) -> Result<(), String> {
    {
        let mut guard = slot.lock().map_err(|_| "lock poisoned")?;
        let cfg = SessionConfig {
            private_terms: vec!["Springfield High".into()],
            ..Default::default()
        };
        *guard = Some(Session::new(cfg));
    }

    std::thread::spawn(move || {
        let mut asr = ScriptedRecognizer::from_text(SCRIPT, Ts::from_millis(500), 330);
        let started = Instant::now();
        let chunk = vec![0.0f32; 1_600]; // 100ms @ 16 kHz
        let mut last_judge = Ts::ZERO;
        let mut showed_key = false;

        loop {
            std::thread::sleep(Duration::from_millis(100));
            let live = Ts::from_millis(started.elapsed().as_millis() as i64);
            let words = asr
                .push(&chunk, live - airlock_core::Dur::from_millis(100))
                .unwrap_or_default();

            let mut guard = match slot.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let Some(session) = guard.as_mut() else {
                return;
            };

            session.on_words(words);

            // Fake LLM judge every 1.5s.
            if live - last_judge >= airlock_core::Dur::from_millis(1_500) {
                last_judge = live;
                if let Some(window) = session.next_judge_window() {
                    let flags = demo_judge(&window.words, window.focus_start);
                    session.on_judge_result(&window, flags);
                }
            }

            // Fake OCR: a "stream key" appears on screen ~26s in, after the streamer says "API key".
            if !showed_key && live >= Ts::from_millis(26_000) {
                showed_key = true;
                let boxes = [TextBox {
                    text: "TWITCH_KEY=live_918273645_q8Zr2LmX9vNc4TpY7wKd".into(),
                    rect: Rect {
                        x: 0.55,
                        y: 0.82,
                        w: 0.4,
                        h: 0.05,
                    },
                    confidence: 0.95,
                }];
                session.on_screen_text(&boxes, Span::from_millis(25_750, 29_000));
            }

            session.on_live_time(live);
            for ev in session.drain_events() {
                let _ = app.emit(EVENT_CHANNEL, ev);
            }
            if live > Ts::from_millis(45_000) {
                return; // everything has aired
            }
        }
    });
    Ok(())
}

/// Stand-in for the LLM: flags "ddos" plus surrounding words as a ToS risk.
fn demo_judge(words: &[airlock_core::Word], focus_start: usize) -> Vec<Detection> {
    words
        .iter()
        .enumerate()
        .skip(focus_start)
        .filter(|(_, w)| w.text.to_lowercase().starts_with("ddos"))
        .map(|(i, w)| {
            let from = &words[i.saturating_sub(5)];
            Detection {
                source: DetectorKind::ContextLlm,
                category: Category::TosRisk,
                modality: Modality::Audio,
                span: Span::new(from.span.start, w.span.end),
                region: None,
                confidence: 0.86,
                reason: "Encouraging an attack on a chatter (demo judge)".into(),
                evidence: String::new(),
            }
        })
        .collect()
}
