use super::*;
use crate::{
    AgentStore, HookDrain, PromptQueueStore, SqliteAgentStore, SqlitePromptQueueStore,
    SurfacingSource,
};

const NOW: &str = "2026-01-02T02:00:00Z";
const SESSION: &str = "fixture-session";

fn fixture() -> (tempfile::TempDir, SqlitePromptQueueStore, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
    queue.init().unwrap();
    SqliteAgentStore::open(temp.path()).unwrap().init().unwrap();
    let conn = Connection::open(temp.path().join("cas.db")).unwrap();
    conn.execute_batch(crate::EVENT_SCHEMA).unwrap();
    conn.execute_batch(include_str!(
        "../../../../scripts/fixtures/communication-slos/session.sql"
    ))
    .unwrap();
    (temp, queue, conn)
}

fn values(conn: &Connection) -> CommunicationSlos {
    measure(conn, SESSION, &now().to_rfc3339()).unwrap()
}
fn now() -> DateTime<Utc> {
    NOW.parse().unwrap()
}
fn scalar(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> i64 {
    conn.query_row(sql, params, |r| r.get(0)).unwrap()
}

#[test]
fn cas_7e24_false_abandon_pass_and_breach() {
    let (_temp, queue, conn) = fixture();
    assert_eq!(values(&conn).false_abandon, 0);
    let id = queue
        .enqueue_with_session("supervisor", "worker", "real body", SESSION)
        .unwrap();
    queue
        .surface_unseen_for_recipient("worker", Some(SESSION), 10)
        .unwrap();
    conn.execute("UPDATE prompt_queue SET highest_stage='abandoned',last_pending_reason='abandoned_unknown_target' WHERE id=?", [id]).unwrap();
    assert_eq!(values(&conn).false_abandon, 1);
    conn.execute(
        "UPDATE prompt_queue_recipient_seen SET source='transport_delivered' WHERE prompt_id=?",
        [id],
    )
    .unwrap();
    assert_eq!(values(&conn).false_abandon, 0, "handoff is not seen");
    conn.execute("UPDATE prompt_queue_recipient_seen SET source='hook_surfaced',recipient='someone-else' WHERE prompt_id=?", [id]).unwrap();
    assert_eq!(
        values(&conn).false_abandon,
        0,
        "other recipients cannot establish a read"
    );
    conn.execute(
        "UPDATE prompt_queue_recipient_seen SET recipient='worker' WHERE prompt_id=?",
        [id],
    )
    .unwrap();
    assert_eq!(values(&conn).false_abandon, 1);
    conn.execute(
        "UPDATE prompt_queue SET last_pending_reason='recipient_ended_unread' WHERE id=?",
        [id],
    )
    .unwrap();
    assert_eq!(
        values(&conn).false_abandon,
        0,
        "ended-unread is a different disposition"
    );
}

#[test]
fn cas_7e24_operator_receipts_pass_and_breach() {
    let (_temp, _queue, conn) = fixture();
    let m = values(&conn);
    assert_eq!((m.operator_receipts, m.operator_events), (1, 1));
    assert_eq!(m.operator_unenrolled, Some(0));
    // cas-5c12 (GH #1261): a local relay_receipt is not the enrolled lane.
    conn.execute(
        "UPDATE operator_delivery_outbox SET relay_receipt=NULL,retained_at=NULL",
        [],
    )
    .unwrap();
    assert_eq!(values(&conn).breach_count(), 0);
    // Unenrolled snapshots only: not applicable, never a 0% breach, while the
    // independent gated-without-outcome breach stays visible.
    conn.execute("DELETE FROM operator_cloud_outbox", [])
        .unwrap();
    conn.execute(
        "UPDATE prompt_queue SET transport_delivered_at=NULL WHERE id=1",
        [],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM prompt_queue_recipient_seen WHERE prompt_id=1",
        [],
    )
    .unwrap();
    let m = values(&conn);
    assert_eq!(
        (
            m.operator_receipts,
            m.operator_events,
            m.operator_unenrolled
        ),
        (0, 0, Some(1))
    );
    assert!(
        m.lines()[1].starts_with("N/A operator relay receipts: not applicable"),
        "{}",
        m.lines()[1]
    );
    assert!(m.lines()[1].contains("cas hub operator bind"));
    assert!(m.lines()[4].starts_with("BREACH gated-without-outcome: 1"));
    assert_eq!(m.breach_count(), 1);
    // Mixed: an enrolled turn still pending its receipt breaches; the
    // unenrolled snapshot stays out of the denominator.
    conn.execute_batch(
        "INSERT INTO operator_delivery_outbox(event_id,prompt_id,factory_session,payload_snapshot,created_at)
         VALUES('enrolled-event',2,'fixture-session','{}','2026-01-02T00:01:00Z');
         INSERT INTO operator_cloud_outbox(event_id,account_id,machine_id,hub_id,project_id,session_id,created_at)
         VALUES('enrolled-event','acct','machine','hub','project','s_fixture','2026-01-02T00:01:00Z');",
    )
    .unwrap();
    let m = values(&conn);
    assert_eq!(
        (
            m.operator_receipts,
            m.operator_events,
            m.operator_unenrolled
        ),
        (0, 1, Some(1))
    );
    assert!(m.lines()[1].starts_with("BREACH operator relay receipts: 0.0% (0/1"));
    assert!(m.lines()[1].contains("1 unenrolled excluded"));
    assert_eq!(m.breach_count(), 2);
    conn.execute("DELETE FROM operator_delivery_outbox", [])
        .unwrap();
    assert!(values(&conn).lines()[1].contains("100.0% (0/0"));
    // A summary saved before the split is shown as recorded, labelled.
    let mut legacy = serde_json::to_value(&m).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("operator_unenrolled");
    let legacy: CommunicationSlos = serde_json::from_value(legacy).unwrap();
    assert!(legacy.lines()[1].contains("legacy denominator"));
}

#[test]
fn cas_7e24_duplicate_bodies_pass_and_breach_through_real_drains() {
    let (_temp, queue, conn) = fixture();
    assert_eq!(values(&conn).duplicate_body_reads, 0);
    let id = queue
        .enqueue_with_session("supervisor", "reader", "body", SESSION)
        .unwrap();
    assert_eq!(
        queue
            .poll_unseen_for_recipient("reader", Some(SESSION), 10)
            .unwrap()
            .len(),
        1
    );
    assert!(
        queue
            .poll_unseen_for_recipient("reader", Some(SESSION), 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(values(&conn).duplicate_body_reads, 0);
    let rows = queue
        .surface_for_recipient_within_budget(
            "reader",
            Some(SESSION),
            HookDrain::PolledBefore(Utc::now() + chrono::Duration::seconds(1)),
            10,
            12000,
        )
        .unwrap();
    assert_eq!(rows.rows.len(), 1, "recovery returns an unread reference");
    assert!(rows.rows[0].reference_only);
    assert_eq!(values(&conn).duplicate_body_reads, 0, "references do not repeat the body");
    let named = queue.poll_named_for_recipient("reader", Some(SESSION), &[id]).unwrap();
    assert_eq!(named.len(), 1);
    // pin: named recovery returns the original body, never the projected repeat notice.
    assert_eq!(named[0].prompt, "body", "explicit named reads recover the actual body");
    assert!(!named[0].reference_only);
    assert_eq!(values(&conn).duplicate_body_reads, 1);
    conn.execute("DELETE FROM prompt_queue WHERE id=?", [id])
        .unwrap();
    assert_eq!(
        values(&conn).duplicate_body_reads,
        0,
        "observations prune with queue rows"
    );
    let id = queue
        .enqueue_with_session("supervisor", "observer", "observed body", SESSION)
        .unwrap();
    queue
        .record_recipient_surfaced(id, "observer", SurfacingSource::TransportDelivered)
        .unwrap();
    let observed = "SELECT COUNT(*) FROM communication_body_observations WHERE prompt_id=?";
    assert_eq!(scalar(&conn, observed, [id]), 0);
    for _ in 0..2 {
        queue
            .record_recipient_surfaced(id, "observer", SurfacingSource::ObservedWake)
            .unwrap();
    }
    let bodies = "SELECT body_count FROM communication_body_observations WHERE prompt_id=?";
    assert_eq!(
        scalar(&conn, bodies, [id]),
        1,
        "wake observation retries are bookkeeping"
    );
    conn.execute(
        "UPDATE communication_observation_epoch SET started_at='2099-01-01T00:00:00Z'",
        [],
    )
    .unwrap();
    assert_eq!(
        values(&conn).outcome(),
        "UNKNOWN",
        "do not invent historical duplicate coverage"
    );
}

#[test]
fn cas_7e24_nudges_pass_and_breach_with_retired_worker_exposure() {
    let (_temp, _queue, conn) = fixture();
    let m = values(&conn);
    assert_eq!(m.director_nudges, 1);
    assert!((m.worker_hours - 2.0).abs() < 1e-8);
    assert!(m.nudges_pass());
    conn.execute("UPDATE agents SET registered_at='2026-01-02T01:30:00Z'", [])
        .unwrap();
    assert!(!values(&conn).nudges_pass());
    conn.execute("DELETE FROM agents", []).unwrap();
    assert!(
        !values(&conn).nudges_pass(),
        "nudges with no exposure cannot be zero/hour"
    );
    conn.execute("INSERT INTO events(event_type,entity_type,entity_id,summary,metadata,created_at) VALUES('agent_shutdown','agent','fixture-worker','retired',?1,?2)", params![
        r#"{"retired_agent":{"role":"worker","factory_session":"fixture-session","registered_at":"2026-01-02T00:00:00Z"}}"#, NOW]).unwrap();
    assert!(values(&conn).nudges_pass());
    assert!((values(&conn).worker_hours - 2.0).abs() < 1e-8);
    conn.execute("INSERT INTO agents(id,name,role,registered_at,last_heartbeat,factory_session) VALUES('fixture-worker','worker','worker','2026-01-02T00:00:00Z',?1,?2)", params![NOW, SESSION]).unwrap();
    assert!(
        (values(&conn).worker_hours - 2.0).abs() < 1e-8,
        "same incarnation counts once"
    );
}

#[test]
fn cas_7e24_gated_outcome_pass_and_breach() {
    let (_temp, _queue, conn) = fixture();
    assert_eq!(values(&conn).gated_without_outcome, 0);
    conn.execute(
        "UPDATE prompt_queue SET transport_delivered_at=NULL WHERE id=1",
        [],
    )
    .unwrap();
    assert_eq!(
        values(&conn).gated_without_outcome,
        0,
        "surfacing resolves a declined wake"
    );
    conn.execute(
        "DELETE FROM prompt_queue_recipient_seen WHERE prompt_id=1",
        [],
    )
    .unwrap();
    assert_eq!(values(&conn).gated_without_outcome, 1);
    conn.execute("UPDATE prompt_queue SET highest_stage='abandoned',last_pending_reason='recipient_ended_unread' WHERE id=1", []).unwrap();
    assert_eq!(
        values(&conn).gated_without_outcome,
        0,
        "terminal unread has an outcome"
    );
}

#[test]
fn cas_7e24_session_end_and_last_session_keep_the_same_five_values() {
    let (temp, _queue, conn) = fixture();
    let m = values(&conn);
    assert_eq!(m.outcome(), "PASS");
    assert_eq!(m.lines().len(), 5);
    assert_eq!(read_session(temp.path(), SESSION, now()).unwrap(), m);
    let saved = write_session_end(temp.path(), SESSION, now()).unwrap();
    assert_eq!(saved, m);
    conn.execute("DELETE FROM agents", []).unwrap();
    assert_eq!(
        last_session(temp.path(), Utc::now()).unwrap().unwrap(),
        saved,
        "doctor reads the exact session-end snapshot after registry teardown"
    );
    let persisted: String = conn
        .query_row(
            "SELECT summary_json FROM communication_slo_summaries",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<CommunicationSlos>(&persisted).unwrap(),
        saved
    );
    conn.execute("INSERT INTO agents(id,name,role,registered_at,last_heartbeat,factory_session) VALUES('later','later','worker','2026-01-03T00:00:00Z','2026-01-03T00:00:00Z','later-session')", []).unwrap();
    assert_eq!(
        last_session(temp.path(), Utc::now())
            .unwrap()
            .unwrap()
            .factory_session,
        "later-session"
    );
}

#[test]
fn cas_7e24_additive_schema_preserves_seen_pk_and_epoch() {
    let (_temp, queue, conn) = fixture();
    let epoch = values(&conn).body_observed_since;
    for _ in 0..2 {
        queue.init().unwrap();
    }
    assert_eq!(values(&conn).body_observed_since, epoch);
    let seen = "SELECT COUNT(*) FROM prompt_queue_recipient_seen WHERE prompt_id=1";
    assert_eq!(scalar(&conn, seen, []), 1);
    assert!(conn.execute("INSERT INTO prompt_queue_recipient_seen(prompt_id,recipient,seen_at,source) VALUES(1,'worker',?,'inbox_poll')", [NOW]).is_err(), "existing PK semantics preserved");
}

#[test]
fn cas_7e24_read_only_missing_store_never_creates_it() {
    let temp = tempfile::tempdir().unwrap();
    assert!(last_session(temp.path(), now()).is_err());
    assert!(!temp.path().join("cas.db").exists());
}

#[test]
fn cas_7e24_body_observation_failure_rolls_back_the_surfacing_receipt() {
    let (_temp, queue, conn) = fixture();
    let id = queue
        .enqueue_with_session("supervisor", "reader", "body", SESSION)
        .unwrap();
    conn.execute_batch("DROP TABLE communication_body_observations")
        .unwrap();
    assert!(
        queue
            .poll_unseen_for_recipient("reader", Some(SESSION), 10)
            .is_err()
    );
    let seen = "SELECT COUNT(*) FROM prompt_queue_recipient_seen WHERE prompt_id=?";
    assert_eq!(scalar(&conn, seen, [id]), 0);
    for statement in SCHEMA_STATEMENTS {
        conn.execute_batch(statement).unwrap();
    }
    assert_eq!(
        queue
            .poll_unseen_for_recipient("reader", Some(SESSION), 10)
            .unwrap()
            .len(),
        1
    );
}
