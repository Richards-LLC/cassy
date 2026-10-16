//! One occurrence per logical reader, committed with its actual prompt row.
use super::*;

pub const VIOLET_ADMISSION_SCHEMA_STATEMENTS: &[&str] = &[
    r#"
CREATE TABLE IF NOT EXISTS violet_admissions (
    delivery_id TEXT PRIMARY KEY,
    occurrence_id TEXT NOT NULL,
    reader_slot_id TEXT NOT NULL,
    registration_id TEXT NOT NULL,
    delivery_generation INTEGER NOT NULL,
    route_generation INTEGER NOT NULL,
    project_id TEXT NOT NULL,
    target_json TEXT NOT NULL,
    factory_session TEXT NOT NULL,
    recipient TEXT NOT NULL,
    prompt_id INTEGER NOT NULL REFERENCES prompt_queue(id),
    consumption TEXT NOT NULL DEFAULT 'queued'
        CHECK(consumption IN ('queued','dispatch_uncertain','consumed')),
    lease_token TEXT NOT NULL,
    ack_pending INTEGER NOT NULL DEFAULT 1,
    recovery_prompt_id INTEGER,
    UNIQUE(occurrence_id,reader_slot_id)
);"#,
    r#"
CREATE TABLE IF NOT EXISTS violet_admission_cursors (
    reader_slot_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    message_ts TEXT NOT NULL,
    PRIMARY KEY(reader_slot_id,workspace_id,channel_id)
);"#,
];

// Queue/admission invariants belong to store initialization. The migration
// runner replays only additive table/index statements for safe reconciliation.
const VIOLET_ADMISSION_TRIGGER: &str = r#"
CREATE TRIGGER IF NOT EXISTS violet_admission_identity_immutable
BEFORE UPDATE OF delivery_id,occurrence_id,reader_slot_id,project_id,target_json,prompt_id
ON violet_admissions
BEGIN SELECT RAISE(ABORT, 'Violet admission identity is immutable'); END;
"#;

#[derive(Debug, Clone)]
pub struct VioletAdmissionInput {
    pub delivery_id: String,
    pub occurrence_id: String,
    pub reader_slot_id: String,
    pub registration_id: String,
    pub delivery_generation: i64,
    pub route_generation: i64,
    pub project_id: String,
    pub target_json: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub message_ts: String,
    pub factory_session: String,
    pub recipient: String,
    pub prompt: String,
    pub lease_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> VioletAdmissionInput {
        VioletAdmissionInput {
            delivery_id: "delivery-a".into(), occurrence_id: "occurrence".into(),
            reader_slot_id: "reader-a".into(), registration_id: "registration-a".into(),
            delivery_generation: 1, route_generation: 0,
            project_id: "github.com/richards-llc/cassy".into(),
            target_json: r#"{"workspace_id":"T1","channel_id":"C1","root_ts":"1760616000.123456","message_ts":"1760616042.000200"}"#.into(),
            workspace_id: "T1".into(), channel_id: "C1".into(), message_ts: "1760616042.000200".into(),
            factory_session: "factory-a".into(), recipient: "supervisor-a".into(),
            prompt: "typed activity reference; no Slack body".into(), lease_token: "lease-a".into(),
        }
    }

    #[test]
    fn cas_c423_store_admitted_body_waits_for_confirmed_remote_uncertainty() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let admitted = store.admit(&row).unwrap();
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        assert!(queue.poll_unseen_for_recipient(&row.recipient, Some(&row.factory_session), 10)
            .unwrap().is_empty(), "admission ack alone must not authorize a body");
        assert!(queue.surface_unseen_for_recipient(&row.recipient, Some(&row.factory_session), 10)
            .unwrap().is_empty(), "hooks must share the managed body barrier");
        assert!(queue.queued_prompt(admitted.prompt_id).unwrap().is_some(),
            "blocked body remains durable for the background receipt job");
    }

    #[test]
    fn cas_c423_store_admission_is_stable_across_lost_ack_restart_and_reader_fanout() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let first = SqliteVioletAdmissionStore::open(root.path())
            .unwrap()
            .admit(&row)
            .unwrap();
        assert!(first.created);
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let duplicate = store.admit(&row).unwrap();
        assert!(!duplicate.created);
        assert_eq!(duplicate.prompt_id, first.prompt_id);
        let mut peer = row.clone();
        peer.delivery_id = "delivery-b".into();
        peer.reader_slot_id = "reader-b".into();
        peer.registration_id = "registration-b".into();
        peer.factory_session = "factory-b".into();
        peer.recipient = "supervisor-b".into();
        assert!(store.admit(&peer).unwrap().created);
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        let counts: (i64,i64,i64) = db.query_row("SELECT (SELECT COUNT(*) FROM prompt_queue),
            (SELECT COUNT(*) FROM violet_admissions), (SELECT COUNT(*) FROM violet_admission_cursors)",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(counts, (2, 2, 2));
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        assert_eq!(
            queue
                .queued_prompt(first.prompt_id)
                .unwrap()
                .unwrap()
                .origin,
            Some(QueueOrigin::Daemon)
        );
        let mut conflict = row.clone();
        conflict.registration_id = "replacement".into();
        assert!(
            store.admit(&conflict).is_err(),
            "new registration requires an authorized rebind"
        );
    }

    #[test]
    fn cas_c423_store_cursor_write_failure_rolls_back_queue_and_admission() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_cursor BEFORE INSERT ON violet_admission_cursors
            BEGIN SELECT RAISE(ABORT, 'cursor write failed'); END;",
        )
        .unwrap();
        let error = store.admit(&input()).unwrap_err().to_string();
        assert!(error.contains("cursor write failed"), "{error}");
        for table in [
            "prompt_queue",
            "violet_admissions",
            "violet_admission_cursors",
        ] {
            let count: i64 = db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} must roll back");
        }
    }

    #[test]
    fn cas_c423_store_ack_settlement_fences_old_tokens_and_foreign_registrations() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let mut row = input();
        store.admit(&row).unwrap();
        let old = store
            .pending_acknowledgements(&row.registration_id, &row.project_id)
            .unwrap()
            .remove(0);
        row.lease_token = "replacement-token".into();
        store.admit(&row).unwrap();
        assert!(
            !store
                .settle_acknowledgement(&old, &row.registration_id, &row.project_id)
                .unwrap()
        );
        let pending = store
            .pending_acknowledgements(&row.registration_id, &row.project_id)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            store
                .pending_acknowledgements("foreign-registration", &row.project_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            !store
                .settle_acknowledgement(&pending[0], "foreign-registration", &row.project_id)
                .unwrap()
        );
        assert!(
            store
                .settle_acknowledgement(&pending[0], &row.registration_id, &row.project_id)
                .unwrap()
        );
        assert!(
            store
                .pending_acknowledgements(&row.registration_id, &row.project_id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cas_c423_store_independent_connections_race_one_admission_and_keep_cursor_monotonic() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut threads = vec![];
        for _ in 0..2 {
            // Bypass the process connection pool to exercise the SQLite writer
            // fence across genuinely independent handles, as on two processes.
            let conn = Connection::open(root.path().join("cas.db")).unwrap();
            conn.busy_timeout(crate::SQLITE_BUSY_TIMEOUT).unwrap();
            let queue = SqlitePromptQueueStore::with_connection(std::sync::Arc::new(
                std::sync::Mutex::new(conn),
            ));
            let separate = SqliteVioletAdmissionStore { queue };
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                separate.admit(&input()).unwrap()
            }));
        }
        let receipts: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(receipts[0].prompt_id, receipts[1].prompt_id);
        assert_ne!(receipts[0].created, receipts[1].created);
        let mut older = input();
        older.delivery_id = "older-delivery".into();
        older.occurrence_id = "older-occurrence".into();
        older.message_ts = "1760616042.000199".into();
        store.admit(&older).unwrap();
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        let cursor = || {
            db.query_row("SELECT message_ts FROM violet_admission_cursors", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
        };
        assert_eq!(cursor(), input().message_ts);
        older.delivery_id = "newer-delivery".into();
        older.occurrence_id = "newer-occurrence".into();
        older.message_ts = "1760616042.0002001".into();
        store.admit(&older).unwrap();
        assert_eq!(cursor(), older.message_ts, "preserve all timestamp digits");
    }

    fn successor(row: &VioletAdmissionInput) -> VioletAdmissionRebind {
        VioletAdmissionRebind {
            delivery_id: row.delivery_id.clone(),
            occurrence_id: row.occurrence_id.clone(),
            reader_slot_id: row.reader_slot_id.clone(),
            project_id: row.project_id.clone(),
            registration_id: "registration-successor".into(),
            expected_generation: 1,
            delivery_generation: 2,
            factory_session: "factory-successor".into(),
            recipient: "supervisor-successor".into(),
        }
    }

    #[test]
    fn cas_c423_store_known_unread_rebind_moves_same_queue_row_and_fences_predecessor() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let first = SqliteVioletAdmissionStore::open(root.path())
            .unwrap()
            .admit(&row)
            .unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let binding = successor(&row);
        let adopted = store.adopt_unread(&binding).unwrap();
        assert_eq!(adopted.prompt_id, first.prompt_id);
        assert!(!adopted.created);
        assert_eq!(
            store.adopt_unread(&binding).unwrap(),
            adopted,
            "lost local result is idempotent"
        );
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        assert!(
            queue
                .peek_unseen_for_recipient(&row.recipient, Some(&row.factory_session), 10)
                .unwrap()
                .is_empty()
        );
        let resumed = queue
            .peek_unseen_for_recipient(&binding.recipient, Some(&binding.factory_session), 10)
            .unwrap();
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].id, first.prompt_id);
        assert!(
            store.admit(&row).is_err(),
            "old generation cannot refresh its token after adoption"
        );
        let mut stale = binding.clone();
        stale.registration_id = "racing-successor".into();
        assert!(store.adopt_unread(&stale).is_err());
    }

    #[test]
    fn cas_c423_store_successor_lease_before_ack_preserves_admission_and_fences_old_poll() {
        let root = tempfile::tempdir().unwrap();
        let old = input();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let first = store.admit(&old).unwrap();
        let fence = store
            .pending_lease_fences(&old.project_id)
            .unwrap()
            .pop()
            .unwrap();
        let old_ack = store
            .pending_acknowledgements(&old.registration_id, &old.project_id)
            .unwrap()
            .pop()
            .unwrap();
        let mut next = old.clone();
        next.registration_id = "successor-registration".into();
        next.recipient = "successor-supervisor".into();
        next.factory_session = "successor-factory".into();
        next.lease_token = "successor-lease".into();
        let resumed = store.adopt_lease(&fence, &next).unwrap();
        assert_eq!(resumed.admission.prompt_id, first.prompt_id);
        assert_eq!(resumed.consumption, VioletConsumption::Queued);
        assert_eq!(resumed.recovery_prompt_id, None);
        let pending = store
            .pending_acknowledgements(&next.registration_id, &next.project_id)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].delivery_generation, 1,
            "Cloud re-leases without advancing generation"
        );
        assert_eq!(pending[0].lease_token, next.lease_token);
        assert!(
            !store
                .settle_acknowledgement(&old_ack, &old.registration_id, &old.project_id)
                .unwrap()
        );
        assert!(
            store.adopt_lease(&fence, &old).is_err(),
            "a poll begun before takeover cannot move the admission back"
        );
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        assert!(
            queue
                .peek_unseen_for_recipient(&old.recipient, Some(&old.factory_session), 10)
                .unwrap()
                .is_empty()
        );
        let inbox = queue
            .peek_unseen_for_recipient(&next.recipient, Some(&next.factory_session), 10)
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].id, first.prompt_id);
    }

    #[test]
    fn cas_c423_store_rebind_rolls_back_both_routes_and_refuses_transport_evidence() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let first = store.admit(&row).unwrap();
        let binding = successor(&row);
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_rebind BEFORE UPDATE OF registration_id ON violet_admissions
            BEGIN SELECT RAISE(ABORT, 'rebind write failed'); END;",
        )
        .unwrap();
        let error = store.adopt_unread(&binding).unwrap_err().to_string();
        assert!(error.contains("rebind write failed"), "{error}");
        let state: (String, String) = db
            .query_row(
                "SELECT q.target,a.registration_id FROM violet_admissions a
            JOIN prompt_queue q ON a.prompt_id=q.id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (row.recipient.clone(), row.registration_id));
        db.execute_batch("DROP TRIGGER fail_rebind").unwrap();
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        queue.mark_transport_delivered(first.prompt_id).unwrap();
        let error = store.adopt_unread(&binding).unwrap_err().to_string();
        assert!(error.contains("consumption recovery"), "{error}");
        assert!(
            queue
                .peek_unseen_for_recipient(&binding.recipient, Some(&binding.factory_session), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            queue.peek_all(10).unwrap().len(),
            0,
            "transport uncertainty does not replay the ask"
        );
    }

    #[test]
    fn cas_c423_store_uncertain_recovery_keeps_one_reference_and_never_replays_ask() {
        let root = tempfile::tempdir().unwrap();
        let mut row = input();
        row.prompt = "ASK BODY MUST NEVER REPLAY".into();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let first = store.admit(&row).unwrap();
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        queue.mark_transport_delivered(first.prompt_id).unwrap();
        let binding = successor(&row);
        let recovered = store
            .recover_known(&binding, VioletConsumption::Queued)
            .unwrap();
        assert_eq!(recovered.admission.prompt_id, first.prompt_id);
        assert_eq!(recovered.consumption, VioletConsumption::DispatchUncertain);
        let notice_id = recovered
            .recovery_prompt_id
            .expect("uncertain dispatch needs a recovery reference");
        let pending = queue
            .peek_unseen_for_recipient(&binding.recipient, Some(&binding.factory_session), 10)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, notice_id);
        assert!(pending[0].prompt.contains(&row.delivery_id));
        assert!(!pending[0].prompt.contains("ASK BODY"));
        assert_eq!(pending[0].origin, Some(QueueOrigin::Daemon));
        let reopened = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        assert_eq!(
            reopened
                .recover_known(&binding, VioletConsumption::Queued)
                .unwrap(),
            recovered
        );
        assert!(
            queue
                .peek_unseen_for_recipient(&row.recipient, Some(&row.factory_session), 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store.admit(&row).is_err(),
            "predecessor cannot revive its original route"
        );
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        let counts: (i64, i64) = db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM violet_admissions),
            (SELECT COUNT(*) FROM prompt_queue)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 2));
    }

    #[test]
    fn cas_c423_store_consumed_recipient_receipt_wins_over_remote_unread() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let first = store.admit(&row).unwrap();
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        assert_eq!(
            queue
                .surface_unseen_for_recipient(&row.recipient, Some(&row.factory_session), 10)
                .unwrap()
                .len(),
            1
        );
        let binding = successor(&row);
        let recovered = store
            .recover_known(&binding, VioletConsumption::Queued)
            .unwrap();
        assert_eq!(recovered.admission.prompt_id, first.prompt_id);
        assert_eq!(recovered.consumption, VioletConsumption::Consumed);
        assert_eq!(recovered.recovery_prompt_id, None);
        assert!(
            queue
                .peek_unseen_for_recipient(&binding.recipient, Some(&binding.factory_session), 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store.adopt_unread(&binding).is_err(),
            "consumed cannot regress to queued"
        );
    }

    #[test]
    fn cas_c423_store_explicit_ack_is_consumed_on_successor_recovery() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let first = store.admit(&row).unwrap();
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        queue.ack(first.prompt_id).unwrap();
        let binding = successor(&row);
        let recovered = store
            .recover_known(&binding, VioletConsumption::Queued)
            .unwrap();
        assert_eq!(recovered.admission.prompt_id, first.prompt_id);
        assert_eq!(recovered.consumption, VioletConsumption::Consumed);
        assert_eq!(recovered.recovery_prompt_id, None);
        assert!(
            queue
                .peek_unseen_for_recipient(&binding.recipient, Some(&binding.factory_session), 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cas_c423_store_fresh_restore_is_atomic_idempotent_and_never_reacks() {
        for consumption in [
            VioletConsumption::Queued,
            VioletConsumption::DispatchUncertain,
            VioletConsumption::Consumed,
        ] {
            let root = tempfile::tempdir().unwrap();
            let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
            let mut row = input();
            row.lease_token.clear();
            row.delivery_generation = 2;
            let first = store.restore_admitted(&row, 1, consumption).unwrap();
            let duplicate = store.restore_admitted(&row, 1, consumption).unwrap();
            assert!(first.admission.created);
            assert!(!duplicate.admission.created);
            assert_eq!(duplicate.admission.prompt_id, first.admission.prompt_id);
            assert_eq!(duplicate.recovery_prompt_id, first.recovery_prompt_id);
            assert_eq!(duplicate.consumption, consumption);
            assert!(
                store
                    .pending_acknowledgements(&row.registration_id, &row.project_id)
                    .unwrap()
                    .is_empty()
            );
            let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
            let inbox = queue
                .peek_unseen_for_recipient(&row.recipient, Some(&row.factory_session), 10)
                .unwrap();
            match consumption {
                VioletConsumption::Queued => assert_eq!(inbox[0].id, first.admission.prompt_id),
                VioletConsumption::DispatchUncertain => {
                    assert_eq!(inbox.len(), 1);
                    assert_eq!(Some(inbox[0].id), first.recovery_prompt_id);
                    assert!(!inbox[0].prompt.contains(&row.prompt));
                }
                VioletConsumption::Consumed => assert!(inbox.is_empty()),
            }
            let db = Connection::open(root.path().join("cas.db")).unwrap();
            let cursor: String = db
                .query_row("SELECT message_ts FROM violet_admission_cursors", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(cursor, row.message_ts);
        }
    }

    #[test]
    fn cas_c423_store_fresh_restore_cursor_failure_rolls_back_everything() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_restore BEFORE INSERT ON violet_admission_cursors
            BEGIN SELECT RAISE(ABORT,'restore cursor failed'); END;",
        )
        .unwrap();
        let mut row = input();
        row.lease_token.clear();
        assert!(
            store
                .restore_admitted(&row, 1, VioletConsumption::DispatchUncertain)
                .is_err()
        );
        let counts: (i64, i64) = db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM violet_admissions),
            (SELECT COUNT(*) FROM prompt_queue)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn cas_c423_store_recovery_notice_and_retirement_roll_back_together() {
        let root = tempfile::tempdir().unwrap();
        let row = input();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let first = store.admit(&row).unwrap();
        let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
        queue
            .select_for_targets(&[&row.recipient], Some(&row.factory_session), 10)
            .unwrap();
        let db = Connection::open(root.path().join("cas.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_recovery BEFORE UPDATE OF consumption ON violet_admissions
            BEGIN SELECT RAISE(ABORT, 'recovery commit failed'); END;",
        )
        .unwrap();
        let error = store
            .recover_known(&successor(&row), VioletConsumption::Queued)
            .unwrap_err()
            .to_string();
        assert!(error.contains("recovery commit failed"), "{error}");
        let before: (i64, String, Option<String>) = db
            .query_row(
                "SELECT
            (SELECT COUNT(*) FROM prompt_queue),a.registration_id,q.processed_at
            FROM violet_admissions a JOIN prompt_queue q ON q.id=a.prompt_id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(before, (1, row.registration_id.clone(), None));
        db.execute_batch("DROP TRIGGER fail_recovery").unwrap();
        assert_eq!(
            store
                .recover_known(&successor(&row), VioletConsumption::Queued)
                .unwrap()
                .admission
                .prompt_id,
            first.prompt_id
        );
    }

    #[test]
    fn cas_c423_store_independent_recovery_races_keep_one_notice_and_one_successor() {
        for competing_successors in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
            store.admit(&input()).unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let mut threads = vec![];
            for index in 0..2 {
                let conn = Connection::open(root.path().join("cas.db")).unwrap();
                conn.busy_timeout(crate::SQLITE_BUSY_TIMEOUT).unwrap();
                let queue = SqlitePromptQueueStore::with_connection(std::sync::Arc::new(
                    std::sync::Mutex::new(conn),
                ));
                let separate = SqliteVioletAdmissionStore { queue };
                let barrier = barrier.clone();
                let mut binding = successor(&input());
                if competing_successors {
                    binding.registration_id = format!("successor-{index}");
                    binding.recipient = format!("supervisor-{index}");
                }
                threads.push(std::thread::spawn(move || {
                    barrier.wait();
                    separate.recover_known(&binding, VioletConsumption::DispatchUncertain)
                }));
            }
            let receipts: Vec<_> = threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect();
            assert_eq!(
                receipts.iter().filter(|result| result.is_ok()).count(),
                if competing_successors { 1 } else { 2 }
            );
            for error in receipts.iter().filter_map(|result| result.as_ref().err()) {
                assert!(
                    error
                        .to_string()
                        .contains("stale Violet delivery generation"),
                    "{error}"
                );
            }
            let db = Connection::open(root.path().join("cas.db")).unwrap();
            let state: (i64, i64, i64) = db
                .query_row(
                    "SELECT delivery_generation,
                (SELECT COUNT(*) FROM violet_admissions), (SELECT COUNT(*) FROM prompt_queue)
                FROM violet_admissions",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                state,
                (2, 1, 2),
                "one original admission and one notice across writers"
            );
            if !competing_successors {
                assert_eq!(receipts[0].as_ref().unwrap(), receipts[1].as_ref().unwrap());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VioletConsumption {
    Queued,
    DispatchUncertain,
    Consumed,
}

impl VioletConsumption {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::DispatchUncertain => "dispatch_uncertain",
            Self::Consumed => "consumed",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "dispatch_uncertain" => Ok(Self::DispatchUncertain),
            "consumed" => Ok(Self::Consumed),
            _ => Err(StoreError::Other("unknown Violet consumption state".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VioletRecoveryReceipt {
    pub admission: VioletAdmissionReceipt,
    pub consumption: VioletConsumption,
    pub recovery_prompt_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VioletAdmissionReceipt {
    pub delivery_id: String,
    pub prompt_id: i64,
    pub created: bool,
}

#[derive(Debug, Clone)]
pub struct VioletAdmissionAck {
    pub delivery_id: String,
    pub delivery_generation: i64,
    pub lease_token: String,
}

/// Snapshot taken before the remote lease call. Its token fences a delayed
/// predecessor poll even when Cloud retains the same delivery generation.
#[derive(Debug, Clone)]
pub struct VioletLeaseFence {
    pub delivery_id: String,
    registration_id: String,
    delivery_generation: i64,
    lease_token: String,
}

/// Apply only a validated Cloud rebind receipt for an existing local unread
/// admission. This is not discovery, ownership, or dispatch authorization.
#[derive(Debug, Clone)]
pub struct VioletAdmissionRebind {
    pub delivery_id: String,
    pub occurrence_id: String,
    pub reader_slot_id: String,
    pub project_id: String,
    pub registration_id: String,
    pub expected_generation: i64,
    pub delivery_generation: i64,
    pub factory_session: String,
    pub recipient: String,
}

pub struct SqliteVioletAdmissionStore {
    queue: SqlitePromptQueueStore,
}

impl SqliteVioletAdmissionStore {
    /// Restore Cloud's existing admission on a successor machine. No lease
    /// token or new admission acknowledgement is invented by this path.
    pub fn restore_admitted(
        &self,
        input: &VioletAdmissionInput,
        expected_generation: i64,
        consumption: VioletConsumption,
    ) -> Result<VioletRecoveryReceipt> {
        if !input.lease_token.is_empty() || expected_generation < 1 {
            return Err(StoreError::Other(
                "restore requires an admitted delivery without a lease".into(),
            ));
        }
        self.persist(input, Some((expected_generation, consumption)))
    }
    pub fn pending_lease_fences(&self, project: &str) -> Result<Vec<VioletLeaseFence>> {
        let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT delivery_id,registration_id,delivery_generation,lease_token
            FROM violet_admissions WHERE project_id=?1 AND ack_pending=1",
        )?;
        let rows = stmt.query_map([project], |r| {
            Ok(VioletLeaseFence {
                delivery_id: r.get(0)?,
                registration_id: r.get(1)?,
                delivery_generation: r.get(2)?,
                lease_token: r.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Apply only a fresh authenticated lease obtained AFTER taking `fence`.
    /// The pre-call token CAS prevents an old response from undoing takeover,
    /// including when Cloud has kept the same delivery generation.
    pub fn adopt_lease(
        &self,
        fence: &VioletLeaseFence,
        input: &VioletAdmissionInput,
    ) -> Result<VioletRecoveryReceipt> {
        if fence.delivery_id != input.delivery_id
            || fence.delivery_generation != input.delivery_generation
            || input.lease_token.trim().is_empty()
        {
            return Err(StoreError::Other(
                "invalid successor lease identity or generation".into(),
            ));
        }
        let binding = VioletAdmissionRebind {
            delivery_id: input.delivery_id.clone(),
            occurrence_id: input.occurrence_id.clone(),
            reader_slot_id: input.reader_slot_id.clone(),
            project_id: input.project_id.clone(),
            registration_id: input.registration_id.clone(),
            expected_generation: fence.delivery_generation,
            delivery_generation: input.delivery_generation,
            factory_session: input.factory_session.clone(),
            recipient: input.recipient.clone(),
        };
        validate_rebind(&binding)?;
        let receipt = crate::shared_db::with_immediate_write_txn_pooled(&self.queue.conn, |tx| {
            let matches: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM violet_admissions
                WHERE delivery_id=?1 AND registration_id=?2 AND delivery_generation=?3
                  AND lease_token=?4 AND ack_pending=1 AND target_json=?5 AND route_generation=?6
                  AND occurrence_id=?7 AND reader_slot_id=?8 AND project_id=?9)",
                params![
                    input.delivery_id,
                    fence.registration_id,
                    fence.delivery_generation,
                    fence.lease_token,
                    input.target_json,
                    input.route_generation,
                    input.occurrence_id,
                    input.reader_slot_id,
                    input.project_id
                ],
                |r| r.get(0),
            )?;
            if !matches {
                return Err(StoreError::Other(
                    "stale Violet pre-claim lease snapshot".into(),
                ));
            }
            let receipt = recover_known_in_tx(
                tx,
                &binding,
                VioletConsumption::Queued,
                RecoveryAuthority::Lease,
            )?;
            tx.execute(
                "UPDATE violet_admissions SET lease_token=?2,ack_pending=1 WHERE delivery_id=?1",
                params![input.delivery_id, input.lease_token],
            )?;
            Ok(receipt)
        })?;
        if receipt.consumption != VioletConsumption::Consumed {
            self.queue.signal_inbox(&input.recipient);
        }
        Ok(receipt)
    }

    /// Apply a validated successor receipt without inferring unread from a
    /// dead process. Local recipient and transport evidence remains binding.
    pub fn recover_known(
        &self,
        binding: &VioletAdmissionRebind,
        remote: VioletConsumption,
    ) -> Result<VioletRecoveryReceipt> {
        validate_rebind(binding)?;
        let receipt = crate::shared_db::with_immediate_write_txn_pooled(&self.queue.conn, |tx| {
            recover_known_in_tx(tx, binding, remote, RecoveryAuthority::Rebind)
        })?;
        if receipt.consumption != VioletConsumption::Consumed {
            self.queue.signal_inbox(&binding.recipient);
        }
        Ok(receipt)
    }

    pub fn open(cas_dir: &Path) -> Result<Self> {
        let queue = SqlitePromptQueueStore::open(cas_dir)?;
        queue.init()?;
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&queue.conn)?;
            for statement in VIOLET_ADMISSION_SCHEMA_STATEMENTS {
                conn.execute(statement, [])?;
            }
            conn.execute_batch(VIOLET_ADMISSION_TRIGGER)?;
            Ok(())
        })?;
        Ok(Self { queue })
    }

    /// Move an existing, never-selected queue row after Cloud authorized its
    /// rebind. Any local dispatch/read evidence requires consumption recovery,
    /// not this unread path. In-flight dispatch still requires a runtime fence.
    pub fn adopt_unread(&self, binding: &VioletAdmissionRebind) -> Result<VioletAdmissionReceipt> {
        validate_rebind(binding)?;
        let (receipt, moved) = crate::shared_db::with_immediate_write_txn_pooled(
            &self.queue.conn,
            |tx| {
                let current: Option<(i64, String, i64, String, String, String)> = tx.query_row(
                "SELECT prompt_id,registration_id,delivery_generation,factory_session,recipient,consumption
                 FROM violet_admissions WHERE delivery_id=?1 AND occurrence_id=?2
                   AND reader_slot_id=?3 AND project_id=?4",
                params![binding.delivery_id,binding.occurrence_id,binding.reader_slot_id,binding.project_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))
            ).optional()?;
                let Some((prompt_id, registration, generation, session, recipient, consumption)) =
                    current
                else {
                    return Err(StoreError::NotFound(
                        "known local Violet admission; remote discovery required".into(),
                    ));
                };
                if consumption != "queued" {
                    return Err(StoreError::Other(
                        "Violet consumption requires recovery, not unread adoption".into(),
                    ));
                }
                let receipt = VioletAdmissionReceipt {
                    delivery_id: binding.delivery_id.clone(),
                    prompt_id,
                    created: false,
                };
                if registration == binding.registration_id
                    && generation == binding.delivery_generation
                    && session == binding.factory_session
                    && recipient == binding.recipient
                {
                    return Ok((receipt, false));
                }
                let next_generation =
                    generation.checked_add(i64::from(registration != binding.registration_id));
                if generation != binding.expected_generation
                    || next_generation != Some(binding.delivery_generation)
                {
                    return Err(StoreError::Other(
                        "stale Violet delivery generation on rebind".into(),
                    ));
                }
                let untouched: bool = tx.query_row(
                "SELECT processed_at IS NULL AND selected_at IS NULL
                    AND NOT EXISTS (SELECT 1 FROM prompt_queue_recipient_seen WHERE prompt_id=q.id)
                    AND NOT EXISTS (SELECT 1 FROM prompt_queue_recipient_transport WHERE prompt_id=q.id)
                 FROM prompt_queue q WHERE id=?1 AND target=?2 AND factory_session=?3",
                params![prompt_id,recipient,session], |r|r.get(0)
            ).optional()?.unwrap_or(false);
                if !untouched {
                    return Err(StoreError::Other(
                        "Violet dispatch/read evidence requires consumption recovery".into(),
                    ));
                }
                tx.execute(
                    "UPDATE prompt_queue SET target=?2,factory_session=?3 WHERE id=?1",
                    params![prompt_id, binding.recipient, binding.factory_session],
                )?;
                tx.execute(
                    "UPDATE violet_admissions SET registration_id=?2,delivery_generation=?3,
                factory_session=?4,recipient=?5,ack_pending=0 WHERE delivery_id=?1",
                    params![
                        binding.delivery_id,
                        binding.registration_id,
                        binding.delivery_generation,
                        binding.factory_session,
                        binding.recipient
                    ],
                )?;
                Ok((receipt, true))
            },
        )?;
        if moved {
            self.queue.signal_inbox(&binding.recipient);
        }
        Ok(receipt)
    }

    /// Includes admissions already acknowledged remotely: a lost response must
    /// not depend on Cloud leasing that admitted row again.
    pub fn pending_acknowledgements(
        &self,
        registration: &str,
        project: &str,
    ) -> Result<Vec<VioletAdmissionAck>> {
        let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT delivery_id,delivery_generation,lease_token FROM violet_admissions
             WHERE registration_id=?1 AND project_id=?2 AND ack_pending=1
             ORDER BY rowid LIMIT 25",
        )?;
        let rows = stmt.query_map(params![registration, project], |row| {
            Ok(VioletAdmissionAck {
                delivery_id: row.get(0)?,
                delivery_generation: row.get(1)?,
                lease_token: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// A late response from a previous lease or registration cannot settle a
    /// replacement's pending work. The caller verifies the individual remote
    /// result is completed/admitted before invoking this compare-and-set.
    pub fn settle_acknowledgement(
        &self,
        ack: &VioletAdmissionAck,
        registration: &str,
        project: &str,
    ) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.queue.conn)?;
            Ok(conn.execute(
                "UPDATE violet_admissions SET ack_pending=0
                 WHERE delivery_id=?1 AND delivery_generation=?2 AND lease_token=?3
                   AND registration_id=?4 AND project_id=?5 AND ack_pending=1",
                params![
                    ack.delivery_id,
                    ack.delivery_generation,
                    ack.lease_token,
                    registration,
                    project
                ],
            )? == 1)
        })
    }

    /// Only a committed receipt may authorize an admission acknowledgement.
    pub fn admit(&self, input: &VioletAdmissionInput) -> Result<VioletAdmissionReceipt> {
        if input.lease_token.trim().is_empty() {
            return Err(StoreError::Other(
                "Violet admission requires a lease token".into(),
            ));
        }
        self.persist(input, None).map(|receipt| receipt.admission)
    }

    fn persist(
        &self,
        input: &VioletAdmissionInput,
        recovery: Option<(i64, VioletConsumption)>,
    ) -> Result<VioletRecoveryReceipt> {
        for value in [
            &input.delivery_id,
            &input.occurrence_id,
            &input.reader_slot_id,
            &input.registration_id,
            &input.project_id,
            &input.workspace_id,
            &input.channel_id,
            &input.factory_session,
            &input.recipient,
        ] {
            if value.trim().is_empty() {
                return Err(StoreError::Other(
                    "Violet admission has an empty identity".into(),
                ));
            }
        }
        if input.delivery_generation < 1 || input.route_generation < 0 {
            return Err(StoreError::Other(
                "invalid Violet delivery generation".into(),
            ));
        }
        slack_timestamp_parts(&input.message_ts)?;
        let recover = |tx: &Connection, expected_generation, consumption| {
            recover_known_in_tx(
                tx,
                &VioletAdmissionRebind {
                    delivery_id: input.delivery_id.clone(),
                    occurrence_id: input.occurrence_id.clone(),
                    reader_slot_id: input.reader_slot_id.clone(),
                    project_id: input.project_id.clone(),
                    registration_id: input.registration_id.clone(),
                    expected_generation,
                    delivery_generation: input.delivery_generation,
                    factory_session: input.factory_session.clone(),
                    recipient: input.recipient.clone(),
                },
                consumption,
                RecoveryAuthority::Rebind,
            )
        };
        let receipt = crate::shared_db::with_immediate_write_txn_pooled(&self.queue.conn, |tx| {
            let existing: Option<(String, i64, String, i64, String, String, String, String)> = tx
                .query_row(
                    "SELECT delivery_id,prompt_id,registration_id,delivery_generation,project_id,
                        target_json,factory_session,recipient FROM violet_admissions
                 WHERE occurrence_id=?1 AND reader_slot_id=?2",
                    params![input.occurrence_id, input.reader_slot_id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((
                id,
                prompt_id,
                registration,
                generation,
                project,
                target,
                session,
                recipient,
            )) = existing
            {
                if id != input.delivery_id
                    || project != input.project_id
                    || target != input.target_json
                {
                    return Err(StoreError::Other(
                        "conflicting restored Violet identity or target".into(),
                    ));
                }
                if let Some((expected, consumption)) = recovery {
                    return recover(tx, expected, consumption);
                }
                if registration != input.registration_id
                    || generation != input.delivery_generation
                    || session != input.factory_session
                    || recipient != input.recipient
                {
                    return Err(StoreError::Other(
                        "conflicting Violet admission identity or registration; authorized rebind required".into()));
                }
                tx.execute("UPDATE violet_admissions SET lease_token=?2,ack_pending=1 WHERE delivery_id=?1",
                    params![id,input.lease_token])?;
                return Ok(VioletRecoveryReceipt {
                    admission: VioletAdmissionReceipt {
                        delivery_id: id,
                        prompt_id,
                        created: false,
                    },
                    consumption: VioletConsumption::Queued,
                    recovery_prompt_id: None,
                });
            }
            let key =
                serde_json::to_string(&["violet-v2", &input.occurrence_id, &input.reader_slot_id])
                    .map_err(|error| StoreError::Other(error.to_string()))?;
            let source = format!("violet-activity:{}", input.channel_id);
            tx.execute("INSERT INTO prompt_queue
                (source,target,prompt,created_at,factory_session,summary,priority,urgent,dedupe_key,origin_kind)
                VALUES (?1,?2,?3,?4,?5,'Managed Slack occurrence',?6,0,?7,'daemon')",
                params![source,input.recipient,input.prompt,Utc::now().to_rfc3339(),input.factory_session,
                    i32::from(NotificationPriority::High),key])?;
            let prompt_id = tx.last_insert_rowid();
            tx.execute("INSERT INTO violet_admissions
                (delivery_id,occurrence_id,reader_slot_id,registration_id,delivery_generation,
                 route_generation,project_id,target_json,factory_session,recipient,prompt_id,lease_token,ack_pending)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![input.delivery_id,input.occurrence_id,input.reader_slot_id,input.registration_id,
                    input.delivery_generation,input.route_generation,input.project_id,input.target_json,
                    input.factory_session,input.recipient,prompt_id,input.lease_token,i64::from(recovery.is_none())])?;
            let cursor: Option<String> = tx
                .query_row(
                    "SELECT message_ts FROM violet_admission_cursors
                WHERE reader_slot_id=?1 AND workspace_id=?2 AND channel_id=?3",
                    params![input.reader_slot_id, input.workspace_id, input.channel_id],
                    |r| r.get(0),
                )
                .optional()?;
            let advance = match cursor {
                None => true,
                Some(cursor) => slack_timestamp_after(&input.message_ts, &cursor)?,
            };
            if advance {
                tx.execute("INSERT INTO violet_admission_cursors(reader_slot_id,workspace_id,channel_id,message_ts)
                    VALUES (?1,?2,?3,?4) ON CONFLICT(reader_slot_id,workspace_id,channel_id)
                    DO UPDATE SET message_ts=excluded.message_ts",
                    params![input.reader_slot_id,input.workspace_id,input.channel_id,input.message_ts])?;
            }
            let _ = capture_message_event(tx, &source, &input.recipient);
            let mut receipt = if let Some((expected, consumption)) = recovery {
                recover(tx, expected, consumption)?
            } else {
                VioletRecoveryReceipt {
                    admission: VioletAdmissionReceipt {
                        delivery_id: input.delivery_id.clone(),
                        prompt_id,
                        created: false,
                    },
                    consumption: VioletConsumption::Queued,
                    recovery_prompt_id: None,
                }
            };
            receipt.admission.created = true;
            Ok(receipt)
        })?;
        if receipt.admission.created && receipt.consumption != VioletConsumption::Consumed {
            self.queue.signal_inbox(&input.recipient);
        }
        Ok(receipt)
    }
}

#[derive(Clone, Copy)]
enum RecoveryAuthority {
    Rebind,
    Lease,
}

fn recover_known_in_tx(
    tx: &Connection,
    binding: &VioletAdmissionRebind,
    remote: VioletConsumption,
    authority: RecoveryAuthority,
) -> Result<VioletRecoveryReceipt> {
    let current: Option<(i64, String, i64, String, String, String, Option<i64>)> = tx.query_row(
        "SELECT prompt_id,registration_id,delivery_generation,factory_session,recipient,consumption,recovery_prompt_id
         FROM violet_admissions WHERE delivery_id=?1 AND occurrence_id=?2
           AND reader_slot_id=?3 AND project_id=?4",
        params![binding.delivery_id,binding.occurrence_id,binding.reader_slot_id,binding.project_id],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))
    ).optional()?;
    let Some((
        prompt_id,
        registration,
        generation,
        session,
        recipient,
        stored,
        mut recovery_prompt_id,
    )) = current
    else {
        return Err(StoreError::NotFound(
            "known local Violet admission; remote discovery required".into(),
        ));
    };
    let same_route = registration == binding.registration_id
        && generation == binding.delivery_generation
        && session == binding.factory_session
        && recipient == binding.recipient;
    let next_generation = match authority {
        RecoveryAuthority::Rebind => {
            generation.checked_add(i64::from(registration != binding.registration_id))
        }
        RecoveryAuthority::Lease => Some(generation),
    };
    if !same_route
        && (generation != binding.expected_generation
            || next_generation != Some(binding.delivery_generation))
    {
        return Err(StoreError::Other(
            "stale Violet delivery generation on recovery".into(),
        ));
    }
    // Recipient-side full surfacing is distinct from a daemon's
    // transport handoff. Selection alone is conservatively uncertain.
    let (surfaced, touched): (bool, bool) = tx.query_row(
        "SELECT (acked_at IS NOT NULL AND acked_via='explicit_ack')
           OR EXISTS(SELECT 1 FROM prompt_queue_recipient_seen
             WHERE prompt_id=q.id AND recipient=?2
               AND source IN ('hook_surfaced','inbox_poll','observed_wake')),
           selected_at IS NOT NULL OR processed_at IS NOT NULL
             OR transport_delivered_at IS NOT NULL
             OR EXISTS(SELECT 1 FROM prompt_queue_recipient_seen WHERE prompt_id=q.id)
             OR EXISTS(SELECT 1 FROM prompt_queue_recipient_transport WHERE prompt_id=q.id)
         FROM prompt_queue q WHERE id=?1",
        params![prompt_id, recipient],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let evidence = if surfaced {
        VioletConsumption::Consumed
    } else if touched {
        VioletConsumption::DispatchUncertain
    } else {
        VioletConsumption::Queued
    };
    let consumption = remote.max(VioletConsumption::parse(&stored)?).max(evidence);
    if consumption == VioletConsumption::Queued {
        tx.execute(
            "UPDATE prompt_queue SET target=?2,factory_session=?3 WHERE id=?1",
            params![prompt_id, binding.recipient, binding.factory_session],
        )?;
    } else {
        // Preserve transport/recipient history and its monotonic stage.
        // A retired ask is history, never an inbox item for either process.
        let stage = SqlitePromptQueueStore::read_highest_stage(tx, prompt_id)?;
        let stage = if stage.rank() >= DeliveryStage::Suppressed.rank() {
            stage
        } else {
            DeliveryStage::Suppressed
        };
        SqlitePromptQueueStore::atomic_stage_stamp_in_tx(
            tx,
            prompt_id,
            stage,
            AtomicStampOpts {
                reason: Some(PendingReason::SupersededStale),
                detail: Some("Violet successor recovery retired the original ask"),
                set_processed: true,
                broadcast_attempted: None,
                broadcast_succeeded: None,
                broadcast_failed: None,
            },
        )?;
        tx.execute(
            "UPDATE prompt_queue SET target='violet-admission-history' WHERE id=?1",
            [prompt_id],
        )?;
        if consumption == VioletConsumption::DispatchUncertain {
            if let Some(notice_id) = recovery_prompt_id {
                // Move the existing notice; never reset its read or
                // transport receipts, even on a further successor.
                tx.execute(
                    "UPDATE prompt_queue SET target=?2,factory_session=?3 WHERE id=?1",
                    params![notice_id, binding.recipient, binding.factory_session],
                )?;
            } else {
                let reference = serde_json::to_string(&binding.delivery_id)
                    .map_err(|error| StoreError::Other(error.to_string()))?;
                let prompt = format!(
                    "Managed Slack recovery notice. Admission {reference} may already have reached the previous supervisor. Do not replay its ask. Inspect the verified thread and obtain current claim authority before acting."
                );
                let key = serde_json::to_string(&["violet-recovery", &binding.delivery_id])
                    .map_err(|error| StoreError::Other(error.to_string()))?;
                tx.execute("INSERT INTO prompt_queue
                    (source,target,prompt,created_at,factory_session,summary,priority,urgent,dedupe_key,origin_kind)
                    VALUES ('violet-recovery',?1,?2,?3,?4,'Managed Slack recovery',?5,0,?6,'daemon')",
                    params![binding.recipient,prompt,Utc::now().to_rfc3339(),binding.factory_session,
                        i32::from(NotificationPriority::High),key])?;
                recovery_prompt_id = Some(tx.last_insert_rowid());
                let _ = capture_message_event(tx, "violet-recovery", &binding.recipient);
            }
        } else if let Some(notice_id) = recovery_prompt_id {
            // A later authoritative consumed receipt retires a pending
            // uncertainty notice without inventing a recipient read.
            tx.execute(
                "UPDATE prompt_queue SET target='violet-admission-history',
                processed_at=COALESCE(processed_at,?2) WHERE id=?1",
                params![notice_id, Utc::now().to_rfc3339()],
            )?;
        }
    }
    tx.execute("UPDATE violet_admissions SET registration_id=?2,delivery_generation=?3,
        factory_session=?4,recipient=?5,consumption=?6,recovery_prompt_id=?7,ack_pending=0 WHERE delivery_id=?1",
        params![binding.delivery_id,binding.registration_id,binding.delivery_generation,
            binding.factory_session,binding.recipient,consumption.as_str(),recovery_prompt_id])?;
    Ok(VioletRecoveryReceipt {
        admission: VioletAdmissionReceipt {
            delivery_id: binding.delivery_id.clone(),
            prompt_id,
            created: false,
        },
        consumption,
        recovery_prompt_id,
    })
}

fn validate_rebind(binding: &VioletAdmissionRebind) -> Result<()> {
    if [
        &binding.delivery_id,
        &binding.occurrence_id,
        &binding.reader_slot_id,
        &binding.project_id,
        &binding.registration_id,
        &binding.factory_session,
        &binding.recipient,
    ]
    .iter()
    .any(|value| value.trim().is_empty())
        || binding.expected_generation < 1
        || binding.delivery_generation < 1
    {
        return Err(StoreError::Other(
            "invalid Violet rebind identity or generation".into(),
        ));
    }
    Ok(())
}

fn slack_timestamp_parts(value: &str) -> Result<(&str, &str)> {
    let Some((seconds, fraction)) = value.split_once('.') else {
        return Err(StoreError::Other("invalid Slack timestamp".into()));
    };
    if seconds.is_empty()
        || fraction.is_empty()
        || !seconds
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(StoreError::Other("invalid Slack timestamp".into()));
    }
    Ok((seconds.trim_start_matches('0'), fraction))
}

fn slack_timestamp_after(next: &str, current: &str) -> Result<bool> {
    let (ns, nf) = slack_timestamp_parts(next)?;
    let (cs, cf) = slack_timestamp_parts(current)?;
    let seconds = ns.len().cmp(&cs.len()).then_with(|| ns.cmp(cs));
    let width = nf.len().max(cf.len());
    Ok(seconds
        .then_with(|| {
            nf.bytes()
                .chain(std::iter::repeat(b'0'))
                .take(width)
                .cmp(cf.bytes().chain(std::iter::repeat(b'0')).take(width))
        })
        .is_gt())
}
