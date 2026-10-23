use super::*;
#[test]
fn expired_setup_cannot_be_promoted_by_fully_ready_buffered_control() {
    let now = Instant::now();
    for state in [
        SessionState::Negotiating,
        SessionState::Preparing,
        SessionState::AwaitingActive,
    ] {
        assert!(setup_expired(state, now, now));
        assert!(setup_expired(state, now, now + Duration::from_millis(1)));
        assert!(!setup_expired(state, now + Duration::from_millis(1), now));
    }
    assert!(!setup_expired(
        SessionState::Active,
        now,
        now + Duration::from_secs(3600)
    ));
}
