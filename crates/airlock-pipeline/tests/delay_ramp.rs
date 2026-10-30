//! A new delay is approached by 5% of each new slice of media, not applied at once.

use airlock_core::{Dur, Ts};
use airlock_pipeline::{Session, SessionConfig};

#[test]
fn raising_the_delay_does_not_jump_the_hold() {
    let mut session = Session::new(SessionConfig::default());
    assert_eq!(session.hold(), Dur::from_millis(8_000));
    session.set_hold_target(Dur::from_millis(10_000));
    session.on_live_time(Ts::from_millis(1_000));
    assert_eq!(session.hold(), Dur::from_millis(8_050));
    session.on_live_time(Ts::from_millis(2_000));
    assert_eq!(session.hold(), Dur::from_millis(8_100));
}
