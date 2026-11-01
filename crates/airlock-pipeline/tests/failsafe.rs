//! Dump and analysis-stall behavior. No media ingest required.

use airlock_core::{Action, Category, Policy, Span, StallFallback, Ts, Word};
use airlock_pipeline::{Session, SessionConfig, UiEvent};

fn session(fallback: StallFallback, stall_after_ms: i64) -> Session {
    Session::new(SessionConfig {
        policy: Policy {
            stall_fallback: fallback,
            stall_after_ms,
            ..Policy::default()
        },
        ..Default::default()
    })
}

fn advance(session: &mut Session, mut t: i64, to_ms: i64, energetic: bool) {
    while t < to_ms {
        t += 100;
        session.on_live_time(Ts::from_millis(t));
        session.note_audio_energy(energetic);
    }
}

#[test]
fn dump_mutes_only_audio_still_in_the_hold() {
    let mut s = session(StallFallback::Warn, 3_000);
    s.on_live_time(Ts::from_millis(20_000));
    s.dump(false);
    let muted = s
        .engine()
        .audio_redactions_in(Span::from_millis(15_000, 15_100))
        .any(|r| r.action == Action::Mute && r.category == Category::Manual && r.is_active());
    assert!(muted);
    assert_eq!(
        s.engine()
            .audio_redactions_in(Span::from_millis(10_000, 12_000))
            .count(),
        0,
        "audio that already aired stays untouched"
    );
    assert!(
        s.drain_events()
            .iter()
            .any(|e| matches!(e, UiEvent::RedactionUpserted { .. }))
    );
}

#[test]
fn silence_does_not_stall() {
    let mut s = session(StallFallback::Mute, 1_000);
    advance(&mut s, 0, 5_000, false);
    assert!(
        !s.drain_events()
            .iter()
            .any(|e| matches!(e, UiEvent::AnalysisStall { active: true, .. }))
    );
    assert_eq!(s.engine().all().len(), 0);
}

#[test]
fn stall_warn_banners_without_muting() {
    let mut s = session(StallFallback::Warn, 1_000);
    advance(&mut s, 0, 1_500, true);
    let events = s.drain_events();
    assert!(events.iter().any(|e| matches!(
        e,
        UiEvent::AnalysisStall {
            active: true,
            fallback: StallFallback::Warn,
            message,
        } if message.contains("not connected")
    )));
    assert_eq!(s.engine().all().len(), 0);
}

#[test]
fn a_connected_recognizer_says_it_is_listening() {
    let mut s = session(StallFallback::Warn, 1_000);
    s.note_recognizer(true);
    advance(&mut s, 0, 9_000, true);
    assert!(s.drain_events().iter().any(|e| matches!(
        e,
        UiEvent::AnalysisStall { active: true, message, .. } if message.contains("Listening")
    )));
}

#[test]
fn stall_mute_covers_audio_after_the_last_word() {
    let mut s = session(StallFallback::Mute, 1_000);
    s.on_live_time(Ts::from_millis(2_000));
    s.on_words(vec![Word::new(
        "hello",
        Span::from_millis(1_800, 2_000),
        0.9,
    )]);
    s.drain_events();
    advance(&mut s, 2_000, 3_500, true);
    assert!(
        s.engine()
            .audio_redactions_in(Span::from_millis(3_000, 3_200))
            .any(|r| r.is_active() && r.action == Action::Mute)
    );
    assert_eq!(
        s.engine()
            .audio_redactions_in(Span::from_millis(1_000, 1_500))
            .count(),
        0,
        "audio from before the last word is not part of the stall mute"
    );
    s.on_words(vec![Word::new(
        "back",
        Span::from_millis(3_400, 3_500),
        0.9,
    )]);
    let events = s.drain_events();
    assert!(events.iter().any(|e| matches!(
        e,
        UiEvent::AnalysisStall { active: true, message, .. } if message.contains("stalled")
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::AnalysisStall { active: false, .. }))
    );
}

#[test]
fn a_live_recognizer_does_not_stall_during_a_loud_pause() {
    let mut s = session(StallFallback::Warn, 1_000);
    s.note_recognizer(true);
    s.on_live_time(Ts::from_millis(2_000));
    s.on_words(vec![Word::new(
        "hello",
        Span::from_millis(1_800, 2_000),
        0.9,
    )]);
    s.drain_events();
    s.on_live_time(Ts::from_millis(12_000));
    s.note_recognizer_progress(Ts::from_millis(11_500));
    s.note_audio_energy(true);
    assert!(
        !s.drain_events()
            .iter()
            .any(|e| matches!(e, UiEvent::AnalysisStall { active: true, .. }))
    );
    s.on_live_time(Ts::from_millis(22_000));
    s.note_audio_energy(true);
    assert!(
        s.drain_events()
            .iter()
            .any(|e| matches!(e, UiEvent::AnalysisStall { active: true, .. }))
    );
}

#[test]
fn stall_extend_delay_holds_longer_and_does_not_snap_back() {
    let mut s = session(StallFallback::ExtendDelay, 1_000);
    advance(&mut s, 0, 1_500, true);
    assert_eq!(s.hold().as_secs_f64(), 9.0);
    s.on_words(vec![Word::new(
        "back",
        Span::from_millis(1_400, 1_500),
        0.9,
    )]);
    assert_eq!(
        s.hold().as_secs_f64(),
        9.0,
        "dropping the extra delay would skip audio"
    );
}

#[test]
fn stall_extend_delay_stops_at_sixty_seconds() {
    let mut s = Session::new(SessionConfig {
        policy: Policy {
            delay_ms: 59_000,
            stall_after_ms: 3_000,
            stall_fallback: StallFallback::ExtendDelay,
            ..Policy::default()
        },
        ..Default::default()
    });
    s.on_live_time(Ts::from_millis(4_000));
    s.note_audio_energy(true);
    assert_eq!(s.hold().as_secs_f64(), 59.0);
}
