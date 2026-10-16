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
