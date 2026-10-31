//! Records the browser-preview fixture (PLAN 0.14) from the same scripted session the
//! desktop demo plays. The JSON is committed; regenerate with:
//! `cargo test -p airlock-pipeline --test preview_fixture write_preview_fixture -- --ignored`

use airlock_asr::{ScriptedRecognizer, SpeechRecognizer};
use airlock_core::{Category, Detection, DetectorKind, Dur, Modality, Rect, Span, TextBox, Ts};
use airlock_pipeline::{Session, SessionConfig, UiEvent};
use serde::Serialize;
use std::path::PathBuf;

const SCRIPT: &str = "alright chat we are so back today welcome in everybody \
    oh my god what the fuck was that boss fight honestly \
    if you want to join the squad text me at 631-555-0123 and I'll add you \
    shoutout to my old school Springfield High by the way \
    and honestly that guy in chat should get his server ddosed lmao \
    okay let me grab my API key real quick for the bot";

#[derive(Serialize)]
struct TimedEvent {
    at_ms: u64,
    event: UiEvent,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/app/fixtures/demo-events.json")
}

fn record() -> Vec<TimedEvent> {
    let mut session = Session::new(SessionConfig {
        private_terms: vec!["Springfield High".into()],
        ..Default::default()
    });
    let mut asr = ScriptedRecognizer::from_text(SCRIPT, Ts::from_millis(500), 330);
    let chunk = vec![0.0f32; 1_600];
    let mut last_judge = Ts::ZERO;
    let mut showed_key = false;
    let mut out = Vec::new();

    for step in 1..=450 {
        let live = Ts::from_millis(step * 100);
        let words = asr
            .push(&chunk, live - Dur::from_millis(100))
            .expect("scripted recognizer");
        session.on_words(words);
        if live - last_judge >= Dur::from_millis(1_500) {
            last_judge = live;
            if let Some(window) = session.next_judge_window() {
                let flags = demo_judge(&window.words, window.focus_start);
                session.on_judge_result(&window, flags);
            }
        }
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
        for event in session.drain_events() {
            out.push(TimedEvent {
                at_ms: (step * 100) as u64,
                event,
            });
        }
    }
    out
}

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

#[test]
#[ignore = "rewrites the committed browser-preview fixture"]
fn write_preview_fixture() {
    let path = fixture_path();
    std::fs::create_dir_all(path.parent().expect("fixture dir")).expect("mkdir");
    let json = serde_json::to_string_pretty(&record()).expect("json");
    std::fs::write(&path, json + "\n").expect("write fixture");
}

#[test]
fn preview_fixture_covers_the_demo() {
    let path = fixture_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing {path:?} ({e}). Regenerate with the ignored write_preview_fixture test")
    });
    let events: Vec<TimedEventView> = serde_json::from_str(&raw).expect("fixture json");
    assert!(events.len() > 50, "fixture looks empty");
    assert!(events.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
    assert!(events.iter().any(|e| e.event_type() == "transcript"));
    assert!(
        events
            .iter()
            .any(|e| e.event_type() == "redaction_upserted")
    );
    assert!(events.iter().any(|e| e.event_type() == "clock"));
    assert!(events.last().unwrap().at_ms >= 40_000);
}

#[derive(serde::Deserialize)]
struct TimedEventView {
    at_ms: u64,
    event: UiEvent,
}

impl TimedEventView {
    fn event_type(&self) -> &'static str {
        match self.event {
            UiEvent::Transcript { .. } => "transcript",
            UiEvent::RedactionUpserted { .. } => "redaction_upserted",
            UiEvent::RedactionAired { .. } => "redaction_aired",
            UiEvent::Clock { .. } => "clock",
            UiEvent::AnalysisStall { .. } => "analysis_stall",
            UiEvent::Ingest { .. } => "ingest",
            UiEvent::Egress { .. } => "egress",
        }
    }
}
