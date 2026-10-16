//! One occurrence per logical reader, committed with its actual prompt row.
use super::*;

pub const VIOLET_ADMISSION_SCHEMA: &str = r#"
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
);
CREATE TABLE IF NOT EXISTS violet_admission_cursors (
    reader_slot_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    message_ts TEXT NOT NULL,
    PRIMARY KEY(reader_slot_id,workspace_id,channel_id)
);
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VioletAdmissionReceipt {
    pub delivery_id: String,
    pub prompt_id: i64,
    pub created: bool,
}

pub struct SqliteVioletAdmissionStore {
    queue: SqlitePromptQueueStore,
}

impl SqliteVioletAdmissionStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let queue = SqlitePromptQueueStore::open(cas_dir)?;
        queue.init()?;
        crate::shared_db::with_write_retry(|| {
            crate::shared_db::lock_connection(&queue.conn)?
                .execute_batch(VIOLET_ADMISSION_SCHEMA)?;
            Ok(())
        })?;
        Ok(Self { queue })
    }

    /// Only a committed receipt may authorize an admission acknowledgement.
    pub fn admit(&self, _input: &VioletAdmissionInput) -> Result<VioletAdmissionReceipt> {
        Err(StoreError::Other(
            "v2 durable admission is not implemented".into(),
        ))
    }
}
