//! The judge sees streamer rules and the platform pack, never the private-term list.

use airlock_core::{Platform, Span, Word};
use airlock_llm::build_prompt;
use airlock_pipeline::{Session, SessionConfig};

#[test]
fn prompt_includes_the_rule_and_the_platform_pack_only() {
    let cfg = SessionConfig {
        platform: Platform::Kick,
        streamer_rules: vec!["don't say my ex's name".into()],
        private_terms: vec!["super-secret-town".into()],
        ..SessionConfig::default()
    };
    let mut session = Session::new(cfg);
    session.on_words(vec![
        Word::new("hello", Span::from_millis(0, 200), 0.9),
        Word::new("chat", Span::from_millis(200, 400), 0.9),
    ]);
    let window = session.next_judge_window().unwrap();
    let prompt = build_prompt(&session.judge_request(&window));
    assert!(prompt.contains("don't say my ex's name"));
    assert!(prompt.contains("Kick pack v1"));
    assert!(!prompt.contains("Twitch pack v1"));
    assert!(!prompt.contains("super-secret-town"));
}
