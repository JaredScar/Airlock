//! A vision finding the recognizer cannot read from text.
//! Inside the delay it is a suggestion. After the span has aired it is missed.

use airlock_core::{Category, DetectorKind, Dur, Mode, Rect, Span, Ts};
use airlock_llm::deadline_budget;
use airlock_pipeline::{Session, SessionConfig};
use airlock_vision::scene_deadline;

#[test]
fn the_scene_budget_matches_the_judge() {
    let delay = Dur::from_millis(8_000);
    let judge = deadline_budget(delay, Dur::from_millis(1_500), Dur::from_millis(1_500));
    assert_eq!(scene_deadline(delay).as_micros() as i64, judge.0);
}

#[test]
fn a_scene_inside_the_delay_is_a_suggestion() {
    let mut session = Session::new(SessionConfig::default());
    session.on_live_time(Ts::from_millis(1_000));
    session.on_scene(
        Span::new(Ts::ZERO, Ts::from_millis(250)),
        Rect::FULL,
        0.9,
        "explicit image on screen",
    );
    let redactions = session.engine().all();
    assert_eq!(redactions.len(), 1);
    let redaction = &redactions[0];
    assert_eq!(redaction.category, Category::ScreenPii);
    assert_eq!(redaction.mode, Mode::Suggest);
    assert!(!redaction.is_active());
    assert!(session.aired().is_empty());
    let events = session.drain_events();
    let upsert = events.into_iter().find_map(|event| match event {
        airlock_pipeline::UiEvent::RedactionUpserted { redaction } => Some(redaction),
        _ => None,
    });
    let redaction = upsert.expect("upsert");
    assert_eq!(redaction.sources, vec![DetectorKind::VisionModel]);
    assert_eq!(
        redaction.evidence,
        vec![airlock_core::model::mask_evidence(
            "explicit image on screen"
        )]
    );
}

#[test]
fn a_scene_that_arrives_after_air_time_is_missed() {
    let mut session = Session::new(SessionConfig::default());
    session.on_live_time(Ts::from_millis(10_000));
    session.on_scene(
        Span::new(Ts::ZERO, Ts::from_millis(250)),
        Rect {
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.2,
        },
        0.9,
        "direct message on screen",
    );
    assert!(session.engine().all().is_empty());
    assert_eq!(session.aired().len(), 1);
    assert_eq!(session.aired()[0].sources, vec![DetectorKind::VisionModel]);
    assert!(session.aired()[0].missed_live);
}
