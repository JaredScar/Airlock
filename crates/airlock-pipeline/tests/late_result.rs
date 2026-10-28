//! A flag that arrives after its span has aired is kept for VOD cleanup.

use airlock_core::{Category, Detection, DetectorKind, Modality, Span, Ts, Word};
use airlock_pipeline::{Session, SessionConfig};

#[test]
fn late_word_is_missed_and_a_span_still_in_the_hold_is_redacted() {
    let mut session = Session::new(SessionConfig::default());
    session.on_live_time(Ts::from_millis(20_000));
    assert_eq!(session.playhead(), Ts::from_millis(12_000));

    session.on_words(vec![Word::new(
        "fuck",
        Span::from_millis(1_000, 1_400),
        0.99,
    )]);
    assert!(
        session.engine().all().is_empty(),
        "an aired span must not sit in the live list"
    );
    assert_eq!(session.aired().len(), 1);
    let missed = &session.aired()[0];
    assert!(missed.missed_live);
    assert!(!missed.is_active());
    assert_eq!(missed.category, Category::Profanity);
    assert!(missed.reasons.iter().any(|r| r == "Missed live"));

    session.on_words(vec![Word::new(
        "ass",
        Span::from_millis(11_000, 13_000),
        0.99,
    )]);
    let live = session
        .engine()
        .all()
        .iter()
        .find(|r| r.category == Category::Profanity)
        .expect("the tail of the span is still in the hold");
    assert!(!live.missed_live);
    assert!(live.is_active());
    assert!(live.span.end > session.playhead());
}

#[test]
fn late_judge_result_is_filed_and_a_weak_one_is_dropped() {
    let mut session = Session::new(SessionConfig::default());
    session.on_live_time(Ts::from_millis(20_000));
    session.on_words(vec![Word::new(
        "hello",
        Span::from_millis(1_000, 1_400),
        0.99,
    )]);
    let window = session.next_judge_window().expect("a word to judge");

    let late = |confidence: f32, reason: &str| Detection {
        source: DetectorKind::ContextLlm,
        category: Category::TosRisk,
        modality: Modality::Audio,
        span: Span::from_millis(1_000, 1_400),
        region: None,
        confidence,
        reason: reason.into(),
        evidence: String::new(),
    };
    session.on_judge_result(&window, vec![late(0.1, "too weak")]);
    assert!(session.aired().is_empty());

    session.on_judge_result(&window, vec![late(0.9, "threat")]);
    assert!(
        session
            .aired()
            .iter()
            .any(|r| r.missed_live && r.category == Category::TosRisk)
    );
    assert!(
        session
            .engine()
            .all()
            .iter()
            .all(|r| r.category != Category::TosRisk)
    );
}
