//! Regression coverage for exact verification-dispatch timeout identity.

use cas_store::{
    SqliteVerificationStore, VerificationStore, create_verification_dispatch,
    get_latest_verification_dispatch, get_verification_dispatch, timeout_verification_dispatch,
};
use cas_types::VerificationDispatchState;
use chrono::{Duration, Utc};
use rusqlite::Connection;

#[test]
fn timeout_returns_the_exact_row_mutated_when_a_replacement_is_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteVerificationStore::open(dir.path()).expect("verification store");
    store.init().expect("initialize verification schema");
    let due = create_verification_dispatch(
        dir.path(),
        "cas-timeout-atomic",
        "requester-original",
        "owner-original",
        Utc::now() - Duration::minutes(1),
    )
    .expect("create due dispatch");

    let conn = Connection::open(dir.path().join("cas.db")).expect("open cas.db");
    conn.execute_batch(
        "CREATE TRIGGER create_replacement_after_timeout
         AFTER UPDATE OF state ON verification_dispatches
         WHEN NEW.id = OLD.id
              AND NEW.task_id = 'cas-timeout-atomic'
              AND OLD.state IN ('pending', 'claimed')
              AND NEW.state = 'timed_out'
         BEGIN
             INSERT INTO verification_dispatches
                 (id, task_id, requester_agent_id, owner_agent_id, state,
                  requested_at, deadline_at, recovery_action)
             VALUES
                 ('vdispatch-replacement', 'cas-timeout-atomic',
                  'requester-replacement', 'owner-replacement', 'pending',
                  '2098-01-01T00:00:00+00:00', '2099-01-01T00:00:00+00:00',
                  'supervisor_redispatch_or_direct');
         END;",
    )
    .expect("install deterministic replacement race");
    drop(conn);

    let returned = timeout_verification_dispatch(dir.path(), "cas-timeout-atomic", Utc::now())
        .expect("timeout operation")
        .expect("due dispatch");

    assert_eq!(
        returned.id, due.id,
        "timeout must return the exact dispatch whose state it changed"
    );
    assert_eq!(returned.state, VerificationDispatchState::TimedOut);
    assert_eq!(returned.requester_agent_id, "requester-original");
    assert_eq!(returned.owner_agent_id, "owner-original");
    assert_eq!(
        get_verification_dispatch(dir.path(), &due.id)
            .expect("load original")
            .state,
        VerificationDispatchState::TimedOut
    );
    let latest = get_latest_verification_dispatch(dir.path(), "cas-timeout-atomic")
        .expect("load latest")
        .expect("replacement");
    assert_eq!(latest.id, "vdispatch-replacement");
    assert_eq!(latest.state, VerificationDispatchState::Pending);
}
