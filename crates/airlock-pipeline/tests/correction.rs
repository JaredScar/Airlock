//! A later recognition pass can fix a word that has not started airing, and the
//! bleep follows that correction.

use airlock_core::{Category, Span, Ts, Word};
use airlock_pipeline::{Session, SessionConfig, UiEvent};

fn texts(session: &Session) -> Vec<String> {
    session
        .transcript()
        .iter()
        .map(|word| word.text.clone())
        .collect()
}

#[test]
fn a_held_swear_can_be_corrected_before_it_airs() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![
        Word::new("free", Span::from_millis(1_000, 1_300), 0.6),
        Word::new("fuck", Span::from_millis(1_400, 1_700), 0.7),
    ]);
    assert!(
        session
            .engine()
            .upcoming(session.playhead())
            .iter()
            .any(|redaction| redaction.category == Category::Profanity)
    );
    session.drain_events();

    session.on_words(vec![
        Word::new("I", Span::from_millis(1_000, 1_300), 0.85),
        Word::new("truck", Span::from_millis(1_400, 1_700), 0.9),
    ]);
    assert_eq!(texts(&session), vec!["I", "truck"]);
    let events = session.drain_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, UiEvent::TranscriptRevised { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, UiEvent::RedactionRemoved { .. }))
    );
    assert!(
        session
            .engine()
            .upcoming(session.playhead())
            .iter()
            .all(|redaction| redaction.category != Category::Profanity)
    );
}

#[test]
fn a_correction_can_add_the_bleep_the_first_guess_missed() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![Word::new(
        "truck",
        Span::from_millis(2_000, 2_400),
        0.6,
    )]);
    session.drain_events();
    session.on_words(vec![Word::new(
        "fuck",
        Span::from_millis(2_000, 2_400),
        0.9,
    )]);
    assert_eq!(texts(&session), vec!["fuck"]);
    assert!(
        session
            .engine()
            .upcoming(session.playhead())
            .iter()
            .any(|redaction| redaction.category == Category::Profanity)
    );
}

#[test]
fn a_word_that_has_started_airing_is_kept() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![Word::new(
        "hello",
        Span::from_millis(1_000, 1_400),
        0.7,
    )]);
    session.on_live_time(Ts::from_millis(10_000));
    session.on_words(vec![Word::new(
        "nope",
        Span::from_millis(1_000, 1_400),
        0.99,
    )]);
    assert_eq!(texts(&session), vec!["hello"]);
}

#[test]
fn a_word_repeated_by_the_next_listen_is_kept_once() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![
        Word::new("next", Span::from_millis(1_000, 1_250), 0.8),
        Word::new("grid", Span::from_millis(1_300, 1_550), 0.8),
    ]);
    session.on_words(vec![
        Word::new("next", Span::from_millis(1_900, 2_150), 0.8),
        Word::new("grid", Span::from_millis(2_200, 2_450), 0.8),
    ]);
    assert_eq!(texts(&session), vec!["next", "grid"]);
    session.on_words(vec![Word::new(
        "next",
        Span::from_millis(6_000, 6_250),
        0.8,
    )]);
    assert_eq!(texts(&session), vec!["next", "grid", "next"]);
}

#[test]
fn repeated_digits_stay_in_the_transcript() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![
        Word::new("five", Span::from_millis(1_000, 1_200), 0.9),
        Word::new("five", Span::from_millis(1_300, 1_500), 0.9),
        Word::new("five", Span::from_millis(1_600, 1_800), 0.9),
    ]);
    assert_eq!(texts(&session), vec!["five", "five", "five"]);
}

#[test]
fn a_quiet_swear_is_still_bleeped() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![Word::new(
        "fucking...",
        Span::from_millis(3_000, 3_400),
        0.3,
    )]);
    assert_eq!(texts(&session), vec!["fucking..."]);
    assert!(
        session
            .engine()
            .upcoming(session.playhead())
            .iter()
            .any(|redaction| redaction.category == Category::Profanity && redaction.is_active())
    );
}

#[test]
fn a_much_worse_guess_does_not_replace_the_held_word() {
    let mut session = Session::new(SessionConfig::default());
    session.on_words(vec![Word::new(
        "fuck",
        Span::from_millis(1_000, 1_400),
        0.95,
    )]);
    session.on_words(vec![Word::new(
        "speloni",
        Span::from_millis(1_000, 1_400),
        0.4,
    )]);
    assert_eq!(texts(&session), vec!["fuck"]);
    assert!(
        session
            .engine()
            .upcoming(session.playhead())
            .iter()
            .any(|redaction| redaction.category == Category::Profanity)
    );
}
