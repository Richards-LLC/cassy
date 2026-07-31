use cas_store::{
    DELIVERY_SCHEMA, SqliteVerificationStore, StoreError, VERIFICATION_SCHEMA, VerificationStore,
    transition_worker_delivery_verification_with_conn,
};
use cas_types::{Verification, VerificationProvenance};
use chrono::Utc;
use rusqlite::{Connection, params};
use tempfile::TempDir;

#[test]
fn generic_add_rejects_forged_trusted_provenance_without_mutation() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVerificationStore::open(dir.path()).unwrap();
    let mut forged = Verification::approved(
        store.generate_id().unwrap(),
        "cas-forged".to_string(),
        "forged".to_string(),
    );
    forged.provenance = VerificationProvenance::SupervisorDirect;
    forged.agent_id = Some("agent-forged".to_string());
    forged.issuer_agent_id = Some("agent-forged".to_string());
    forged.dispatch_id = Some("vdispatch-forged".to_string());

    assert!(store.add(&forged).is_err());
    assert!(matches!(
        store.get(&forged.id),
        Err(StoreError::NotFound(_))
    ));
    assert!(store.get_for_task(&forged.task_id).unwrap().is_empty());
}

#[test]
fn generic_update_cannot_promote_a_legacy_row_to_trusted_provenance() {
    let dir = TempDir::new().unwrap();
    let store = SqliteVerificationStore::open(dir.path()).unwrap();
    let legacy = Verification::approved(
        store.generate_id().unwrap(),
        "cas-legacy".to_string(),
        "legacy".to_string(),
    );
    store.add(&legacy).unwrap();

    let mut forged = legacy.clone();
    forged.provenance = VerificationProvenance::SupervisorDirect;
    forged.agent_id = Some("agent-forged".to_string());
    forged.issuer_agent_id = Some("agent-forged".to_string());
    forged.dispatch_id = Some("vdispatch-forged".to_string());
    assert!(store.update(&forged).is_err());

    let persisted = store.get(&legacy.id).unwrap();
    assert_eq!(persisted.provenance, VerificationProvenance::Legacy);
    assert_eq!(persisted.agent_id, None);
    assert_eq!(persisted.summary, "legacy");
}

#[test]
fn delivery_projection_rejects_an_unrelated_verification_without_mutation() {
    let dir = TempDir::new().unwrap();
    let conn = Connection::open(dir.path().join("cas.db")).unwrap();
    conn.execute_batch(DELIVERY_SCHEMA).unwrap();
    conn.execute_batch(VERIFICATION_SCHEMA).unwrap();
    let now = Utc::now().to_rfc3339();

    for suffix in ["a", "b"] {
        conn.execute(
            "INSERT INTO worker_completion_receipts
             (id, task_id, worker_agent_id, worker_name, repo_selector, source_branch,
              commit_sha, merge_base_sha, target_branch, target_sha, proof_reference,
              scope_summary, created_at)
             VALUES (?1, ?2, 'worker', 'worker', 'repo', 'source', 'commit', 'base',
                     'target', 'tip', 'proof', 'scope', ?3)",
            params![format!("receipt-{suffix}"), format!("task-{suffix}"), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO worker_delivery_transactions
             (id, receipt_id, task_id, state, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'awaiting_verification', ?4, ?4)",
            params![
                format!("delivery-{suffix}"),
                format!("receipt-{suffix}"),
                format!("task-{suffix}"),
                now
            ],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO verification_dispatches
         (id, task_id, receipt_id, delivery_transaction_id, requester_agent_id,
          owner_agent_id, state, requested_at, deadline_at, resolved_at, recovery_action)
         VALUES ('dispatch-a', 'task-a', 'receipt-a', 'delivery-a', 'worker',
                 'supervisor', 'resolved', ?1, ?1, ?1, 'supervisor_redispatch_or_direct')",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO verifications
         (id, task_id, agent_id, verification_type, provenance, dispatch_id,
          issuer_agent_id, status, summary, files_reviewed, created_at)
         VALUES ('verification-a', 'task-a', 'supervisor', 'task', 'supervisor_direct',
                 'dispatch-a', 'supervisor', 'approved', 'approved', '[]', ?1)",
        params![now],
    )
    .unwrap();

    let event_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM worker_delivery_events", [], |row| row.get(0))
        .unwrap();
    assert!(
        transition_worker_delivery_verification_with_conn(
            &conn,
            "delivery-b",
            "verification-a",
            true,
            "supervisor",
        )
        .is_err()
    );
    let state: String = conn
        .query_row(
            "SELECT state FROM worker_delivery_transactions WHERE id = 'delivery-b'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "awaiting_verification");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM worker_delivery_events", [], |row| {
            row.get::<_, i64>(0)
        })
            .unwrap(),
        event_count
    );

    assert!(
        transition_worker_delivery_verification_with_conn(
            &conn,
            "delivery-a",
            "verification-a",
            true,
            "supervisor",
        )
        .unwrap()
        .is_some()
    );
    assert!(
        transition_worker_delivery_verification_with_conn(
            &conn,
            "delivery-a",
            "verification-a",
            true,
            "supervisor",
        )
        .unwrap()
        .is_none()
    );
}
