//! Real-SQLite tests of the enrolled cloud lane (cas-9b7d S2).

use super::*;
use tempfile::TempDir;

fn fixture() -> (TempDir, SqlitePromptQueueStore) {
    let dir = TempDir::new().unwrap();
    let store = SqlitePromptQueueStore::open(dir.path()).unwrap();
    store.init().unwrap();
    (dir, store)
}

fn binding() -> OperatorFeedBinding {
    OperatorFeedBinding {
        account_id: "acct-1".into(),
        machine_id: "4f1c0a52-5d1e-4b7a-9c43-2a7e0f6b1d01".into(),
        hub_id: "hub-1".into(),
        project_id: "github.com/richards-llc/cassy".into(),
        bound_at: "2026-10-05T19:00:00Z".into(),
    }
}

fn turn(prompt: &str) -> OperatorTurn<'_> {
    static DAEMON: QueueOrigin = QueueOrigin::Daemon;
    OperatorTurn {
        source: "supervisor",
        target: "operator",
        prompt,
        factory_session: Some("cas-src-quiet hawk ✦"),
        metadata: OperatorTurnMetadata {
            origin: Some(&DAEMON),
            ..Default::default()
        },
    }
}

fn sealed(epoch: &str) -> OperatorSealedBytes {
    OperatorSealedBytes {
        feed_generation: "1".into(),
        key_epoch: epoch.into(),
        ciphertext: format!("ciphertext-under-{epoch}"),
        digest: format!("sha256:{epoch:0>64}"),
    }
}

#[test]
fn session_routing_ids_are_opaque_stable_and_valid() {
    // Expected values computed independently (Python hashlib + base64).
    assert_eq!(
        session_routing_id("cas-src-quiet hawk ✦"),
        "s_n6FU0wUgvtSSnXD4XfjTHyP1QthiEe3Y_eC0Y5PAKq0"
    );
    assert_eq!(
        session_routing_id("x"),
        "s_LXEWQrcmsEQBYnyp-6wy9chTD7GQPMTbAiWHF5IaSIE"
    );
    assert!(is_routing_id(&session_routing_id(
        "any name, even with spaces"
    )));
    assert!(!is_routing_id("has space"));
    assert!(!is_routing_id(""));
    assert!(!is_routing_id(&"a".repeat(201)));
}

#[test]
fn turns_before_binding_never_enter_the_cloud_lane() {
    let (_dir, store) = fixture();
    store.record_operator_turn(&turn("before consent")).unwrap();
    assert_eq!(store.operator_cloud_backlog().unwrap().pending, 0);

    store.bind_operator_feed(&binding()).unwrap();
    let id = store
        .record_operator_turn(&turn("after consent"))
        .unwrap()
        .id();
    let backlog = store.operator_cloud_backlog().unwrap();
    assert_eq!(backlog.pending, 1);

    let claims = store
        .claim_operator_cloud(&binding().machine_id, Utc::now(), 100, 30)
        .unwrap();
    assert_eq!(claims.len(), 1);
    let claim = &claims[0];
    let event = store.operator_delivery_event(id).unwrap().unwrap();
    assert_eq!(claim.event_id, event.event_id);
    assert_eq!(claim.payload_snapshot, event.payload_snapshot);
    assert_eq!(claim.account_id, "acct-1");
    assert_eq!(claim.project_id, "github.com/richards-llc/cassy");
    assert_eq!(claim.session_id, session_routing_id("cas-src-quiet hawk ✦"));
    assert!(claim.sealed.is_none());
    // Another machine's drain never claims this audience's rows.
    assert!(
        store
            .claim_operator_cloud(
                "other-machine",
                Utc::now() + chrono::Duration::minutes(5),
                100,
                30
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn binding_refuses_a_second_audience_and_detach_keeps_pending_rows() {
    let (_dir, store) = fixture();
    store.bind_operator_feed(&binding()).unwrap();
    assert_eq!(store.bind_operator_feed(&binding()).unwrap(), binding());
    let mut other = binding();
    other.account_id = "acct-2".into();
    assert!(store.bind_operator_feed(&other).is_err());
    let mut invalid = binding();
    invalid.project_id = "github.com/x/petra_stella tools".into();
    assert!(store.bind_operator_feed(&invalid).is_err());

    store.record_operator_turn(&turn("pending")).unwrap();
    assert!(store.detach_operator_feed(Utc::now()).unwrap());
    store.record_operator_turn(&turn("after detach")).unwrap();
    let backlog = store.operator_cloud_backlog().unwrap();
    assert_eq!(
        backlog.pending, 1,
        "only the pre-detach turn is in the lane"
    );
    // A new audience can bind after detach; the old row keeps its account.
    store.bind_operator_feed(&other).unwrap();
    let claims = store
        .claim_operator_cloud(&binding().machine_id, Utc::now(), 100, 30)
        .unwrap();
    assert_eq!(claims[0].account_id, "acct-1");
}

#[test]
fn sealed_bytes_are_set_once_and_every_retry_reuses_them() {
    let (_dir, store) = fixture();
    store.bind_operator_feed(&binding()).unwrap();
    store.record_operator_turn(&turn("hello")).unwrap();
    let now = Utc::now();
    let claim = store
        .claim_operator_cloud(&binding().machine_id, now, 100, 30)
        .unwrap()
        .remove(0);
    assert!(
        store
            .seal_operator_cloud(&claim, &sealed("4"), now)
            .unwrap()
    );
    assert!(
        !store
            .seal_operator_cloud(&claim, &sealed("5"), now)
            .unwrap(),
        "a sealed row cannot be re-sealed by a later seal"
    );
    // ACK lost: retry with backoff, same bytes on the next claim.
    assert!(
        store
            .settle_operator_cloud(&claim, &OperatorCloudSettlement::Retry, now)
            .unwrap()
    );
    assert!(
        store
            .claim_operator_cloud(&binding().machine_id, now, 100, 30)
            .unwrap()
            .is_empty(),
        "backoff holds the row"
    );
    let later = now + chrono::Duration::minutes(2);
    let retry = store
        .claim_operator_cloud(&binding().machine_id, later, 100, 30)
        .unwrap()
        .remove(0);
    assert_eq!(retry.sealed, Some(sealed("4")));
    assert_eq!(retry.attempts, 2);
    assert!(
        store
            .settle_operator_cloud(
                &retry,
                &OperatorCloudSettlement::Receipt {
                    kind: "stored",
                    sequence: "812".into()
                },
                later
            )
            .unwrap()
    );
    let backlog = store.operator_cloud_backlog().unwrap();
    assert_eq!((backlog.pending, backlog.delivered), (0, 1));
}

#[test]
fn epoch_retired_reseals_the_same_event_and_a_stale_lease_settles_nothing() {
    let (_dir, store) = fixture();
    store.bind_operator_feed(&binding()).unwrap();
    store.record_operator_turn(&turn("hello")).unwrap();
    let now = Utc::now();
    let claim = store
        .claim_operator_cloud(&binding().machine_id, now, 100, 1)
        .unwrap()
        .remove(0);
    store
        .seal_operator_cloud(&claim, &sealed("4"), now)
        .unwrap();
    assert!(
        store
            .settle_operator_cloud(&claim, &OperatorCloudSettlement::Reseal, now)
            .unwrap()
    );
    let again = store
        .claim_operator_cloud(&binding().machine_id, now, 100, 1)
        .unwrap()
        .remove(0);
    assert_eq!(again.event_id, claim.event_id);
    assert!(again.sealed.is_none());
    assert!(
        store
            .seal_operator_cloud(&again, &sealed("5"), now)
            .unwrap()
    );

    // The lease lapses; a new worker claims it, and the old token is fenced.
    let lapsed = now + chrono::Duration::seconds(2);
    let newer = store
        .claim_operator_cloud(&binding().machine_id, lapsed, 100, 30)
        .unwrap()
        .remove(0);
    assert!(
        !store
            .settle_operator_cloud(
                &again,
                &OperatorCloudSettlement::Receipt {
                    kind: "stored",
                    sequence: "1".into()
                },
                lapsed
            )
            .unwrap()
    );
    assert_eq!(newer.sealed, Some(sealed("5")));
}

#[test]
fn a_terminal_refusal_parks_the_row_visibly() {
    let (_dir, store) = fixture();
    store.bind_operator_feed(&binding()).unwrap();
    store.record_operator_turn(&turn("hello")).unwrap();
    let now = Utc::now();
    let claim = store
        .claim_operator_cloud(&binding().machine_id, now, 100, 30)
        .unwrap()
        .remove(0);
    assert!(
        store
            .settle_operator_cloud(
                &claim,
                &OperatorCloudSettlement::Park {
                    reason: "event_conflict"
                },
                now
            )
            .unwrap()
    );
    let backlog = store.operator_cloud_backlog().unwrap();
    assert_eq!((backlog.pending, backlog.parked), (0, 1));
    assert!(
        store
            .claim_operator_cloud(
                &binding().machine_id,
                now + chrono::Duration::hours(1),
                100,
                30
            )
            .unwrap()
            .is_empty()
    );
}
