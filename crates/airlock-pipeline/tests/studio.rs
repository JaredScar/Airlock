//! Studio settings choose the platform, the streamer's rules, and the policy.

use airlock_core::{Category, Platform, Span, Word};
use airlock_llm::build_prompt;
use airlock_pipeline::{StudioSettings, issue_dev_license};

#[test]
fn selected_platform_rules_reach_the_judge_and_private_terms_do_not() {
    let mut settings = StudioSettings {
        platform: Platform::Youtube,
        rules_youtube: vec!["no sponsor names".into()],
        rules_twitch: vec!["twitch only rule".into()],
        private_terms: vec!["super-secret-town".into()],
        extra_profanity: "airlocktest\n".into(),
        ..StudioSettings::default()
    };
    settings.policy.delay_ms = 5_000;

    let mut session = airlock_pipeline::Session::new(settings.session_config().unwrap());
    assert_eq!(session.hold().as_millis(), 5_000);
    session.on_words(vec![Word::new(
        "airlocktest",
        Span::from_millis(0, 400),
        0.99,
    )]);
    assert!(
        session
            .engine()
            .all()
            .iter()
            .any(|r| r.category == Category::Profanity)
    );

    let window = session.next_judge_window().unwrap();
    let prompt = build_prompt(&session.judge_request(&window));
    assert!(prompt.contains("no sponsor names"));
    assert!(prompt.contains("YouTube pack v1"));
    assert!(!prompt.contains("twitch only rule"));
    assert!(!prompt.contains("super-secret-town"));
}

#[test]
fn two_destinations_share_the_prompt_and_need_pro() {
    let mut settings = StudioSettings {
        platform: Platform::Youtube,
        destinations: vec![Platform::Twitch, Platform::Youtube],
        rules_twitch: vec!["twitch only rule".into()],
        rules_youtube: vec!["no sponsor names".into()],
        ..StudioSettings::default()
    };
    let free = settings.session_config().unwrap();
    assert_eq!(free.platform, Platform::Youtube);
    assert!(free.also_platforms.is_empty());
    assert_eq!(free.streamer_rules, vec!["no sponsor names".to_string()]);

    settings.license_key = issue_dev_license();
    let pro = settings.session_config().unwrap();
    assert_eq!(pro.platform, Platform::Twitch);
    assert_eq!(pro.also_platforms, vec![Platform::Youtube]);
    let mut session = airlock_pipeline::Session::new(pro);
    session.on_words(vec![Word::new("hello", Span::from_millis(0, 200), 0.9)]);
    let window = session.next_judge_window().unwrap();
    let prompt = build_prompt(&session.judge_request(&window));
    assert!(prompt.contains("on Twitch and YouTube"));
    assert!(prompt.contains("Twitch pack v1"));
    assert!(prompt.contains("YouTube pack v1"));
    assert!(prompt.contains("twitch only rule"));
    assert!(prompt.contains("no sponsor names"));
    assert!(!prompt.contains("Springfield"));
}

#[test]
fn an_old_settings_file_still_publishes_one_platform() {
    let mut value = serde_json::to_value(StudioSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("destinations");
    let loaded: StudioSettings = serde_json::from_value(value).unwrap();
    assert!(loaded.destinations.is_empty());
    assert_eq!(loaded.publish_platforms(), vec![Platform::Twitch]);
}

#[test]
fn bad_delay_is_rejected() {
    let settings = StudioSettings {
        policy: airlock_core::Policy {
            delay_ms: 50,
            ..airlock_core::Policy::default()
        },
        ..StudioSettings::default()
    };
    assert!(settings.session_config().is_err());
}
