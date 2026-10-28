//! Simulates a live stream: scripted ASR words arrive as audio is "ingested", the
//! session detects and plans redactions, a fake judge flags context, a fake OCR sees
//! a stream key, and we check what would air.

use airlock_asr::{ScriptedRecognizer, SpeechRecognizer};
use airlock_core::{Action, Category, Detection, DetectorKind, Modality, Rect, Span, TextBox, Ts};
use airlock_pipeline::{Session, SessionConfig, UiEvent, incident};

const SCRIPT: &str = "okay chat what the fuck was that boss \
                      anyway text me at 631-555-0123 if you want in \
                      also shoutout to my buddy from springfield";

fn run_audio(session: &mut Session, asr: &mut ScriptedRecognizer, until_ms: i64) {
    // 100ms chunks of 16 kHz silence; the scripted recognizer only cares about time.
    let chunk = vec![0.0f32; 1_600];
    let mut t = 0;
    while t < until_ms {
        let words = asr.push(&chunk, Ts::from_millis(t)).unwrap();
        session.on_words(words);
        t += 100;
        session.on_live_time(Ts::from_millis(t));
    }
}

#[test]
fn live_session_end_to_end() {
    let cfg = SessionConfig {
        private_terms: vec!["Springfield".into()],
        ..Default::default()
    };
    let mut s = Session::new(cfg);
    let mut asr = ScriptedRecognizer::from_text(SCRIPT, Ts::from_millis(1000), 400);

    // 7 seconds in: nothing has aired yet (8s delay), but redactions are planned.
    run_audio(&mut s, &mut asr, 7_000);
    let upcoming = s.engine().upcoming(s.playhead());
    let cats: Vec<_> = upcoming.iter().map(|r| r.category).collect();
    assert!(cats.contains(&Category::Profanity), "{cats:?}");
    assert!(cats.contains(&Category::SpokenPii), "{cats:?}");

    // "fuck" is word 4 -> 1000 + 4*400 = 2600..2960ms, padded by 80ms.
    let bleep = (*upcoming
        .iter()
        .find(|r| r.category == Category::Profanity)
        .unwrap())
    .clone();
    assert_eq!(bleep.action, Action::Bleep);
    assert_eq!(bleep.span, Span::from_millis(2520, 3040));
    assert!(bleep.is_active());

    // Context judge flags something in the focus window (as a fake LLM would).
    let window = s.next_judge_window().expect("words to judge");
    let flagged = &window.words[window.words.len() - 2..];
    let det = Detection {
        source: DetectorKind::ContextLlm,
        category: Category::TosRisk,
        modality: Modality::Audio,
        span: Span::new(flagged[0].span.start, flagged[1].span.end),
        region: None,
        confidence: 0.9,
        reason: "test flag".into(),
        evidence: String::new(),
    };
    s.on_judge_result(&window, vec![det]);
    let tos = s
        .engine()
        .all()
        .iter()
        .find(|r| r.category == Category::TosRisk)
        .unwrap()
        .clone();
    assert!(!tos.is_active(), "tos_risk is suggest-only by default");
    s.approve(tos.id).unwrap();
    assert!(s.engine().get(tos.id).unwrap().is_active());
    assert!(s.next_judge_window().is_none() || s.next_judge_window().unwrap().focus_start > 0);

    // OCR sees a Twitch stream key on screen.
    let boxes = [TextBox {
        text: "live_123456789_AbCdEfGhIjKlMnOpQrStUvWx".into(),
        rect: Rect {
            x: 0.6,
            y: 0.9,
            w: 0.35,
            h: 0.04,
        },
        confidence: 0.95,
    }];
    s.on_screen_text(&boxes, Span::from_millis(5_750, 6_000));
    assert_eq!(
        s.engine()
            .video_redactions_at(Ts::from_millis(5_900))
            .count(),
        1
    );

    // Veto the bleep (streamer decides it's fine), then let everything air.
    s.veto(bleep.id).unwrap();
    run_audio(&mut s, &mut asr, 40_000);
    s.on_words(asr.flush().unwrap());
    s.on_live_time(Ts::from_millis(60_000));

    let aired = s.aired();
    assert!(aired.iter().any(|r| r.category == Category::ScreenSecret));
    assert!(
        aired
            .iter()
            .any(|r| r.category == Category::SpokenPii && r.reasons[0].contains("Private term"))
    );
    let csv = incident::to_csv(aired);
    assert!(csv.lines().count() > 3);
    assert!(
        !csv.contains("AbCdEfGhIjKl"),
        "secrets must be masked in the incident log"
    );
    assert!(
        !csv.contains("631-555-0123"),
        "PII must be masked in the incident log"
    );

    let events = s.drain_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::RedactionAired { .. }))
    );
    assert!(serde_json::to_string(&events).is_ok());
}
