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
            let conn = crate::shared_db::lock_connection(&queue.conn)?;
            for statement in VIOLET_ADMISSION_SCHEMA_STATEMENTS {
                conn.execute(statement, [])?;
            }
            conn.execute_batch(VIOLET_ADMISSION_TRIGGER)?;
            Ok(())
        })?;
        Ok(Self { queue })
    }

    /// Only a committed receipt may authorize an admission acknowledgement.
    pub fn admit(&self, input: &VioletAdmissionInput) -> Result<VioletAdmissionReceipt> {
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
            &input.lease_token,
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
                    || registration != input.registration_id
                    || generation != input.delivery_generation
                    || project != input.project_id
                    || target != input.target_json
                    || session != input.factory_session
                    || recipient != input.recipient
                {
                    return Err(StoreError::Other(
                        "conflicting Violet admission identity or registration; authorized rebind required".into()));
                }
                tx.execute("UPDATE violet_admissions SET lease_token=?2,ack_pending=1 WHERE delivery_id=?1",
                    params![id,input.lease_token])?;
                return Ok(VioletAdmissionReceipt {
                    delivery_id: id,
                    prompt_id,
                    created: false,
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
                 route_generation,project_id,target_json,factory_session,recipient,prompt_id,lease_token)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![input.delivery_id,input.occurrence_id,input.reader_slot_id,input.registration_id,
                    input.delivery_generation,input.route_generation,input.project_id,input.target_json,
                    input.factory_session,input.recipient,prompt_id,input.lease_token])?;
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
            Ok(VioletAdmissionReceipt {
                delivery_id: input.delivery_id.clone(),
                prompt_id,
                created: true,
            })
        })?;
        if receipt.created {
            self.queue.signal_inbox(&input.recipient);
        }
        Ok(receipt)
    }
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
