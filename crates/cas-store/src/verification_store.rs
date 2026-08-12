//! Verification storage for task quality gates
//!
//! Stores verification results in SQLite. Verifications are created when
//! attempting to close a task, with a Haiku subagent reviewing the work.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::SqliteEventStore;
use crate::TaskStore;
use crate::agent_store::register_agent_with_conn;
use crate::error::StoreError;
use crate::event_store::record_event_with_conn;
use crate::recording_store::capture_task_event;
use crate::shared_db::ImmediateTx;
use crate::supervisor_queue_store::{
    NotificationPriority, SqliteSupervisorQueueStore, SupervisorQueueStore,
};
use crate::task_store::SqliteTaskStore;
use cas_types::{
    Agent, AgentRole, AgentStatus, AgentType, Dependency, DependencyType, Event, EventEntityType,
    EventType, IssueSeverity, RecordingEventType, Task, TaskDeliverables, TaskStatus, Verification,
    VerificationDispatch, VerificationDispatchState, VerificationIssue, VerificationProofBoundary,
    VerificationProvenance, VerificationRecoveryAction, VerificationStatus, VerificationType,
    VerifierCapability, WorkerDeliveryState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReopenLifecycleOutbox {
    pub supervisor_id: String,
    pub payload: String,
    pub priority: NotificationPriority,
    pub transition_key: String,
    pub prompt_delivered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub enum ParentDependencyUpdate {
    Unchanged,
    Replace(Option<Dependency>),
}

// Helper to convert lock errors
fn lock_err<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Parse("Failed to acquire lock".to_string())
}

fn sanitized_verification_for_write(verification: &Verification) -> Verification {
    let mut sanitized = verification.clone();
    if matches!(
        sanitized.provenance,
        VerificationProvenance::TaskVerifier | VerificationProvenance::SupervisorDirect
    ) {
        sanitized.sanitize_verifier_authored_content();
    }
    sanitized
}

fn validate_verification_authority_with_conn(
    conn: &Connection,
    verification: &Verification,
    allow_resolved: bool,
) -> Result<()> {
    let fail =
        || StoreError::Parse("verification provenance lacks exact durable authority".to_string());
    match verification.provenance {
        VerificationProvenance::Legacy => {
            if verification.capability_id.is_some()
                || verification.dispatch_id.is_some()
                || verification.issuer_agent_id.is_some()
            {
                return Err(fail());
            }
            Ok(())
        }
        VerificationProvenance::System => Err(fail()),
        VerificationProvenance::SupervisorDirect => {
            let agent_id = verification.agent_id.as_deref().ok_or_else(fail)?;
            let issuer_id = verification.issuer_agent_id.as_deref().ok_or_else(fail)?;
            let dispatch_id = verification.dispatch_id.as_deref().ok_or_else(fail)?;
            if agent_id != issuer_id || verification.capability_id.is_some() {
                return Err(fail());
            }
            let valid: i64 = conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM agents a
                    JOIN verification_dispatches d ON d.id = ?2
                    WHERE a.id = ?1 AND a.role = 'supervisor'
                      AND a.status IN ('active', 'idle')
                      AND d.task_id = ?3
                      AND d.state IN ('pending', 'claimed', 'timed_out', 'resolved')
                      AND (?4 = 1 OR d.state != 'resolved')
                )",
                params![
                    agent_id,
                    dispatch_id,
                    verification.task_id,
                    allow_resolved as i64
                ],
                |row| row.get(0),
            )?;
            if valid == 1 { Ok(()) } else { Err(fail()) }
        }
        VerificationProvenance::TaskVerifier => {
            let agent_id = verification.agent_id.as_deref().ok_or_else(fail)?;
            let issuer_id = verification.issuer_agent_id.as_deref().ok_or_else(fail)?;
            let dispatch_id = verification.dispatch_id.as_deref().ok_or_else(fail)?;
            let capability_id = verification.capability_id.as_deref().ok_or_else(fail)?;
            let valid: i64 = conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM verification_capabilities c
                    JOIN verification_dispatches d ON d.id = c.dispatch_id
                    JOIN agents issuer ON issuer.id = c.issuer_agent_id
                    WHERE c.id = ?1 AND c.task_id = ?2 AND c.dispatch_id = ?3
                      AND c.issuer_agent_id = ?4 AND c.verifier_agent_id = ?5
                      AND c.consumed_at IS NOT NULL
                      AND d.task_id = ?2 AND d.verifier_agent_id = ?5
                      AND d.capability_id = ?1
                      AND d.state IN ('claimed', 'resolved')
                      AND (?6 = 1 OR d.state != 'resolved')
                      AND issuer.status IN ('active', 'idle')
                )",
                params![
                    capability_id,
                    verification.task_id,
                    dispatch_id,
                    issuer_id,
                    agent_id,
                    allow_resolved as i64
                ],
                |row| row.get(0),
            )?;
            if valid == 1 { Ok(()) } else { Err(fail()) }
        }
    }
}

/// SQLite DDL for the `verifications` and `verification_issues` tables.
///
/// Re-exported via `cas_store::VERIFICATION_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base tables before applying ALTER migrations.
/// See cas-bdb9 / EPIC cas-9fdb.
pub const VERIFICATION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS verifications (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    agent_id TEXT,
    verification_type TEXT NOT NULL DEFAULT 'task',
    provenance TEXT NOT NULL DEFAULT 'legacy',
    capability_id TEXT,
    dispatch_id TEXT,
    issuer_agent_id TEXT,
    status TEXT NOT NULL DEFAULT 'approved',
    confidence REAL,
    summary TEXT NOT NULL DEFAULT '',
    files_reviewed TEXT NOT NULL DEFAULT '[]',
    duration_ms INTEGER,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS verification_issues (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    verification_id TEXT NOT NULL,
    file TEXT NOT NULL,
    line INTEGER,
    severity TEXT NOT NULL DEFAULT 'blocking',
    category TEXT NOT NULL,
    code TEXT NOT NULL DEFAULT '',
    problem TEXT NOT NULL,
    suggestion TEXT,
    FOREIGN KEY (verification_id) REFERENCES verifications(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS verification_capabilities (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    dispatch_id TEXT,
    issuer_agent_id TEXT NOT NULL,
    verifier_agent_id TEXT,
    token_hash TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    bound_at TEXT,
    consumed_at TEXT
);

CREATE TABLE IF NOT EXISTS verification_handoffs (
    capability_id TEXT PRIMARY KEY,
    issuer_agent_id TEXT NOT NULL,
    verifier_agent_id TEXT,
    tool_use_id_hash TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    bound_at TEXT,
    consumed_at TEXT,
    FOREIGN KEY (capability_id) REFERENCES verification_capabilities(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS verification_dispatches (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    receipt_id TEXT,
    delivery_transaction_id TEXT,
    repository_proof TEXT,
    requester_agent_id TEXT NOT NULL,
    owner_agent_id TEXT NOT NULL,
    verifier_agent_id TEXT,
    capability_id TEXT,
    state TEXT NOT NULL DEFAULT 'pending',
    requested_at TEXT NOT NULL,
    deadline_at TEXT NOT NULL,
    resolved_at TEXT,
    recovery_action TEXT NOT NULL DEFAULT 'supervisor_redispatch_or_direct'
);

CREATE INDEX IF NOT EXISTS idx_verifications_task ON verifications(task_id);
CREATE INDEX IF NOT EXISTS idx_verifications_status ON verifications(status);
CREATE INDEX IF NOT EXISTS idx_verifications_created ON verifications(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_verification_issues_verification ON verification_issues(verification_id);
CREATE INDEX IF NOT EXISTS idx_verification_capabilities_task
    ON verification_capabilities(task_id, consumed_at, expires_at);
CREATE INDEX IF NOT EXISTS idx_verification_handoffs_child
    ON verification_handoffs(verifier_agent_id, state);
CREATE UNIQUE INDEX IF NOT EXISTS idx_verification_handoffs_pending_parent
    ON verification_handoffs(issuer_agent_id)
    WHERE state = 'pending';
CREATE INDEX IF NOT EXISTS idx_verification_dispatches_task
    ON verification_dispatches(task_id, requested_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_verification_dispatches_active_task
    ON verification_dispatches(task_id)
    WHERE state IN ('pending', 'claimed');
"#;

/// Additive m230 upgrade for dispatches written before repository proofs were
/// durable. Kept here so the store-open repair and numbered migration execute
/// the exact same DDL.
pub const VERIFICATION_DISPATCH_REPOSITORY_PROOF_STATEMENT: &str =
    "ALTER TABLE verification_dispatches ADD COLUMN repository_proof TEXT";

const VERIFIER_CAPABILITY_TTL_MINUTES: i64 = 30;
const SERVER_HANDOFF_ID_PREFIX: &str = "vhnd-";

/// Newly issued capability. The bearer token exists only in this return value
/// and must be delivered directly to the verifier child; it is never stored.
#[derive(Debug, Clone)]
pub struct IssuedVerifierCapability {
    pub capability: VerifierCapability,
    pub token: String,
}

/// Trait for verification storage operations
pub trait VerificationStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Generate a new unique verification ID (e.g., ver-a1b2)
    fn generate_id(&self) -> Result<String>;

    /// Add a new verification with its issues
    fn add(&self, verification: &Verification) -> Result<()>;

    /// Get a verification by ID (includes issues)
    fn get(&self, id: &str) -> Result<Verification>;

    /// Update an existing verification
    fn update(&self, verification: &Verification) -> Result<()>;

    /// Delete a verification and its issues
    fn delete(&self, id: &str) -> Result<()>;

    /// Get verifications for a task
    fn get_for_task(&self, task_id: &str) -> Result<Vec<Verification>>;

    /// Get the most recent verification for a task
    fn get_latest_for_task(&self, task_id: &str) -> Result<Option<Verification>>;

    /// Get the most recent verification for a task of a specific type
    fn get_latest_for_task_by_type(
        &self,
        task_id: &str,
        verification_type: VerificationType,
    ) -> Result<Option<Verification>>;

    /// List recent verifications
    fn list_recent(&self, limit: usize) -> Result<Vec<Verification>>;

    /// List verifications by status
    fn list_by_status(&self, status: VerificationStatus) -> Result<Vec<Verification>>;

    /// Delete verifications older than the given number of days
    fn prune(&self, older_than_days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based verification store
pub struct SqliteVerificationStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteVerificationStore {
    /// Open or create a SQLite verification store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };

        store.init()?;
        Ok(store)
    }

    fn parse_verification(row: &rusqlite::Row) -> rusqlite::Result<Verification> {
        let verification_type_str: String = row.get(3)?;
        let verification_type =
            VerificationType::from_str(&verification_type_str).unwrap_or_default();

        let provenance_str: String = row.get(4)?;
        let provenance = VerificationProvenance::from_str(&provenance_str).unwrap_or_default();

        let status_str: String = row.get(8)?;
        let status = VerificationStatus::from_str(&status_str).unwrap_or_default();

        let created_at_str: String = row.get(13)?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let files_reviewed_json: String = row.get(11)?;
        let files_reviewed: Vec<String> =
            serde_json::from_str(&files_reviewed_json).unwrap_or_default();

        Ok(Verification {
            id: row.get(0)?,
            task_id: row.get(1)?,
            agent_id: row.get(2)?,
            verification_type,
            provenance,
            capability_id: row.get(5)?,
            dispatch_id: row.get(6)?,
            issuer_agent_id: row.get(7)?,
            status,
            confidence: row.get(9)?,
            summary: row.get(10)?,
            files_reviewed,
            duration_ms: row.get::<_, Option<i64>>(12)?.map(|v| v as u64),
            created_at,
            issues: Vec::new(), // Issues loaded separately
        })
    }

    fn load_issues(
        &self,
        conn: &Connection,
        verification_id: &str,
    ) -> Result<Vec<VerificationIssue>> {
        let mut stmt = conn.prepare_cached(
            "SELECT file, line, severity, category, code, problem, suggestion
             FROM verification_issues WHERE verification_id = ?1
             ORDER BY id",
        )?;

        let issues = stmt
            .query_map(params![verification_id], |row| {
                let severity_str: String = row.get(2)?;
                let severity = IssueSeverity::from_str(&severity_str).unwrap_or_default();

                Ok(VerificationIssue {
                    file: row.get(0)?,
                    line: row.get::<_, Option<i32>>(1)?.map(|v| v as u32),
                    severity,
                    category: row.get(3)?,
                    code: row.get(4)?,
                    problem: row.get(5)?,
                    suggestion: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(issues)
    }

    fn load_issues_batch(
        &self,
        conn: &Connection,
        verification_ids: &[&str],
    ) -> Result<HashMap<String, Vec<VerificationIssue>>> {
        if verification_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders: Vec<String> = (0..verification_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let query = format!(
            "SELECT verification_id, file, line, severity, category, code, problem, suggestion
             FROM verification_issues WHERE verification_id IN ({})
             ORDER BY id",
            placeholders.join(", ")
        );

        let mut stmt = conn.prepare(&query)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(verification_ids.len());
        for id in verification_ids {
            params_vec.push(id);
        }

        let rows = stmt.query_map(params_vec.as_slice(), |row| {
            let vid: String = row.get(0)?;
            let severity_str: String = row.get(3)?;
            let severity = IssueSeverity::from_str(&severity_str).unwrap_or_default();

            Ok((
                vid,
                VerificationIssue {
                    file: row.get(1)?,
                    line: row.get::<_, Option<i32>>(2)?.map(|v| v as u32),
                    severity,
                    category: row.get(4)?,
                    code: row.get(5)?,
                    problem: row.get(6)?,
                    suggestion: row.get(7)?,
                },
            ))
        })?;

        let mut map: HashMap<String, Vec<VerificationIssue>> = HashMap::new();
        for row in rows.filter_map(|r| r.ok()) {
            map.entry(row.0).or_default().push(row.1);
        }
        Ok(map)
    }

    fn attach_issues_batch(
        &self,
        conn: &Connection,
        verifications: &mut [Verification],
    ) -> Result<()> {
        let ids: Vec<&str> = verifications.iter().map(|v| v.id.as_str()).collect();
        let mut map = self.load_issues_batch(conn, &ids)?;
        for v in verifications.iter_mut() {
            v.issues = map.remove(&v.id).unwrap_or_default();
        }
        Ok(())
    }

    fn save_issues(&self, conn: &Connection, verification: &Verification) -> Result<()> {
        // Delete existing issues first
        conn.execute(
            "DELETE FROM verification_issues WHERE verification_id = ?1",
            params![verification.id],
        )?;

        // Insert new issues
        let mut stmt = conn.prepare_cached(
            "INSERT INTO verification_issues
             (verification_id, file, line, severity, category, code, problem, suggestion)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;

        for issue in &verification.issues {
            stmt.execute(params![
                verification.id,
                issue.file,
                issue.line.map(|v| v as i32),
                issue.severity.to_string(),
                issue.category,
                issue.code,
                issue.problem,
                issue.suggestion,
            ])?;
        }

        Ok(())
    }
}

/// Return the verdict bound to one exact durable proof-cycle dispatch.
///
/// Legacy task-wide rows have `dispatch_id IS NULL` and are intentionally
/// invisible here so they cannot authorize a current close cycle.
pub fn get_verification_for_dispatch(
    cas_dir: &Path,
    dispatch_id: &str,
) -> Result<Option<Verification>> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let verification = conn
        .query_row(
            "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                    dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                    duration_ms, created_at
             FROM verifications WHERE dispatch_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT 1",
            params![dispatch_id],
            SqliteVerificationStore::parse_verification,
        )
        .optional()
        .map_err(StoreError::Database)?;
    match verification {
        Some(mut verification) => {
            verification.issues = store.load_issues(&conn, &verification.id)?;
            Ok(Some(verification))
        }
        None => Ok(None),
    }
}

/// Add a verification record using an existing connection (for cross-store transactions).
///
/// Caller is responsible for managing the transaction. Does not save issues -
/// call `save_verification_issues_with_conn` separately.
fn insert_verification_with_conn(conn: &Connection, verification: &Verification) -> Result<()> {
    let files_reviewed_json =
        serde_json::to_string(&verification.files_reviewed).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO verifications
         (id, task_id, agent_id, verification_type, provenance, capability_id, dispatch_id,
          issuer_agent_id, status, confidence, summary, files_reviewed, duration_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            verification.id,
            verification.task_id,
            verification.agent_id,
            verification.verification_type.to_string(),
            verification.provenance.to_string(),
            verification.capability_id,
            verification.dispatch_id,
            verification.issuer_agent_id,
            verification.status.to_string(),
            verification.confidence,
            verification.summary,
            files_reviewed_json,
            verification.duration_ms.map(|v| v as i64),
            verification.created_at.to_rfc3339(),
        ],
    )?;

    // Save issues inline
    save_verification_issues_with_conn(conn, &verification)?;

    Ok(())
}

pub fn add_verification_with_conn(conn: &Connection, verification: &Verification) -> Result<()> {
    let verification = sanitized_verification_for_write(verification);
    validate_verification_authority_with_conn(conn, &verification, false)?;
    insert_verification_with_conn(conn, &verification)
}

/// Persist an internal close-flow verification that cannot be supplied through
/// the generic store contract.
pub fn add_system_verification(cas_dir: &Path, verification: &Verification) -> Result<()> {
    if verification.provenance != VerificationProvenance::System {
        return Err(StoreError::Parse(
            "internal verification write requires system provenance".to_string(),
        ));
    }
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    insert_verification_with_conn(&conn, verification)
}

/// Update mutable result fields on an existing internal close-flow record.
pub fn update_system_verification(cas_dir: &Path, verification: &Verification) -> Result<()> {
    if verification.provenance != VerificationProvenance::System {
        return Err(StoreError::Parse(
            "internal verification update requires system provenance".to_string(),
        ));
    }
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let existing = conn
        .query_row(
            "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                duration_ms, created_at FROM verifications WHERE id = ?1",
            params![verification.id],
            SqliteVerificationStore::parse_verification,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(verification.id.clone()))?;
    if existing.task_id != verification.task_id
        || existing.agent_id != verification.agent_id
        || existing.verification_type != verification.verification_type
        || existing.provenance != verification.provenance
        || existing.capability_id != verification.capability_id
        || existing.dispatch_id != verification.dispatch_id
        || existing.issuer_agent_id != verification.issuer_agent_id
    {
        return Err(StoreError::Parse(
            "verification authority and identity fields are immutable".to_string(),
        ));
    }
    let files =
        serde_json::to_string(&verification.files_reviewed).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "UPDATE verifications SET status = ?2, confidence = ?3, summary = ?4,
         files_reviewed = ?5, duration_ms = ?6 WHERE id = ?1",
        params![
            verification.id,
            verification.status.to_string(),
            verification.confidence,
            verification.summary,
            files,
            verification.duration_ms.map(|v| v as i64)
        ],
    )?;
    save_verification_issues_with_conn(&conn, verification)
}

/// Save verification issues using an existing connection (for cross-store transactions).
pub fn save_verification_issues_with_conn(
    conn: &Connection,
    verification: &Verification,
) -> Result<()> {
    let verification = sanitized_verification_for_write(verification);
    conn.execute(
        "DELETE FROM verification_issues WHERE verification_id = ?1",
        params![verification.id],
    )?;

    let mut stmt = conn.prepare_cached(
        "INSERT INTO verification_issues
         (verification_id, file, line, severity, category, code, problem, suggestion)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    for issue in &verification.issues {
        stmt.execute(params![
            verification.id,
            issue.file,
            issue.line.map(|v| v as i32),
            issue.severity.to_string(),
            issue.category,
            issue.code,
            issue.problem,
            issue.suggestion,
        ])?;
    }

    Ok(())
}

fn capability_token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cas-verifier-capability-v1\0");
    hasher.update(token.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verifier_handoff_tool_hash(tool_use_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cas-verifier-handoff-tool-v1\0");
    hasher.update(tool_use_id.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verifier_handoff_secret_hash(secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cas-verifier-handoff-secret-v1\0");
    hasher.update(secret);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_server_handoff(capability: &VerifierCapability) -> bool {
    capability.id.starts_with(SERVER_HANDOFF_ID_PREFIX)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn capability_id_from_token(token: &str) -> Result<&str> {
    let Some((id, secret)) = token.split_once('.') else {
        return Err(StoreError::Parse(
            "malformed verifier capability".to_string(),
        ));
    };
    if !id.starts_with("vcap-")
        || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        || secret.len() < 32
    {
        return Err(StoreError::Parse(
            "malformed verifier capability".to_string(),
        ));
    }
    Ok(id)
}

/// Validate a bearer against its stored digest without binding or consuming it.
///
/// This exists so the MCP boundary can persist an exact dispatch timeout
/// before beginning the verdict transaction. The raw token is never returned.
pub fn inspect_verifier_capability(cas_dir: &Path, token: &str) -> Result<VerifierCapability> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let id = capability_id_from_token(token)?;
    let capability = load_capability_with_conn(&conn, id)?;
    if !constant_time_eq(&capability.token_hash, &capability_token_hash(token)) {
        return Err(StoreError::Parse(
            "verifier capability is invalid".to_string(),
        ));
    }
    Ok(capability)
}

fn load_capability_with_conn(conn: &Connection, id: &str) -> Result<VerifierCapability> {
    conn.query_row(
        "SELECT id, task_id, dispatch_id, issuer_agent_id, verifier_agent_id, token_hash,
                issued_at, expires_at, bound_at, consumed_at
         FROM verification_capabilities WHERE id = ?1",
        params![id],
        |row| {
            let parse_time = |index| -> rusqlite::Result<DateTime<Utc>> {
                let value: String = row.get(index)?;
                DateTime::parse_from_rfc3339(&value)
                    .map(|time| time.with_timezone(&Utc))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            index,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
            };
            let parse_optional_time = |index| -> rusqlite::Result<Option<DateTime<Utc>>> {
                let value: Option<String> = row.get(index)?;
                value
                    .map(|value| {
                        DateTime::parse_from_rfc3339(&value)
                            .map(|time| time.with_timezone(&Utc))
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    index,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })
                    })
                    .transpose()
            };
            Ok(VerifierCapability {
                id: row.get(0)?,
                task_id: row.get(1)?,
                dispatch_id: row.get(2)?,
                issuer_agent_id: row.get(3)?,
                verifier_agent_id: row.get(4)?,
                token_hash: row.get(5)?,
                issued_at: parse_time(6)?,
                expires_at: parse_time(7)?,
                bound_at: parse_optional_time(8)?,
                consumed_at: parse_optional_time(9)?,
            })
        },
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => {
            StoreError::NotFound("verifier capability".to_string())
        }
        other => StoreError::Database(other),
    })
}

fn parse_dispatch(row: &rusqlite::Row) -> rusqlite::Result<VerificationDispatch> {
    let parse_time = |index| -> rusqlite::Result<DateTime<Utc>> {
        let value: String = row.get(index)?;
        DateTime::parse_from_rfc3339(&value)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
    };
    let parse_optional_time = |index| -> rusqlite::Result<Option<DateTime<Utc>>> {
        let value: Option<String> = row.get(index)?;
        value
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|time| time.with_timezone(&Utc))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            index,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
            })
            .transpose()
    };
    let repository = row
        .get::<_, Option<String>>(4)?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    let state_value: String = row.get(9)?;
    let state = VerificationDispatchState::from_str(&state_value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let recovery_value: String = row.get(13)?;
    let recovery_action =
        VerificationRecoveryAction::from_str(&recovery_value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(VerificationDispatch {
        id: row.get(0)?,
        task_id: row.get(1)?,
        receipt_id: row.get(2)?,
        delivery_transaction_id: row.get(3)?,
        repository,
        requester_agent_id: row.get(5)?,
        owner_agent_id: row.get(6)?,
        verifier_agent_id: row.get(7)?,
        capability_id: row.get(8)?,
        state,
        requested_at: parse_time(10)?,
        deadline_at: parse_time(11)?,
        resolved_at: parse_optional_time(12)?,
        recovery_action,
    })
}

pub fn get_latest_verification_dispatch_with_conn(
    conn: &Connection,
    task_id: &str,
) -> Result<Option<VerificationDispatch>> {
    conn.query_row(
        "SELECT id, task_id, receipt_id, delivery_transaction_id, repository_proof, requester_agent_id,
                owner_agent_id, verifier_agent_id, capability_id, state, requested_at,
                deadline_at, resolved_at, recovery_action
         FROM verification_dispatches
         WHERE task_id = ?1
         ORDER BY requested_at DESC, id DESC
         LIMIT 1",
        params![task_id],
        parse_dispatch,
    )
    .optional()
    .map_err(StoreError::Database)
}

/// Return the latest typed dispatch for one task.
pub fn get_latest_verification_dispatch(
    cas_dir: &Path,
    task_id: &str,
) -> Result<Option<VerificationDispatch>> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    get_latest_verification_dispatch_with_conn(&conn, task_id)
}

pub fn get_verification_dispatch_with_conn(
    conn: &Connection,
    dispatch_id: &str,
) -> Result<VerificationDispatch> {
    conn.query_row(
        "SELECT id, task_id, receipt_id, delivery_transaction_id, repository_proof, requester_agent_id,
                owner_agent_id, verifier_agent_id, capability_id, state, requested_at,
                deadline_at, resolved_at, recovery_action
         FROM verification_dispatches WHERE id = ?1",
        params![dispatch_id],
        parse_dispatch,
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => {
            StoreError::NotFound("verification dispatch".to_string())
        }
        other => StoreError::Database(other),
    })
}

/// Return one exact durable proof-cycle dispatch.
pub fn get_verification_dispatch(
    cas_dir: &Path,
    dispatch_id: &str,
) -> Result<VerificationDispatch> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    get_verification_dispatch_with_conn(&conn, dispatch_id)
}

/// Create one durable task-scoped dispatch, returning an existing active
/// dispatch when a retry races the same pending transition.
pub fn create_verification_dispatch_bound(
    cas_dir: &Path,
    task_id: &str,
    requester_agent_id: &str,
    owner_agent_id: &str,
    boundary: &VerificationProofBoundary,
    deadline_at: DateTime<Utc>,
    supervisor_recovery: bool,
) -> Result<VerificationDispatch> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    create_verification_dispatch_bound_with_conn(
        &conn,
        task_id,
        requester_agent_id,
        owner_agent_id,
        boundary,
        deadline_at,
        supervisor_recovery,
    )
}

/// Atomically bind a supervisor-review proof boundary and project the task
/// into `PendingSupervisorReview`.
///
/// The dispatch is task-only (no fabricated delivery receipt or verifier
/// capability). Identical retries reuse the one active dispatch. A task can
/// never become review-pending without the exact identifier a registered
/// supervisor must resolve.
pub fn pend_task_for_supervisor_review_with_dispatch(
    cas_dir: &Path,
    task: &Task,
    expected_status: TaskStatus,
    requester_agent_id: &str,
    owner_agent_id: &str,
    deadline_at: DateTime<Utc>,
) -> Result<VerificationDispatch> {
    if task.status != TaskStatus::PendingSupervisorReview || !task.pending_verification {
        return Err(StoreError::Parse(
            "supervisor-review transition requires a pending task projection".to_string(),
        ));
    }
    SqliteTaskStore::open(cas_dir)?.init()?;
    SqliteEventStore::open(cas_dir)?;
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    let dispatch = create_verification_dispatch_bound_with_conn(
        &tx,
        &task.id,
        requester_agent_id,
        owner_agent_id,
        &VerificationProofBoundary::task(),
        deadline_at,
        false,
    )?;
    let now = Utc::now();
    let deliverables = serde_json::to_string(&task.deliverables)
        .map_err(|error| StoreError::Parse(format!("invalid task deliverables: {error}")))?;
    let changed = tx.execute(
        "UPDATE tasks
         SET status = ?1, pending_verification = 1, notes = ?2,
             close_reason = ?3, deliverables = ?4, updated_at = ?5
         WHERE id = ?6 AND status = ?7",
        params![
            TaskStatus::PendingSupervisorReview.to_string(),
            task.notes,
            task.close_reason,
            deliverables,
            now.to_rfc3339(),
            task.id,
            expected_status.to_string(),
        ],
    )?;
    if changed != 1 {
        let current: Option<String> = tx
            .query_row(
                "SELECT status FROM tasks WHERE id = ?1",
                params![task.id],
                |row| row.get(0),
            )
            .optional()?;
        if current.as_deref() != Some("pending_supervisor_review") {
            return Err(current
                .map(|status| {
                    StoreError::Parse(format!(
                        "supervisor-review transition expected {expected_status}, found {status}"
                    ))
                })
                .unwrap_or_else(|| StoreError::TaskNotFound(task.id.clone())));
        }
    } else {
        let event = Event::new(
            EventType::TaskBlocked,
            EventEntityType::Task,
            &task.id,
            format!("Task pending supervisor review: {}", task.title),
        );
        record_event_with_conn(&tx, &event)?;
        // Recording tables are optional in lightweight/test stores; lifecycle
        // recording is observational and must not invalidate the atomic task
        // + dispatch transition.
        let _ = capture_task_event(&tx, RecordingEventType::TaskBlocked, &task.id, None);
    }
    tx.commit()?;
    Ok(dispatch)
}

/// Create an exact dispatch on a caller-owned SQLite transaction.
///
/// This is used to make delivery receipt, transaction, and proof-boundary
/// intent one atomic persistence step. An autocommit connection is promoted
/// to an IMMEDIATE transaction here; callers with an existing transaction
/// must acquire its write lock before invoking this function.
pub fn create_verification_dispatch_bound_with_conn(
    conn: &Connection,
    task_id: &str,
    requester_agent_id: &str,
    owner_agent_id: &str,
    boundary: &VerificationProofBoundary,
    deadline_at: DateTime<Utc>,
    supervisor_recovery: bool,
) -> Result<VerificationDispatch> {
    if conn.is_autocommit() {
        let tx = ImmediateTx::new(conn)?;
        let dispatch = create_verification_dispatch_bound_in_transaction(
            &tx,
            task_id,
            requester_agent_id,
            owner_agent_id,
            boundary,
            deadline_at,
            supervisor_recovery,
        )?;
        tx.commit()?;
        return Ok(dispatch);
    }

    create_verification_dispatch_bound_in_transaction(
        conn,
        task_id,
        requester_agent_id,
        owner_agent_id,
        boundary,
        deadline_at,
        supervisor_recovery,
    )
}

fn create_verification_dispatch_bound_in_transaction(
    conn: &Connection,
    task_id: &str,
    requester_agent_id: &str,
    owner_agent_id: &str,
    boundary: &VerificationProofBoundary,
    deadline_at: DateTime<Utc>,
    supervisor_recovery: bool,
) -> Result<VerificationDispatch> {
    if let Some(existing) = get_latest_verification_dispatch_with_conn(&conn, task_id)?
        && matches!(
            existing.state,
            VerificationDispatchState::Pending | VerificationDispatchState::Claimed
        )
    {
        if existing.requester_agent_id == requester_agent_id
            && existing.owner_agent_id == owner_agent_id
            && existing.receipt_id == boundary.receipt_id
            && existing.delivery_transaction_id == boundary.delivery_transaction_id
            && existing.repository == boundary.repository
        {
            return Ok(existing);
        }
        return Err(StoreError::Parse(
            "a different verification proof boundary is already active for this task".to_string(),
        ));
    }
    if let Some(existing) = get_latest_verification_dispatch_with_conn(&conn, task_id)?
        && existing.state == VerificationDispatchState::TimedOut
        && !supervisor_recovery
    {
        return Err(StoreError::Parse(format!(
            "verification dispatch {} timed out; registered-supervisor recovery must name it",
            existing.id
        )));
    }

    let requested_at = Utc::now();
    let dispatch = VerificationDispatch {
        id: format!("vdispatch-{:032x}", rand::random::<u128>()),
        task_id: task_id.to_string(),
        receipt_id: boundary.receipt_id.clone(),
        delivery_transaction_id: boundary.delivery_transaction_id.clone(),
        repository: boundary.repository.clone(),
        requester_agent_id: requester_agent_id.to_string(),
        owner_agent_id: owner_agent_id.to_string(),
        verifier_agent_id: None,
        capability_id: None,
        state: VerificationDispatchState::Pending,
        requested_at,
        deadline_at,
        resolved_at: None,
        recovery_action: VerificationRecoveryAction::SupervisorRedispatchOrDirect,
    };
    conn.execute(
        "INSERT INTO verification_dispatches
         (id, task_id, receipt_id, delivery_transaction_id, repository_proof, requester_agent_id,
          owner_agent_id, verifier_agent_id, capability_id, state, requested_at,
          deadline_at, resolved_at, recovery_action)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8, ?9, ?10, NULL, ?11)",
        params![
            dispatch.id,
            dispatch.task_id,
            dispatch.receipt_id,
            dispatch.delivery_transaction_id,
            dispatch
                .repository
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| StoreError::Parse(format!("invalid repository proof: {error}")))?,
            dispatch.requester_agent_id,
            dispatch.owner_agent_id,
            dispatch.state.to_string(),
            dispatch.requested_at.to_rfc3339(),
            dispatch.deadline_at.to_rfc3339(),
            dispatch.recovery_action.to_string(),
        ],
    )?;
    Ok(dispatch)
}

/// Backward-compatible task-only dispatch creation.
///
/// This cannot recover a timed-out cycle; registered-supervisor recovery must
/// use the explicit bound API and server-derived authority.
pub fn create_verification_dispatch(
    cas_dir: &Path,
    task_id: &str,
    requester_agent_id: &str,
    owner_agent_id: &str,
    deadline_at: DateTime<Utc>,
) -> Result<VerificationDispatch> {
    create_verification_dispatch_bound(
        cas_dir,
        task_id,
        requester_agent_id,
        owner_agent_id,
        &VerificationProofBoundary::task(),
        deadline_at,
        false,
    )
}

/// Bind an active dispatch to the exact verifier child and capability.
pub fn claim_verification_dispatch_bound(
    cas_dir: &Path,
    dispatch_id: &str,
    owner_agent_id: &str,
    verifier_agent_id: &str,
    capability_id: &str,
) -> Result<VerificationDispatch> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let now = Utc::now();
    let changed = conn.execute(
        "UPDATE verification_dispatches
         SET verifier_agent_id = ?3, capability_id = ?4, state = 'claimed'
         WHERE id = ?1 AND owner_agent_id = ?2 AND state = 'pending'
               AND deadline_at > ?5",
        params![
            dispatch_id,
            owner_agent_id,
            verifier_agent_id,
            capability_id,
            now.to_rfc3339()
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verification dispatch cannot be claimed by this owner/verifier".to_string(),
        ));
    }
    get_verification_dispatch_with_conn(&conn, dispatch_id)
}

/// Backward-compatible task-only claim that still resolves to one exact
/// current dispatch before performing the compare-and-update.
pub fn claim_verification_dispatch(
    cas_dir: &Path,
    task_id: &str,
    owner_agent_id: &str,
    verifier_agent_id: &str,
    capability_id: &str,
) -> Result<VerificationDispatch> {
    let dispatch = get_latest_verification_dispatch(cas_dir, task_id)?
        .ok_or_else(|| StoreError::NotFound("verification dispatch".to_string()))?;
    claim_verification_dispatch_bound(
        cas_dir,
        &dispatch.id,
        owner_agent_id,
        verifier_agent_id,
        capability_id,
    )
}

/// Resolve an active dispatch inside the verdict transaction.
///
/// Capability-bound verifiers must match both child and capability. A
/// capability-free verdict is accepted only when the server has already
/// derived registered-supervisor authority for the caller.
pub fn resolve_verification_dispatch_with_conn(
    conn: &Connection,
    dispatch_id: &str,
    verifier_agent_id: &str,
    capability_id: Option<&str>,
    supervisor_direct: bool,
) -> Result<Option<VerificationDispatch>> {
    let dispatch = get_verification_dispatch_with_conn(conn, dispatch_id)?;
    let now = Utc::now();
    let authorized = match capability_id {
        Some(capability_id) => {
            dispatch.state == VerificationDispatchState::Claimed
                && dispatch.deadline_at > now
                && dispatch.verifier_agent_id.as_deref() == Some(verifier_agent_id)
                && dispatch.capability_id.as_deref() == Some(capability_id)
        }
        None => {
            supervisor_direct
                && matches!(
                    dispatch.state,
                    VerificationDispatchState::Pending
                        | VerificationDispatchState::Claimed
                        | VerificationDispatchState::TimedOut
                )
        }
    };
    if !authorized {
        return Err(StoreError::Parse(
            "verification dispatch is owned by another verifier".to_string(),
        ));
    }
    let resolved_at = now;
    let changed = conn.execute(
        "UPDATE verification_dispatches
         SET state = 'resolved', resolved_at = ?2
         WHERE id = ?1 AND state IN ('pending', 'claimed', 'timed_out')",
        params![dispatch.id, resolved_at.to_rfc3339()],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verification dispatch resolution raced".to_string(),
        ));
    }
    get_verification_dispatch_with_conn(conn, dispatch_id).map(Some)
}

/// Mark one exact due dispatch timed out. Other tasks are never touched.
pub fn timeout_verification_dispatch(
    cas_dir: &Path,
    task_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<VerificationDispatch>> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    let dispatch_id: Option<String> = tx
        .query_row(
            "SELECT id FROM verification_dispatches
             WHERE task_id = ?1 AND state IN ('pending', 'claimed')
                   AND deadline_at <= ?2
             ORDER BY requested_at DESC, id DESC
             LIMIT 1",
            params![task_id, now.to_rfc3339()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(dispatch_id) = dispatch_id else {
        tx.commit()?;
        return Ok(None);
    };
    let changed = tx.execute(
        "UPDATE verification_dispatches
         SET state = 'timed_out', resolved_at = ?2
         WHERE id = ?1 AND task_id = ?3 AND state IN ('pending', 'claimed')
               AND deadline_at <= ?2",
        params![dispatch_id, now.to_rfc3339(), task_id],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verification dispatch timeout selection raced".to_string(),
        ));
    }
    let timed_out = get_verification_dispatch_with_conn(&tx, &dispatch_id)?;
    tx.commit()?;
    Ok(Some(timed_out))
}

/// Invalidate a reusable resolved proof cycle when task rework begins.
pub fn invalidate_verification_dispatch_for_new_cycle(
    cas_dir: &Path,
    task_id: &str,
) -> Result<Option<VerificationDispatch>> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    let Some(dispatch) = get_latest_verification_dispatch_with_conn(&tx, task_id)? else {
        tx.commit()?;
        return Ok(None);
    };
    let dispatch = invalidate_verification_dispatch_for_new_cycle_with_conn(&tx, &dispatch.id)?;
    tx.commit()?;
    Ok(Some(dispatch))
}

/// Invalidate one exact legacy repository proof after its reviewed worktree drifts.
///
/// This transition is deliberately task-scoped: it consumes only authority
/// attached to the named dispatch and clears only that task's pending flag.
pub fn invalidate_verification_dispatch_for_repository_drift(
    cas_dir: &Path,
    dispatch_id: &str,
) -> Result<VerificationDispatch> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    let dispatch = get_verification_dispatch_with_conn(&tx, dispatch_id)?;
    let latest = get_latest_verification_dispatch_with_conn(&tx, &dispatch.task_id)?
        .ok_or_else(|| StoreError::NotFound("latest verification dispatch".to_string()))?;
    if latest.id != dispatch.id
        || dispatch.receipt_id.is_some()
        || dispatch.delivery_transaction_id.is_some()
        || dispatch.repository.is_none()
    {
        return Err(StoreError::Parse(
            "repository proof invalidation requires the latest task-only repository dispatch"
                .to_string(),
        ));
    }
    if dispatch.state == VerificationDispatchState::Invalidated {
        tx.commit()?;
        return Ok(dispatch);
    }
    if !matches!(
        dispatch.state,
        VerificationDispatchState::Pending
            | VerificationDispatchState::Claimed
            | VerificationDispatchState::Resolved
            | VerificationDispatchState::TimedOut
    ) {
        return Err(StoreError::Parse(
            "repository proof dispatch cannot be invalidated from its current state".to_string(),
        ));
    }

    let now = Utc::now().to_rfc3339();
    if let Some(capability_id) = dispatch.capability_id.as_deref() {
        tx.execute(
            "UPDATE verification_capabilities SET consumed_at = COALESCE(consumed_at, ?2)
             WHERE id = ?1",
            params![capability_id, now],
        )?;
        tx.execute(
            "UPDATE verification_handoffs
             SET state = 'consumed', consumed_at = COALESCE(consumed_at, ?2)
             WHERE capability_id = ?1 AND state IN ('pending', 'bound')",
            params![capability_id, now],
        )?;
    }
    let changed = tx.execute(
        "UPDATE verification_dispatches
         SET state = 'invalidated', resolved_at = ?2
         WHERE id = ?1 AND state IN ('pending', 'claimed', 'resolved', 'timed_out')",
        params![dispatch.id, now],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "repository proof invalidation raced".to_string(),
        ));
    }
    tx.execute(
        "UPDATE tasks SET pending_verification = 0, updated_at = ?2 WHERE id = ?1",
        params![dispatch.task_id, now],
    )?;
    let invalidated = get_verification_dispatch_with_conn(&tx, dispatch_id)?;
    tx.commit()?;
    Ok(invalidated)
}

/// Reopen a closed task and invalidate its reusable proof as one durable mutation.
///
/// The full task row, optional parent dependency replacement, task event/recording,
/// and durable supervisor lifecycle occurrence share the same immediate transaction.
/// Any injected or real failure therefore leaves the prior closed proof cycle intact.
#[allow(clippy::too_many_arguments)]
pub fn reopen_closed_task_atomic(
    cas_dir: &Path,
    task: &Task,
    expected_updated_at: DateTime<Utc>,
    parent_dependency: ParentDependencyUpdate,
    lifecycle_outbox: Option<&TaskReopenLifecycleOutbox>,
) -> Result<Option<VerificationDispatch>> {
    reopen_terminal_task_atomic(
        cas_dir,
        task,
        TaskStatus::Closed,
        expected_updated_at,
        parent_dependency,
        lifecycle_outbox,
    )
}

/// Reopen a closed or cancelled task as one optimistic durable mutation.
///
/// `expected_status` keeps the compare-and-swap explicit: callers cannot use
/// this as a generic status rewrite, and a concurrent terminal-state mutation
/// leaves the original row and lifecycle outbox untouched.
pub fn reopen_terminal_task_atomic(
    cas_dir: &Path,
    task: &Task,
    expected_status: TaskStatus,
    expected_updated_at: DateTime<Utc>,
    parent_dependency: ParentDependencyUpdate,
    lifecycle_outbox: Option<&TaskReopenLifecycleOutbox>,
) -> Result<Option<VerificationDispatch>> {
    if !matches!(expected_status, TaskStatus::Closed | TaskStatus::Cancelled) {
        return Err(StoreError::Parse(
            "atomic terminal-task reopen requires closed or cancelled expected status".to_string(),
        ));
    }
    if task.status != TaskStatus::Open || task.closed_at.is_some() {
        return Err(StoreError::Parse(
            "atomic terminal-task reopen requires the complete proposed Open task".to_string(),
        ));
    }

    SqliteEventStore::open(cas_dir)?;
    if lifecycle_outbox.is_some() {
        let supervisor_queue = SqliteSupervisorQueueStore::open(cas_dir)?;
        supervisor_queue.init()?;
    }
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;

    let dispatch = match get_latest_verification_dispatch_with_conn(&tx, &task.id)? {
        Some(dispatch)
            if matches!(
                dispatch.state,
                VerificationDispatchState::Resolved | VerificationDispatchState::TimedOut
            ) =>
        {
            Some(invalidate_verification_dispatch_for_new_cycle_with_conn(
                &tx,
                &dispatch.id,
            )?)
        }
        _ => None,
    };

    SqliteTaskStore::reopen_exact_with_conn(&tx, task, expected_status, expected_updated_at)?;

    if let ParentDependencyUpdate::Replace(replacement) = parent_dependency {
        if let Some(dep) = replacement.as_ref() {
            if dep.from_id != task.id || dep.dep_type != DependencyType::ParentChild {
                return Err(StoreError::Parse(
                    "atomic reopen parent replacement must be a ParentChild dependency".to_string(),
                ));
            }
            let parent_type: Option<String> = tx
                .query_row(
                    "SELECT task_type FROM tasks WHERE id = ?1",
                    params![dep.to_id],
                    |row| row.get(0),
                )
                .optional()?;
            match parent_type.as_deref() {
                Some("epic") => {}
                Some(other) => {
                    return Err(StoreError::Parse(format!(
                        "Task {} is not an epic (type: {other})",
                        dep.to_id
                    )));
                }
                None => return Err(StoreError::TaskNotFound(dep.to_id.clone())),
            }
        }
        tx.execute(
            "DELETE FROM dependencies WHERE from_id = ?1 AND dep_type = ?2",
            params![task.id, DependencyType::ParentChild.to_string()],
        )?;
        if let Some(dep) = replacement {
            tx.execute(
                "INSERT INTO dependencies (from_id, to_id, dep_type, created_at, created_by)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    dep.from_id,
                    dep.to_id,
                    dep.dep_type.to_string(),
                    dep.created_at.to_rfc3339(),
                    dep.created_by,
                ],
            )?;
        }
    }

    let event = Event::new(
        EventType::TaskCreated,
        EventEntityType::Task,
        &task.id,
        format!("Task reopened: {}", task.title),
    );
    record_event_with_conn(&tx, &event)?;
    let recording_schema_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'recordings')",
        [],
        |row| row.get(0),
    )?;
    if recording_schema_exists {
        capture_task_event(&tx, RecordingEventType::TaskCreated, &task.id, None)?;
    }

    if let Some(outbox) = lifecycle_outbox {
        tx.execute(
            "INSERT INTO supervisor_queue
             (supervisor_id, event_type, payload, priority, created_at, transition_key,
              prompt_delivered_at)
             VALUES (?1, 'task_lifecycle', ?2, ?3, ?4, ?5, ?6)",
            params![
                outbox.supervisor_id,
                outbox.payload,
                i32::from(outbox.priority),
                task.updated_at.to_rfc3339(),
                outbox.transition_key,
                outbox.prompt_delivered_at.map(|at| at.to_rfc3339()),
            ],
        )?;
    }

    tx.commit()?;
    Ok(dispatch)
}

/// Atomically invalidate one exact reviewed scope and reopen its task.
///
/// The task compare-and-set is intentionally in the same `BEGIN IMMEDIATE`
/// transaction as the latest-dispatch check. A failed or superseded reopen
/// therefore cannot leave a still-nonterminal task with its close authority
/// silently removed.
pub fn invalidate_verification_dispatch_and_reopen_task_exact(
    cas_dir: &Path,
    dispatch_id: &str,
    task: &Task,
    expected_status: TaskStatus,
) -> Result<VerificationDispatch> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    let selected = get_verification_dispatch_with_conn(&tx, dispatch_id)?;
    if selected.task_id != task.id
        || selected.receipt_id.is_some()
        || selected.delivery_transaction_id.is_some()
        || selected.state != VerificationDispatchState::Resolved
    {
        return Err(StoreError::Parse(
            "fresh-scope recovery requires the exact Resolved task-only dispatch".to_string(),
        ));
    }
    let verdict: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT status, provenance, dispatch_id
             FROM verifications WHERE dispatch_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT 1",
            params![dispatch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (status, provenance, verdict_dispatch_id) = verdict.ok_or_else(|| {
        StoreError::Parse("fresh-scope recovery requires an exact durable verdict".to_string())
    })?;
    let status = VerificationStatus::from_str(&status)
        .map_err(|error| StoreError::Parse(format!("invalid verification status: {error}")))?;
    let provenance = VerificationProvenance::from_str(&provenance)
        .map_err(|error| StoreError::Parse(format!("invalid verification provenance: {error}")))?;
    if !matches!(
        status,
        VerificationStatus::Approved | VerificationStatus::Skipped
    ) || provenance == VerificationProvenance::Legacy
        || verdict_dispatch_id.as_deref() != Some(dispatch_id)
    {
        return Err(StoreError::Parse(
            "fresh-scope recovery requires an exact nonlegacy Approved/Skipped verdict".to_string(),
        ));
    }
    let dispatch = invalidate_verification_dispatch_for_new_cycle_with_conn(&tx, dispatch_id)?;
    if dispatch.task_id != task.id || dispatch.state != VerificationDispatchState::Invalidated {
        return Err(StoreError::Parse(
            "exact reviewed scope was not invalidated".to_string(),
        ));
    }
    let rows = tx.execute(
        "UPDATE tasks SET status = ?1, notes = ?2, updated_at = ?3
         WHERE id = ?4 AND status = ?5",
        params![
            TaskStatus::Open.to_string(),
            task.notes,
            task.updated_at.to_rfc3339(),
            task.id,
            expected_status.to_string(),
        ],
    )?;
    if rows != 1 {
        return Err(StoreError::Parse(
            "exact proof-cycle reopen raced with a task status change".to_string(),
        ));
    }

    let event = Event::new(
        EventType::TaskCreated,
        EventEntityType::Task,
        &task.id,
        format!("Task reopened: {}", task.title),
    );
    let _ = record_event_with_conn(&tx, &event);
    let _ = capture_task_event(&tx, RecordingEventType::TaskCreated, &task.id, None);
    tx.commit()?;
    Ok(dispatch)
}

/// Which proof boundary the declined delivery presented, if any.
///
/// GH #55 / #82: a parked AwaitingMerge task must have a working exit for a
/// negative review verdict regardless of how much exact proof exists, so the
/// boundary shape is reported rather than used as an admission gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestChangesBoundary {
    /// Latest Resolved dispatch bound to an unmerged awaiting_merge delivery.
    Exact,
    /// A delivery transaction exists, but its dispatch is missing, unbound, or
    /// not yet Resolved (GH #82 steps 4-5: the update lock claimed an active
    /// boundary while request_changes claimed none existed).
    UnboundDelivery,
    /// The declined delivery already merged into the target branch
    /// (GH #55: amendment-required after merge).
    Merged,
    /// The task parked awaiting_merge with no exact delivery record at all.
    Legacy,
}

/// What `request_changes_for_parked_delivery` actually declined.
#[derive(Debug, Clone)]
pub struct RequestChangesOutcome {
    pub boundary: RequestChangesBoundary,
    pub delivery_transaction_id: Option<String>,
    pub dispatch_id: Option<String>,
    /// Human-readable branch handling recorded on the decision note.
    pub branch_handling: String,
}

/// Record a supervisor's negative review of a parked AwaitingMerge delivery.
///
/// This is deliberately separate from generic task updates: the delivery proof
/// lock must not prevent recording a negative verdict — recording a failed
/// review is exactly when the proof boundary should yield. The worker keeps
/// ownership and its branch history; CAS invalidates the rejected anchor and
/// proof so a new close cycle requires corrective commits rather than reusing
/// the declined proof.
///
/// It works for every parked shape: an exact Resolved delivery proof, a
/// delivery whose dispatch never resolved or never bound (GH #82), a delivery
/// that already merged before review failed (GH #55 amendment case), and a
/// legacy park with no delivery record at all. Only the task status matters as
/// an admission gate.
pub fn request_changes_for_parked_delivery(
    cas_dir: &Path,
    task_id: &str,
    supervisor_agent_id: &str,
    reason: &str,
) -> Result<RequestChangesOutcome> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(StoreError::Parse(
            "request_changes requires a non-empty supervisor reason".to_string(),
        ));
    }
    if supervisor_agent_id.trim().is_empty() {
        return Err(StoreError::Parse(
            "request_changes requires an authenticated supervisor".to_string(),
        ));
    }

    SqliteTaskStore::open(cas_dir)?.init()?;
    SqliteEventStore::open(cas_dir)?;
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    conn.execute_batch(crate::delivery_store::DELIVERY_SCHEMA)?;
    let tx = ImmediateTx::new(&conn)?;

    let dispatch = get_latest_verification_dispatch_with_conn(&tx, task_id)?
        .filter(|dispatch| dispatch.task_id == task_id);

    // Latest delivery transaction for this task, whatever state it is in.
    let delivery: Option<(String, String, WorkerDeliveryState)> = tx
        .query_row(
            "SELECT t.id, t.receipt_id, t.state
             FROM worker_delivery_transactions t
             JOIN worker_completion_receipts r ON t.receipt_id = r.id
             WHERE t.task_id = ?1 ORDER BY r.created_at DESC LIMIT 1",
            params![task_id],
            |row| {
                let id: String = row.get(0)?;
                let receipt_id: String = row.get(1)?;
                let state: String = row.get(2)?;
                Ok((id, receipt_id, state))
            },
        )
        .optional()?
        .map(|(id, receipt_id, state)| {
            WorkerDeliveryState::from_str(&state)
                .map(|state| (id, receipt_id, state))
                .map_err(|error| {
                    StoreError::Parse(format!("invalid worker delivery state: {error}"))
                })
        })
        .transpose()?;

    let boundary = match (&delivery, &dispatch) {
        (None, _) => RequestChangesBoundary::Legacy,
        (Some((_, _, WorkerDeliveryState::Merged)), _) => RequestChangesBoundary::Merged,
        (
            Some((transaction_id, receipt_id, WorkerDeliveryState::AwaitingMerge)),
            Some(dispatch),
        ) if dispatch.state == VerificationDispatchState::Resolved
            && dispatch.delivery_transaction_id.as_deref() == Some(transaction_id.as_str())
            && dispatch.receipt_id.as_deref() == Some(receipt_id.as_str()) =>
        {
            RequestChangesBoundary::Exact
        }
        (Some(_), _) => RequestChangesBoundary::UnboundDelivery,
    };

    let (status, notes, deliverables_json): (String, String, String) = tx
        .query_row(
            "SELECT status, notes, deliverables FROM tasks WHERE id = ?1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => StoreError::TaskNotFound(task_id.to_string()),
            other => StoreError::Database(other),
        })?;
    if status != TaskStatus::AwaitingMerge.to_string() {
        return Err(StoreError::Parse(format!(
            "request_changes requires task status awaiting_merge, found {status}"
        )));
    }
    let mut deliverables: TaskDeliverables = serde_json::from_str(&deliverables_json)
        .map_err(|error| StoreError::Parse(format!("invalid task deliverables: {error}")))?;
    let rejected_branch = deliverables
        .parked_branch
        .as_deref()
        .unwrap_or("the factory branch")
        .to_string();
    deliverables.factory_branch_anchor = None;
    deliverables.parked_branch = None;
    deliverables.merge_conflicted = false;
    deliverables.review_envelope = None;

    let now = Utc::now();
    let branch_handling = if boundary == RequestChangesBoundary::Merged {
        "prior commits are already merged into the target branch; the assigned worker must land corrective commits (including an explicit revert commit when requested) as new work.".to_string()
    } else {
        format!(
            "prior commits remain on {rejected_branch}; the assigned worker must add corrective commits (including an explicit revert commit when requested) before re-delivery."
        )
    };
    let note = format!(
        "[{}] Decision: changes requested by supervisor. {reason}\n\nBranch handling: {branch_handling}",
        now.format("%Y-%m-%d %H:%M")
    );
    let notes = if notes.is_empty() {
        note.clone()
    } else {
        format!("{notes}\n\n{note}")
    };

    // Invalidate whatever proof authorized (or was about to authorize) the
    // declined close, so re-close needs a fresh cycle. A Pending/Claimed
    // dispatch is invalidated directly: the task is going back to the worker,
    // so an in-flight verification for the rejected scope is stale either way.
    if let Some(active) = dispatch.as_ref() {
        let invalidated = match active.state {
            VerificationDispatchState::Resolved | VerificationDispatchState::TimedOut => {
                invalidate_verification_dispatch_for_new_cycle_with_conn(&tx, &active.id)?.state
                    == VerificationDispatchState::Invalidated
            }
            VerificationDispatchState::Pending | VerificationDispatchState::Claimed => {
                tx.execute(
                    "UPDATE verification_dispatches
                     SET state = 'invalidated', resolved_at = ?2
                     WHERE id = ?1 AND state IN ('pending', 'claimed')",
                    params![active.id, now.to_rfc3339()],
                )? == 1
            }
            VerificationDispatchState::Invalidated => true,
        };
        if !invalidated {
            return Err(StoreError::Parse(
                "request_changes could not invalidate the declined delivery proof".to_string(),
            ));
        }
    }

    // A delivery still in flight is declined; an already-merged one keeps its
    // merged state because the merge is a fact the audit trail must retain.
    if let Some((transaction_id, _, state)) = delivery.as_ref()
        && matches!(
            state,
            WorkerDeliveryState::AwaitingVerification
                | WorkerDeliveryState::AwaitingMerge
                | WorkerDeliveryState::MergeAuthorized
                | WorkerDeliveryState::CloseReady
        )
    {
        let delivery_changed = tx.execute(
            "UPDATE worker_delivery_transactions
             SET state = ?2, supervisor_agent_id = ?3,
                 last_error_code = 'changes_requested', last_error_detail = ?4, updated_at = ?5
             WHERE id = ?1 AND task_id = ?6 AND state = ?7",
            params![
                transaction_id,
                WorkerDeliveryState::ChangesRequested.to_string(),
                supervisor_agent_id,
                reason,
                now.to_rfc3339(),
                task_id,
                state.to_string(),
            ],
        )?;
        if delivery_changed != 1 {
            return Err(StoreError::Parse(
                "request_changes delivery transition raced".to_string(),
            ));
        }
        tx.execute(
            "INSERT INTO worker_delivery_events
             (id, transaction_id, state, actor_agent_id, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format!("wde-{:032x}", rand::random::<u128>()),
                transaction_id,
                WorkerDeliveryState::ChangesRequested.to_string(),
                supervisor_agent_id,
                reason,
                now.to_rfc3339(),
            ],
        )?;
    }
    let task_changed = tx.execute(
        "UPDATE tasks
         SET status = 'open', notes = ?2, deliverables = ?3,
             pending_verification = 0, pending_worktree_merge = 0, updated_at = ?4
         WHERE id = ?1 AND status = 'awaiting_merge'",
        params![
            task_id,
            notes,
            serde_json::to_string(&deliverables).map_err(|error| {
                StoreError::Parse(format!("failed to serialize task deliverables: {error}"))
            })?,
            now.to_rfc3339(),
        ],
    )?;
    if task_changed != 1 {
        return Err(StoreError::Parse(
            "request_changes task transition raced".to_string(),
        ));
    }
    let event = Event::new(
        EventType::TaskNoteAdded,
        EventEntityType::Task,
        task_id,
        note,
    );
    record_event_with_conn(&tx, &event)?;
    let _ = capture_task_event(&tx, RecordingEventType::Custom, task_id, None);
    tx.commit()?;
    Ok(RequestChangesOutcome {
        boundary,
        delivery_transaction_id: delivery.map(|(id, _, _)| id),
        dispatch_id: dispatch.map(|dispatch| dispatch.id),
        branch_handling,
    })
}

fn invalidate_verification_dispatch_for_new_cycle_with_conn(
    conn: &Connection,
    dispatch_id: &str,
) -> Result<VerificationDispatch> {
    let dispatch = get_verification_dispatch_with_conn(conn, dispatch_id)?;
    let latest = get_latest_verification_dispatch_with_conn(conn, &dispatch.task_id)?
        .ok_or_else(|| StoreError::NotFound("latest verification dispatch".to_string()))?;
    if latest.id != dispatch.id {
        return Err(StoreError::Parse(
            "verification proof-cycle invalidation no longer names the latest task dispatch"
                .to_string(),
        ));
    }
    if !matches!(
        dispatch.state,
        VerificationDispatchState::Resolved | VerificationDispatchState::TimedOut
    ) {
        return Ok(dispatch);
    }
    let now = Utc::now();
    let changed = conn.execute(
        "UPDATE verification_dispatches
         SET state = 'invalidated', resolved_at = ?2
         WHERE id = ?1 AND state IN ('resolved', 'timed_out')",
        params![dispatch.id, now.to_rfc3339()],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verification proof-cycle invalidation raced".to_string(),
        ));
    }
    get_verification_dispatch_with_conn(conn, dispatch_id)
}

/// Mint an unbound legacy explicit-bearer verifier capability.
///
/// Retained only for compatibility with already-integrated explicit-token
/// clients and regression coverage. Production hook paths must use
/// [`issue_server_verifier_handoff`] and never release raw bearer material.
pub fn issue_verifier_capability(
    cas_dir: &Path,
    task_id: &str,
    issuer_agent_id: &str,
) -> Result<IssuedVerifierCapability> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let dispatch = get_latest_verification_dispatch_with_conn(&conn, task_id)?
        .filter(|dispatch| {
            dispatch.state == VerificationDispatchState::Pending
                && dispatch.owner_agent_id == issuer_agent_id
                && dispatch.deadline_at > Utc::now()
        })
        .ok_or_else(|| {
            StoreError::Parse(
                "verifier capability requires an active owned unexpired dispatch".to_string(),
            )
        })?;
    let random_id: u128 = rand::random();
    let random_secret: [u8; 32] = rand::random();
    let id = format!("vcap-{random_id:032x}");
    let secret: String = random_secret
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let token = format!("{id}.{secret}");
    let issued_at = Utc::now();
    let capability = VerifierCapability {
        id: id.clone(),
        task_id: task_id.to_string(),
        dispatch_id: Some(dispatch.id),
        issuer_agent_id: issuer_agent_id.to_string(),
        verifier_agent_id: None,
        token_hash: capability_token_hash(&token),
        issued_at,
        expires_at: issued_at + Duration::minutes(VERIFIER_CAPABILITY_TTL_MINUTES),
        bound_at: None,
        consumed_at: None,
    };
    conn.execute(
        "INSERT INTO verification_capabilities
         (id, task_id, dispatch_id, issuer_agent_id, verifier_agent_id, token_hash,
          issued_at, expires_at, bound_at, consumed_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, NULL, NULL)",
        params![
            capability.id,
            capability.task_id,
            capability.dispatch_id,
            capability.issuer_agent_id,
            capability.token_hash,
            capability.issued_at.to_rfc3339(),
            capability.expires_at.to_rfc3339(),
        ],
    )?;
    Ok(IssuedVerifierCapability { capability, token })
}

/// Create a one-time verifier handoff that never releases bearer material.
///
/// The random secret exists only long enough to derive a domain-separated
/// digest. SubagentStart later binds the unique server-side row by registered
/// parent and official child identity; no prompt or request value carries
/// authority.
pub fn issue_server_verifier_handoff(
    cas_dir: &Path,
    task_id: &str,
    dispatch_id: &str,
    issuer_agent_id: &str,
    tool_use_id: &str,
) -> Result<VerifierCapability> {
    let secret: [u8; 32] = rand::random();
    issue_server_verifier_handoff_with_secret(
        cas_dir,
        task_id,
        dispatch_id,
        issuer_agent_id,
        tool_use_id,
        &secret,
    )
}

/// Deterministic entropy seam for privacy regressions.
///
/// Production callers must use [`issue_server_verifier_handoff`]. This
/// function still returns only the durable hash-only capability, never
/// `secret`.
#[doc(hidden)]
pub fn issue_server_verifier_handoff_with_secret(
    cas_dir: &Path,
    task_id: &str,
    dispatch_id: &str,
    issuer_agent_id: &str,
    tool_use_id: &str,
    secret: &[u8],
) -> Result<VerifierCapability> {
    if tool_use_id.trim().is_empty() || secret.is_empty() {
        return Err(StoreError::Parse(
            "verifier handoff requires hook-local correlation and entropy".to_string(),
        ));
    }

    let store = SqliteVerificationStore::open(cas_dir)?;
    let mut conn = store.conn.lock().map_err(lock_err)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = Utc::now();
    let now_value = now.to_rfc3339();

    // Failed/cancelled spawns that never reached a terminal hook are recoverable
    // after expiry. Bound or consumed rows are immutable audit records.
    let mut expired_stmt = tx.prepare(
        "SELECT h.capability_id
         FROM verification_handoffs h
         JOIN verification_capabilities c ON c.id = h.capability_id
         WHERE h.state = 'pending' AND c.verifier_agent_id IS NULL
               AND c.consumed_at IS NULL AND c.expires_at <= ?1",
    )?;
    let expired_ids: Vec<String> = expired_stmt
        .query_map(params![now_value], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(expired_stmt);
    for id in expired_ids {
        tx.execute(
            "DELETE FROM verification_handoffs WHERE capability_id = ?1 AND state = 'pending'",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM verification_capabilities
             WHERE id = ?1 AND verifier_agent_id IS NULL AND consumed_at IS NULL",
            params![id],
        )?;
    }

    let pending_count: i64 = tx.query_row(
        "SELECT COUNT(*)
         FROM verification_handoffs h
         JOIN verification_capabilities c ON c.id = h.capability_id
         WHERE h.issuer_agent_id = ?1 AND h.state = 'pending'
               AND c.verifier_agent_id IS NULL AND c.consumed_at IS NULL
               AND c.expires_at > ?2",
        params![issuer_agent_id, now_value],
        |row| row.get(0),
    )?;
    if pending_count != 0 {
        return Err(StoreError::Parse(
            "another task-verifier spawn is already awaiting SubagentStart for this parent; wait for it to bind or for exact failure cleanup/expiry"
                .to_string(),
        ));
    }

    let dispatch = get_verification_dispatch_with_conn(&tx, dispatch_id)?;
    if dispatch.task_id != task_id
        || dispatch.owner_agent_id != issuer_agent_id
        || dispatch.state != VerificationDispatchState::Pending
        || dispatch.deadline_at <= now
    {
        return Err(StoreError::Parse(
            "verifier handoff requires the exact active owned dispatch".to_string(),
        ));
    }

    let issued_at = now;
    let expires_at = std::cmp::min(
        dispatch.deadline_at,
        issued_at + Duration::minutes(VERIFIER_CAPABILITY_TTL_MINUTES),
    );
    let capability = VerifierCapability {
        id: format!("{SERVER_HANDOFF_ID_PREFIX}{:032x}", rand::random::<u128>()),
        task_id: task_id.to_string(),
        dispatch_id: Some(dispatch.id),
        issuer_agent_id: issuer_agent_id.to_string(),
        verifier_agent_id: None,
        token_hash: verifier_handoff_secret_hash(secret),
        issued_at,
        expires_at,
        bound_at: None,
        consumed_at: None,
    };
    tx.execute(
        "INSERT INTO verification_capabilities
         (id, task_id, dispatch_id, issuer_agent_id, verifier_agent_id, token_hash,
          issued_at, expires_at, bound_at, consumed_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, NULL, NULL)",
        params![
            capability.id,
            capability.task_id,
            capability.dispatch_id,
            capability.issuer_agent_id,
            capability.token_hash,
            capability.issued_at.to_rfc3339(),
            capability.expires_at.to_rfc3339(),
        ],
    )?;
    tx.execute(
        "INSERT INTO verification_handoffs
         (capability_id, issuer_agent_id, verifier_agent_id, tool_use_id_hash,
          state, created_at, bound_at, consumed_at)
         VALUES (?1, ?2, NULL, ?3, 'pending', ?4, NULL, NULL)",
        params![
            capability.id,
            capability.issuer_agent_id,
            verifier_handoff_tool_hash(tool_use_id),
            capability.issued_at.to_rfc3339(),
        ],
    )?;
    tx.commit()?;
    Ok(capability)
}

/// Bind the sole live server handoff for one registered parent to the exact
/// official child and atomically claim its exact dispatch.
///
/// There is intentionally no ordering fallback: zero or multiple candidates
/// fail closed.
pub fn bind_server_verifier_handoff(
    cas_dir: &Path,
    issuer_agent_id: &str,
    verifier_agent_id: &str,
) -> Result<VerifierCapability> {
    validate_distinct_verifier_child(issuer_agent_id, verifier_agent_id)?;
    let store = SqliteVerificationStore::open(cas_dir)?;
    let mut conn = store.conn.lock().map_err(lock_err)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let bound = bind_server_verifier_handoff_with_conn(&tx, issuer_agent_id, verifier_agent_id)?;
    tx.commit()?;
    Ok(bound)
}

/// Bind and claim one exact server handoff together with registration of its
/// official verifier child. Agent and verification state share one SQLite
/// connection, so a registry failure rolls back every authority transition.
pub fn bind_server_verifier_handoff_and_register_child(
    cas_dir: &Path,
    issuer_agent_id: &str,
    child: &Agent,
) -> Result<VerifierCapability> {
    validate_distinct_verifier_child(issuer_agent_id, &child.id)?;
    if child.name != "task-verifier"
        || child.agent_type != AgentType::SubAgent
        || child.role != AgentRole::Standard
        || child.parent_id.as_deref() != Some(issuer_agent_id)
        || child.status != AgentStatus::Active
    {
        return Err(StoreError::Parse(
            "verifier handoff requires the exact official task-verifier child identity".to_string(),
        ));
    }

    let store = SqliteVerificationStore::open(cas_dir)?;
    let mut conn = store.conn.lock().map_err(lock_err)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let issuer_status: Option<String> = tx
        .query_row(
            "SELECT status FROM agents WHERE id = ?1",
            params![issuer_agent_id],
            |row| row.get(0),
        )
        .optional()?;
    if !issuer_status.as_deref().is_some_and(|status| {
        matches!(
            status.parse::<AgentStatus>(),
            Ok(AgentStatus::Active | AgentStatus::Idle)
        )
    }) {
        return Err(StoreError::Parse(
            "verifier handoff issuer is not an active registered parent".to_string(),
        ));
    }

    let existing_identity: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT agent_type, role, parent_id FROM agents WHERE id = ?1",
            params![child.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if existing_identity
        .as_ref()
        .is_some_and(|(agent_type, role, parent_id)| {
            agent_type.parse::<AgentType>().ok() != Some(AgentType::SubAgent)
                || role.parse::<AgentRole>().ok() != Some(AgentRole::Standard)
                || parent_id.as_deref() != Some(issuer_agent_id)
        })
    {
        return Err(StoreError::Parse(
            "verifier child identity conflicts with an existing registered session".to_string(),
        ));
    }

    let bound = bind_server_verifier_handoff_with_conn(&tx, issuer_agent_id, &child.id)?;
    register_agent_with_conn(&tx, child)?;
    tx.commit()?;
    Ok(bound)
}

fn validate_distinct_verifier_child(issuer_agent_id: &str, verifier_agent_id: &str) -> Result<()> {
    if issuer_agent_id.trim().is_empty()
        || verifier_agent_id.trim().is_empty()
        || issuer_agent_id == verifier_agent_id
    {
        return Err(StoreError::Parse(
            "verifier handoff requires a distinct registered child".to_string(),
        ));
    }
    Ok(())
}

fn bind_server_verifier_handoff_with_conn(
    conn: &Connection,
    issuer_agent_id: &str,
    verifier_agent_id: &str,
) -> Result<VerifierCapability> {
    let now = Utc::now();
    let mut stmt = conn.prepare(
        "SELECT h.capability_id
         FROM verification_handoffs h
         JOIN verification_capabilities c ON c.id = h.capability_id
         WHERE h.issuer_agent_id = ?1 AND h.state = 'pending'
               AND c.verifier_agent_id IS NULL AND c.consumed_at IS NULL
               AND c.expires_at > ?2
         ORDER BY h.capability_id",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![issuer_agent_id, now.to_rfc3339()], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    if ids.len() != 1 {
        return Err(StoreError::Parse(
            "verifier handoff is missing or ambiguous for this registered parent".to_string(),
        ));
    }

    let capability = load_capability_with_conn(conn, &ids[0])?;
    if !is_server_handoff(&capability) || capability.issuer_agent_id != issuer_agent_id {
        return Err(StoreError::Parse(
            "verifier handoff type or issuer binding is invalid".to_string(),
        ));
    }
    let dispatch_id = capability.dispatch_id.as_deref().ok_or_else(|| {
        StoreError::Parse("verifier handoff has no exact proof boundary".to_string())
    })?;
    let dispatch = get_verification_dispatch_with_conn(conn, dispatch_id)?;
    if dispatch.task_id != capability.task_id
        || dispatch.owner_agent_id != issuer_agent_id
        || dispatch.state != VerificationDispatchState::Pending
        || dispatch.deadline_at <= now
    {
        return Err(StoreError::Parse(
            "verifier handoff exact dispatch is not active".to_string(),
        ));
    }

    let bound_at = now;
    let changed = conn.execute(
        "UPDATE verification_capabilities
         SET verifier_agent_id = ?2, bound_at = ?3
         WHERE id = ?1 AND verifier_agent_id IS NULL AND consumed_at IS NULL
               AND expires_at > ?3",
        params![capability.id, verifier_agent_id, bound_at.to_rfc3339()],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verifier handoff binding raced or expired".to_string(),
        ));
    }
    let handoff_changed = conn.execute(
        "UPDATE verification_handoffs
         SET verifier_agent_id = ?2, state = 'bound', bound_at = ?3
         WHERE capability_id = ?1 AND issuer_agent_id = ?4 AND state = 'pending'
               AND verifier_agent_id IS NULL",
        params![
            capability.id,
            verifier_agent_id,
            bound_at.to_rfc3339(),
            issuer_agent_id,
        ],
    )?;
    if handoff_changed != 1 {
        return Err(StoreError::Parse(
            "verifier handoff state binding raced".to_string(),
        ));
    }
    let claimed = conn.execute(
        "UPDATE verification_dispatches
         SET verifier_agent_id = ?3, capability_id = ?4, state = 'claimed'
         WHERE id = ?1 AND owner_agent_id = ?2 AND state = 'pending'
               AND deadline_at > ?5",
        params![
            dispatch.id,
            issuer_agent_id,
            verifier_agent_id,
            capability.id,
            now.to_rfc3339(),
        ],
    )?;
    if claimed != 1 {
        return Err(StoreError::Parse(
            "verifier handoff could not claim its exact dispatch".to_string(),
        ));
    }
    load_capability_with_conn(conn, &capability.id)
}

/// Remove one exact failed/cancelled spawn handoff by hook-local tool-use ID.
///
/// Bound and consumed rows are immutable and are never selected by this
/// cleanup path.
pub fn cancel_unbound_server_verifier_handoff(
    cas_dir: &Path,
    issuer_agent_id: &str,
    tool_use_id: &str,
) -> Result<bool> {
    if issuer_agent_id.trim().is_empty() || tool_use_id.trim().is_empty() {
        return Ok(false);
    }
    let store = SqliteVerificationStore::open(cas_dir)?;
    let mut conn = store.conn.lock().map_err(lock_err)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tool_use_id_hash = verifier_handoff_tool_hash(tool_use_id);
    let mut stmt = tx.prepare(
        "SELECT h.capability_id
         FROM verification_handoffs h
         JOIN verification_capabilities c ON c.id = h.capability_id
         WHERE h.issuer_agent_id = ?1 AND h.state = 'pending'
               AND h.verifier_agent_id IS NULL AND c.verifier_agent_id IS NULL
               AND c.consumed_at IS NULL AND h.tool_use_id_hash = ?2
         ORDER BY h.capability_id",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![issuer_agent_id, tool_use_id_hash], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    match ids.as_slice() {
        [] => {
            tx.commit()?;
            Ok(false)
        }
        [id] => {
            let handoff_changed = tx.execute(
                "DELETE FROM verification_handoffs
                 WHERE capability_id = ?1 AND state = 'pending'
                       AND verifier_agent_id IS NULL",
                params![id],
            )?;
            let changed = if handoff_changed == 1 {
                tx.execute(
                    "DELETE FROM verification_capabilities
                     WHERE id = ?1 AND verifier_agent_id IS NULL AND consumed_at IS NULL",
                    params![id],
                )?
            } else {
                0
            };
            tx.commit()?;
            Ok(changed == 1)
        }
        _ => Err(StoreError::Parse(
            "verifier handoff cleanup is ambiguous".to_string(),
        )),
    }
}

/// Inspect the unique bound server handoff for one authenticated child.
///
/// The result is used only to select the exact dispatch before the verdict
/// transaction. Consumption revalidates every field atomically.
pub fn inspect_bound_server_verifier_handoff(
    cas_dir: &Path,
    task_id: &str,
    verifier_agent_id: &str,
    dispatch_id: Option<&str>,
) -> Result<VerifierCapability> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let mut stmt = conn.prepare(
        "SELECT h.capability_id
         FROM verification_handoffs h
         JOIN verification_capabilities c ON c.id = h.capability_id
         WHERE h.state = 'bound' AND h.verifier_agent_id = ?2
               AND c.task_id = ?1 AND c.verifier_agent_id = ?2
               AND c.consumed_at IS NULL
               AND (?3 IS NULL OR c.dispatch_id = ?3)
         ORDER BY h.capability_id",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![task_id, verifier_agent_id, dispatch_id], |row| {
            row.get(0)
        })?
        .collect::<std::result::Result<_, _>>()?;
    if ids.len() != 1 {
        return Err(StoreError::Parse(
            "bound verifier handoff is missing or ambiguous".to_string(),
        ));
    }
    let capability = load_capability_with_conn(&conn, &ids[0])?;
    if !is_server_handoff(&capability) {
        return Err(StoreError::Parse(
            "bound verifier handoff type is invalid".to_string(),
        ));
    }
    Ok(capability)
}

/// Atomically consume one exact server-side verifier handoff.
pub fn consume_server_verifier_handoff_with_conn(
    conn: &Connection,
    capability_id: &str,
    task_id: &str,
    verifier_agent_id: &str,
) -> Result<VerifierCapability> {
    let capability = load_capability_with_conn(conn, capability_id)?;
    if !is_server_handoff(&capability) {
        return Err(StoreError::Parse(
            "verifier handoff type is invalid".to_string(),
        ));
    }
    let dispatch_id = capability.dispatch_id.as_deref().ok_or_else(|| {
        StoreError::Parse("verifier handoff has no exact proof boundary".to_string())
    })?;
    let dispatch = get_verification_dispatch_with_conn(conn, dispatch_id)?;
    let now = Utc::now();
    let handoff_valid: i64 = conn.query_row(
        "SELECT COUNT(*) FROM verification_handoffs
         WHERE capability_id = ?1 AND issuer_agent_id = ?2
               AND verifier_agent_id = ?3 AND state = 'bound'
               AND consumed_at IS NULL",
        params![capability.id, capability.issuer_agent_id, verifier_agent_id],
        |row| row.get(0),
    )?;
    if capability.task_id != task_id
        || capability.verifier_agent_id.as_deref() != Some(verifier_agent_id)
        || capability.consumed_at.is_some()
        || capability.expires_at <= now
        || dispatch.task_id != task_id
        || dispatch.state != VerificationDispatchState::Claimed
        || dispatch.verifier_agent_id.as_deref() != Some(verifier_agent_id)
        || dispatch.capability_id.as_deref() != Some(capability.id.as_str())
        || dispatch.deadline_at <= now
        || handoff_valid != 1
    {
        return Err(StoreError::Parse(
            "verifier handoff is invalid, expired, consumed, or bound to another task/session"
                .to_string(),
        ));
    }
    let changed = conn.execute(
        "UPDATE verification_capabilities
         SET consumed_at = ?2
         WHERE id = ?1 AND consumed_at IS NULL AND verifier_agent_id = ?3
               AND task_id = ?4 AND expires_at > ?2",
        params![capability.id, now.to_rfc3339(), verifier_agent_id, task_id],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verifier handoff was already consumed".to_string(),
        ));
    }
    let handoff_changed = conn.execute(
        "UPDATE verification_handoffs
         SET state = 'consumed', consumed_at = ?2
         WHERE capability_id = ?1 AND verifier_agent_id = ?3
               AND state = 'bound' AND consumed_at IS NULL",
        params![capability.id, now.to_rfc3339(), verifier_agent_id],
    )?;
    if handoff_changed != 1 {
        return Err(StoreError::Parse(
            "verifier handoff audit consumption raced".to_string(),
        ));
    }
    Ok(capability)
}

/// Bind an issued capability to the distinct registered child session that
/// receives the task-verifier prompt.
pub fn bind_verifier_capability(
    cas_dir: &Path,
    token: &str,
    verifier_agent_id: &str,
) -> Result<VerifierCapability> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let id = capability_id_from_token(token)?;
    let capability = load_capability_with_conn(&conn, id)?;
    let dispatch_id = capability.dispatch_id.as_deref().ok_or_else(|| {
        StoreError::Parse("legacy verifier capability has no proof boundary".to_string())
    })?;
    let dispatch = get_verification_dispatch_with_conn(&conn, dispatch_id)?;
    if capability.consumed_at.is_some()
        || capability.verifier_agent_id.is_some()
        || capability.expires_at <= Utc::now()
        || dispatch.state != VerificationDispatchState::Pending
        || dispatch.owner_agent_id != capability.issuer_agent_id
        || dispatch.deadline_at <= Utc::now()
        || !constant_time_eq(&capability.token_hash, &capability_token_hash(token))
        || verifier_agent_id == capability.issuer_agent_id
    {
        return Err(StoreError::Parse(
            "verifier capability cannot be bound".to_string(),
        ));
    }
    let bound_at = Utc::now();
    let changed = conn.execute(
        "UPDATE verification_capabilities
         SET verifier_agent_id = ?2, bound_at = ?3
         WHERE id = ?1 AND verifier_agent_id IS NULL AND consumed_at IS NULL
               AND expires_at > ?3",
        params![id, verifier_agent_id, bound_at.to_rfc3339()],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verifier capability binding raced or expired".to_string(),
        ));
    }
    load_capability_with_conn(&conn, id)
}

/// Validate and atomically consume one verifier capability in the caller's
/// verification transaction.
pub fn consume_verifier_capability_with_conn(
    conn: &Connection,
    token: &str,
    task_id: &str,
    verifier_agent_id: &str,
) -> Result<VerifierCapability> {
    let id = capability_id_from_token(token)?;
    let capability = load_capability_with_conn(conn, id)?;
    let dispatch_id = capability.dispatch_id.as_deref().ok_or_else(|| {
        StoreError::Parse("legacy verifier capability has no proof boundary".to_string())
    })?;
    let dispatch = get_verification_dispatch_with_conn(conn, dispatch_id)?;
    if capability.task_id != task_id
        || capability.verifier_agent_id.as_deref() != Some(verifier_agent_id)
        || capability.consumed_at.is_some()
        || capability.expires_at <= Utc::now()
        || dispatch.task_id != task_id
        || dispatch.state != VerificationDispatchState::Claimed
        || dispatch.verifier_agent_id.as_deref() != Some(verifier_agent_id)
        || dispatch.capability_id.as_deref() != Some(capability.id.as_str())
        || dispatch.deadline_at <= Utc::now()
        || !constant_time_eq(&capability.token_hash, &capability_token_hash(token))
    {
        return Err(StoreError::Parse(
            "verifier capability is invalid, expired, consumed, or bound to another task/session"
                .to_string(),
        ));
    }
    let consumed_at = Utc::now();
    let changed = conn.execute(
        "UPDATE verification_capabilities
         SET consumed_at = ?2
         WHERE id = ?1 AND consumed_at IS NULL AND verifier_agent_id = ?3
               AND task_id = ?4 AND expires_at > ?2",
        params![id, consumed_at.to_rfc3339(), verifier_agent_id, task_id],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "verifier capability was already consumed".to_string(),
        ));
    }
    Ok(capability)
}

impl VerificationStore for SqliteVerificationStore {
    fn init(&self) -> Result<()> {
        // `VERIFICATION_SCHEMA` is a modern baseline. On a database whose
        // `verification_dispatches` table predates m230, its CREATE TABLE IF
        // NOT EXISTS is a no-op and cannot add `repository_proof`, although
        // every dispatch read selects it. Repair the shape before returning
        // from any store open (CLI, MCP, or direct caller), without claiming
        // the numbered migration's ledger row. The normal runner sees m230's
        // detection query succeed and records it. `ensure_column` makes
        // concurrent openers idempotent and race-safe.
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().map_err(lock_err)?;
            conn.execute_batch(VERIFICATION_SCHEMA)?;
            crate::shared_db::ensure_column(
                &conn,
                "verification_dispatches",
                "repository_proof",
                VERIFICATION_DISPATCH_REPOSITORY_PROOF_STATEMENT,
            )?;

            // Serve-first startup can open current store code against a legacy
            // primary/side table before m213 runs. SQLite cannot conditionally
            // create an index on a missing column, so add these bootstrap indexes
            // only when both additive columns already exist. The migration creates
            // them after upgrading legacy tables.
            let has_exact_boundary_columns = conn.query_row(
                "SELECT CASE WHEN
                    EXISTS (
                        SELECT 1 FROM pragma_table_info('verification_capabilities')
                        WHERE name = 'dispatch_id'
                    )
                    AND EXISTS (
                        SELECT 1 FROM pragma_table_info('verifications')
                        WHERE name = 'dispatch_id'
                    )
                 THEN 1 ELSE 0 END",
                [],
                |row| row.get::<_, i64>(0),
            )? == 1;
            if has_exact_boundary_columns {
                conn.execute_batch(
                    "CREATE INDEX IF NOT EXISTS idx_verification_capabilities_dispatch
                        ON verification_capabilities(dispatch_id);
                     CREATE INDEX IF NOT EXISTS idx_verifications_dispatch
                        ON verifications(dispatch_id, created_at DESC);",
                )?;
            }
            Ok(())
        })
    }

    fn generate_id(&self) -> Result<String> {
        use std::time::{SystemTime, UNIX_EPOCH};

        // The pre-cas-3bd4 implementation used `timestamp_millis & 0xffff`
        // (last 4 hex chars), which collides for any two calls landing
        // in the same millisecond — exactly what happens when a task
        // racks up a dispatch row and a skip row back-to-back during a
        // single close path. The collision triggers
        // `UNIQUE constraint failed: verifications.id` and silently
        // drops the newer row.
        //
        // Mix nanoseconds with a per-process random seed so rapid
        // successive calls produce distinct ids even inside the same
        // millisecond.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let rand: u32 = rand::random();
        // 8 hex chars from nanos + 4 from randomness = 48 bits of
        // collision surface, plenty for in-process use.
        Ok(format!(
            "ver-{:08x}{:04x}",
            (nanos as u64) & 0xffff_ffff,
            rand & 0xffff
        ))
    }

    fn add(&self, verification: &Verification) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let verification = sanitized_verification_for_write(verification);
        validate_verification_authority_with_conn(&conn, &verification, false)?;
        insert_verification_with_conn(&conn, &verification)
    }

    fn get(&self, id: &str) -> Result<Verification> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut verification = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                        dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                        duration_ms, created_at
                 FROM verifications WHERE id = ?1",
                params![id],
                Self::parse_verification,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id.to_string()),
                _ => StoreError::Database(e),
            })?;

        verification.issues = self.load_issues(&conn, id)?;

        Ok(verification)
    }

    fn update(&self, verification: &Verification) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let verification = sanitized_verification_for_write(verification);

        let existing = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                        dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                        duration_ms, created_at
                 FROM verifications WHERE id = ?1",
                params![verification.id],
                Self::parse_verification,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(verification.id.clone()))?;
        if existing.task_id != verification.task_id
            || existing.agent_id != verification.agent_id
            || existing.verification_type != verification.verification_type
            || existing.provenance != verification.provenance
            || existing.capability_id != verification.capability_id
            || existing.dispatch_id != verification.dispatch_id
            || existing.issuer_agent_id != verification.issuer_agent_id
        {
            return Err(StoreError::Parse(
                "verification authority and identity fields are immutable".to_string(),
            ));
        }
        validate_verification_authority_with_conn(&conn, &verification, true)?;

        let files_reviewed_json = serde_json::to_string(&verification.files_reviewed)
            .unwrap_or_else(|_| "[]".to_string());

        let rows = conn.execute(
            "UPDATE verifications SET
             task_id = ?2, agent_id = ?3, verification_type = ?4, provenance = ?5,
             capability_id = ?6, dispatch_id = ?7, issuer_agent_id = ?8, status = ?9,
             confidence = ?10, summary = ?11, files_reviewed = ?12, duration_ms = ?13,
             created_at = ?14
             WHERE id = ?1",
            params![
                verification.id,
                verification.task_id,
                verification.agent_id,
                verification.verification_type.to_string(),
                verification.provenance.to_string(),
                verification.capability_id,
                verification.dispatch_id,
                verification.issuer_agent_id,
                verification.status.to_string(),
                verification.confidence,
                verification.summary,
                files_reviewed_json,
                verification.duration_ms.map(|v| v as i64),
                verification.created_at.to_rfc3339(),
            ],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(verification.id.clone()));
        }

        // Update issues
        self.save_issues(&conn, &verification)?;

        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        // Issues deleted via CASCADE
        let rows = conn.execute("DELETE FROM verifications WHERE id = ?1", params![id])?;

        if rows == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }

        Ok(())
    }

    fn get_for_task(&self, task_id: &str) -> Result<Vec<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                    dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                    duration_ms, created_at
             FROM verifications WHERE task_id = ?1
             ORDER BY created_at DESC",
        )?;

        let mut verifications: Vec<Verification> = stmt
            .query_map(params![task_id], Self::parse_verification)?
            .filter_map(|r| r.ok())
            .collect();

        self.attach_issues_batch(&conn, &mut verifications)?;

        Ok(verifications)
    }

    fn get_latest_for_task(&self, task_id: &str) -> Result<Option<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let verification = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                        dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                        duration_ms, created_at
                 FROM verifications WHERE task_id = ?1
                 ORDER BY created_at DESC LIMIT 1",
                params![task_id],
                Self::parse_verification,
            )
            .optional()
            .map_err(StoreError::Database)?;

        match verification {
            Some(mut v) => {
                v.issues = self.load_issues(&conn, &v.id)?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn get_latest_for_task_by_type(
        &self,
        task_id: &str,
        verification_type: VerificationType,
    ) -> Result<Option<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let verification = conn
            .query_row(
                "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                        dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                        duration_ms, created_at
                 FROM verifications WHERE task_id = ?1 AND verification_type = ?2
                 ORDER BY created_at DESC LIMIT 1",
                params![task_id, verification_type.to_string()],
                Self::parse_verification,
            )
            .optional()
            .map_err(StoreError::Database)?;

        match verification {
            Some(mut v) => {
                v.issues = self.load_issues(&conn, &v.id)?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                    dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                    duration_ms, created_at
             FROM verifications ORDER BY created_at DESC LIMIT ?1",
        )?;

        let mut verifications: Vec<Verification> = stmt
            .query_map(params![limit as i32], Self::parse_verification)?
            .filter_map(|r| r.ok())
            .collect();

        self.attach_issues_batch(&conn, &mut verifications)?;

        Ok(verifications)
    }

    fn list_by_status(&self, status: VerificationStatus) -> Result<Vec<Verification>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, verification_type, provenance, capability_id,
                    dispatch_id, issuer_agent_id, status, confidence, summary, files_reviewed,
                    duration_ms, created_at
             FROM verifications WHERE status = ?1
             ORDER BY created_at DESC",
        )?;

        let mut verifications: Vec<Verification> = stmt
            .query_map(params![status.to_string()], Self::parse_verification)?
            .filter_map(|r| r.ok())
            .collect();

        self.attach_issues_batch(&conn, &mut verifications)?;

        Ok(verifications)
    }

    fn prune(&self, older_than_days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        // Issues are deleted via CASCADE
        let rows = conn.execute(
            "DELETE FROM verifications WHERE created_at < ?",
            params![cutoff],
        )?;

        Ok(rows)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::verification_store::*;
    use crate::{SqliteTaskStore, TaskStore};
    use tempfile::TempDir;

    fn create_test_store() -> (SqliteVerificationStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = SqliteVerificationStore::open(dir.path()).unwrap();
        (store, dir)
    }

    /// The exact m213 dispatch table shape before m230 made repository proofs
    /// durable. This is intentionally created outside the store so its modern
    /// baseline DDL cannot mask the missing-column upgrade.
    fn create_pre_m230_dispatch_store(cas_dir: &std::path::Path) {
        let conn = Connection::open(cas_dir.join("cas.db")).expect("legacy fixture db");
        conn.execute_batch(
            "CREATE TABLE verification_dispatches (
                 id TEXT PRIMARY KEY,
                 task_id TEXT NOT NULL,
                 receipt_id TEXT,
                 delivery_transaction_id TEXT,
                 requester_agent_id TEXT NOT NULL,
                 owner_agent_id TEXT NOT NULL,
                 verifier_agent_id TEXT,
                 capability_id TEXT,
                 state TEXT NOT NULL DEFAULT 'pending',
                 requested_at TEXT NOT NULL,
                 deadline_at TEXT NOT NULL,
                 resolved_at TEXT,
                 recovery_action TEXT NOT NULL DEFAULT 'supervisor_redispatch_or_direct'
             );
             INSERT INTO verification_dispatches
                 (id, task_id, requester_agent_id, owner_agent_id, requested_at, deadline_at)
             VALUES
                 ('vdispatch-pre-m230', 'cas-pre-m230', 'worker', 'supervisor',
                  '2026-08-09T22:00:00+00:00', '2026-08-09T23:00:00+00:00');",
        )
        .expect("create pre-m230 dispatch fixture");
    }

    #[test]
    fn open_self_heals_pre_m230_dispatches_before_the_first_dispatch_query() {
        let dir = TempDir::new().unwrap();
        create_pre_m230_dispatch_store(dir.path());

        // This reproduces the close/verification failure shape: the first
        // typed dispatch read selects repository_proof from a pre-m230 table.
        let dispatch = get_latest_verification_dispatch(dir.path(), "cas-pre-m230")
            .expect("store open must repair the m230 column before querying")
            .expect("legacy dispatch remains readable");
        assert_eq!(dispatch.id, "vdispatch-pre-m230");
        assert_eq!(dispatch.repository, None);

        // A second open is the idempotency check; m230's normal migration
        // runner remains responsible for recording its ledger row later.
        let reopened = SqliteVerificationStore::open(dir.path())
            .expect("reopening an already repaired store must be a no-op");
        drop(reopened);
        let conn = Connection::open(dir.path().join("cas.db")).expect("upgraded fixture db");
        assert!(crate::shared_db::column_exists(
            &conn,
            "verification_dispatches",
            "repository_proof"
        ));
    }

    #[test]
    fn test_add_and_get_verification() {
        let (store, _dir) = create_test_store();

        let verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "All checks passed".to_string(),
        );

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert_eq!(retrieved.id, "ver-test");
        assert_eq!(retrieved.task_id, "cas-1234");
        assert!(retrieved.is_approved());
        assert_eq!(retrieved.summary, "All checks passed");
    }

    #[test]
    fn test_verification_with_issues() {
        let (store, _dir) = create_test_store();

        let issues = vec![
            VerificationIssue::blocking(
                "src/main.rs".to_string(),
                Some(42),
                "todo_comment".to_string(),
                "// TODO: implement".to_string(),
                "TODO comment found".to_string(),
                Some("Implement the function".to_string()),
            ),
            VerificationIssue::warning(
                "src/lib.rs".to_string(),
                "hardcoded_value".to_string(),
                "Magic number detected".to_string(),
            ),
        ];

        let verification = Verification::rejected(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "Found incomplete work".to_string(),
            issues,
        );

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert!(retrieved.is_rejected());
        assert_eq!(retrieved.issues.len(), 2);
        assert_eq!(retrieved.blocking_count(), 1);
        assert_eq!(retrieved.warning_count(), 1);

        let first_issue = &retrieved.issues[0];
        assert_eq!(first_issue.file, "src/main.rs");
        assert_eq!(first_issue.line, Some(42));
        assert!(first_issue.is_blocking());
    }

    #[test]
    fn test_update_verification() {
        let (store, _dir) = create_test_store();

        let mut verification = Verification::new("ver-test".to_string(), "cas-1234".to_string());
        store.add(&verification).unwrap();

        // Update with new status and issues
        verification.status = VerificationStatus::Rejected;
        verification.summary = "Found issues".to_string();
        verification.issues.push(VerificationIssue::new(
            "src/api.rs".to_string(),
            "stub".to_string(),
            "Stub implementation".to_string(),
        ));

        store.update(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert!(retrieved.is_rejected());
        assert_eq!(retrieved.issues.len(), 1);
    }

    #[test]
    fn test_get_for_task() {
        let (store, _dir) = create_test_store();

        // Add multiple verifications for same task
        for i in 0..3 {
            let verification = Verification::approved(
                format!("ver-{i}"),
                "cas-1234".to_string(),
                format!("Attempt {i}"),
            );
            store.add(&verification).unwrap();
        }

        // Add one for different task
        let other = Verification::approved(
            "ver-other".to_string(),
            "cas-5678".to_string(),
            "Other task".to_string(),
        );
        store.add(&other).unwrap();

        let task_verifications = store.get_for_task("cas-1234").unwrap();
        assert_eq!(task_verifications.len(), 3);
    }

    #[test]
    fn test_get_latest_for_task() {
        let (store, _dir) = create_test_store();

        // No verifications initially
        let latest = store.get_latest_for_task("cas-1234").unwrap();
        assert!(latest.is_none());

        // Add verifications
        let v1 = Verification::rejected(
            "ver-1".to_string(),
            "cas-1234".to_string(),
            "First attempt".to_string(),
            vec![],
        );
        store.add(&v1).unwrap();

        let v2 = Verification::approved(
            "ver-2".to_string(),
            "cas-1234".to_string(),
            "Second attempt".to_string(),
        );
        store.add(&v2).unwrap();

        let latest = store.get_latest_for_task("cas-1234").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().id, "ver-2");
    }

    #[test]
    fn test_get_latest_for_task_by_type() {
        let (store, _dir) = create_test_store();

        // No verifications initially
        let latest = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Task)
            .unwrap();
        assert!(latest.is_none());

        let latest = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Epic)
            .unwrap();
        assert!(latest.is_none());

        // Add a task-type verification
        let mut v1 = Verification::approved(
            "ver-task-1".to_string(),
            "cas-1234".to_string(),
            "Task verification".to_string(),
        );
        v1.verification_type = VerificationType::Task;
        store.add(&v1).unwrap();

        // Add an epic-type verification
        let mut v2 = Verification::rejected(
            "ver-epic-1".to_string(),
            "cas-1234".to_string(),
            "Epic verification".to_string(),
            vec![],
        );
        v2.verification_type = VerificationType::Epic;
        store.add(&v2).unwrap();

        // Add another task-type verification (newer)
        let mut v3 = Verification::approved(
            "ver-task-2".to_string(),
            "cas-1234".to_string(),
            "Second task verification".to_string(),
        );
        v3.verification_type = VerificationType::Task;
        store.add(&v3).unwrap();

        // Get latest task verification - should be v3
        let latest_task = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Task)
            .unwrap();
        assert!(latest_task.is_some());
        assert_eq!(latest_task.unwrap().id, "ver-task-2");

        // Get latest epic verification - should be v2
        let latest_epic = store
            .get_latest_for_task_by_type("cas-1234", VerificationType::Epic)
            .unwrap();
        assert!(latest_epic.is_some());
        assert_eq!(latest_epic.unwrap().id, "ver-epic-1");

        // Different task has no verifications
        let latest_other = store
            .get_latest_for_task_by_type("cas-5678", VerificationType::Task)
            .unwrap();
        assert!(latest_other.is_none());
    }

    #[test]
    fn test_list_by_status() {
        let (store, _dir) = create_test_store();

        let approved = Verification::approved(
            "ver-approved".to_string(),
            "cas-1".to_string(),
            "Good".to_string(),
        );
        store.add(&approved).unwrap();

        let rejected = Verification::rejected(
            "ver-rejected".to_string(),
            "cas-2".to_string(),
            "Bad".to_string(),
            vec![],
        );
        store.add(&rejected).unwrap();

        let approved_list = store.list_by_status(VerificationStatus::Approved).unwrap();
        assert_eq!(approved_list.len(), 1);
        assert_eq!(approved_list[0].id, "ver-approved");

        let rejected_list = store.list_by_status(VerificationStatus::Rejected).unwrap();
        assert_eq!(rejected_list.len(), 1);
        assert_eq!(rejected_list[0].id, "ver-rejected");
    }

    #[test]
    fn test_delete_verification() {
        let (store, _dir) = create_test_store();

        let verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "Good".to_string(),
        );
        store.add(&verification).unwrap();

        store.delete("ver-test").unwrap();

        let result = store.get("ver-test");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_id() {
        let (store, _dir) = create_test_store();

        let id = store.generate_id().unwrap();
        assert!(id.starts_with("ver-"));
        assert!(id.len() > 4);
    }

    #[test]
    fn test_list_recent() {
        let (store, _dir) = create_test_store();

        for i in 0..5 {
            let verification =
                Verification::approved(format!("ver-{i}"), format!("cas-{i}"), format!("Task {i}"));
            store.add(&verification).unwrap();
        }

        let recent = store.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_files_reviewed_persistence() {
        let (store, _dir) = create_test_store();

        let mut verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "Done".to_string(),
        );
        verification.add_file_reviewed("src/main.rs".to_string());
        verification.add_file_reviewed("src/lib.rs".to_string());
        verification.add_file_reviewed("tests/test.rs".to_string());

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert_eq!(retrieved.files_reviewed.len(), 3);
        assert!(
            retrieved
                .files_reviewed
                .contains(&"src/main.rs".to_string())
        );
    }

    #[test]
    fn test_verification_with_confidence() {
        let (store, _dir) = create_test_store();

        let mut verification = Verification::approved(
            "ver-test".to_string(),
            "cas-1234".to_string(),
            "High confidence".to_string(),
        );
        verification.set_confidence(0.95);

        store.add(&verification).unwrap();

        let retrieved = store.get("ver-test").unwrap();
        assert_eq!(retrieved.confidence, Some(0.95));
    }

    #[test]
    fn verifier_capability_is_hashed_scoped_bound_expiring_and_one_time() {
        let (_store, dir) = create_test_store();
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-cap-a",
            "owner-agent",
            "owner-agent",
            Utc::now() + Duration::minutes(10),
        )
        .expect("dispatch");
        let issued =
            issue_verifier_capability(dir.path(), "cas-cap-a", "owner-agent").expect("issue");

        assert!(
            bind_verifier_capability(dir.path(), &issued.token, "owner-agent").is_err(),
            "issuer/owner must never bind its own capability"
        );
        let bound =
            bind_verifier_capability(dir.path(), &issued.token, "verifier-child").expect("bind");
        assert_eq!(bound.verifier_agent_id.as_deref(), Some("verifier-child"));
        claim_verification_dispatch_bound(
            dir.path(),
            &dispatch.id,
            "owner-agent",
            "verifier-child",
            &issued.capability.id,
        )
        .expect("claim exact dispatch");

        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        assert!(
            consume_verifier_capability_with_conn(
                &conn,
                &issued.token,
                "cas-cap-b",
                "verifier-child"
            )
            .is_err(),
            "wrong-task capability use must fail"
        );
        assert!(
            consume_verifier_capability_with_conn(&conn, &issued.token, "cas-cap-a", "owner-agent")
                .is_err(),
            "stolen-token use from the owner session must fail"
        );
        consume_verifier_capability_with_conn(&conn, &issued.token, "cas-cap-a", "verifier-child")
            .expect("first correct consumption");
        assert!(
            consume_verifier_capability_with_conn(
                &conn,
                &issued.token,
                "cas-cap-a",
                "verifier-child"
            )
            .is_err(),
            "replayed capability must fail"
        );

        let persisted: String = conn
            .query_row(
                "SELECT id || task_id || issuer_agent_id ||
                        COALESCE(verifier_agent_id, '') || token_hash ||
                        issued_at || expires_at || COALESCE(bound_at, '') ||
                        COALESCE(consumed_at, '')
                 FROM verification_capabilities WHERE id = ?1",
                params![issued.capability.id],
                |row| row.get(0),
            )
            .expect("persisted capability");
        assert!(
            !persisted.contains(&issued.token),
            "raw verifier bearer must never be persisted"
        );
        assert!(
            persisted.contains(&capability_token_hash(&issued.token)),
            "only the domain-separated capability digest should persist"
        );
        resolve_verification_dispatch_with_conn(
            &conn,
            &dispatch.id,
            "verifier-child",
            Some(&issued.capability.id),
            false,
        )
        .expect("resolve consumed capability dispatch");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint capability writes");
        for suffix in ["", "-wal", "-shm"] {
            let path = dir.path().join(format!("cas.db{suffix}"));
            if let Ok(bytes) = std::fs::read(path) {
                assert!(
                    !bytes
                        .windows(issued.token.len())
                        .any(|window| window == issued.token.as_bytes()),
                    "raw capability must not appear in any SQLite payload"
                );
            }
        }

        create_verification_dispatch(
            dir.path(),
            "cas-cap-expired",
            "owner-agent",
            "owner-agent",
            Utc::now() + Duration::minutes(10),
        )
        .expect("expired test dispatch");
        let expired =
            issue_verifier_capability(dir.path(), "cas-cap-expired", "owner-agent").expect("issue");
        conn.execute(
            "UPDATE verification_capabilities SET expires_at = ?2 WHERE id = ?1",
            params![
                expired.capability.id,
                (Utc::now() - Duration::minutes(1)).to_rfc3339()
            ],
        )
        .expect("expire capability");
        assert!(
            bind_verifier_capability(dir.path(), &expired.token, "expired-child").is_err(),
            "expired capability must fail before binding"
        );
    }

    #[test]
    fn server_handoff_is_unique_parent_bound_exact_and_secret_free() {
        let (_store, dir) = create_test_store();
        let sentinel = b"CAS_SENTINEL_RAW_VERIFIER_CREDENTIAL_6939";
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-handoff-a",
            "parent-agent",
            "parent-agent",
            Utc::now() + Duration::minutes(10),
        )
        .expect("dispatch");
        let handoff = issue_server_verifier_handoff_with_secret(
            dir.path(),
            "cas-handoff-a",
            &dispatch.id,
            "parent-agent",
            "tool-use-a",
            sentinel,
        )
        .expect("sealed handoff");
        assert!(handoff.id.starts_with(SERVER_HANDOFF_ID_PREFIX));
        assert!(
            !serde_json::to_string(&handoff)
                .expect("serialize")
                .contains(std::str::from_utf8(sentinel).unwrap()),
            "typed handoff must never serialize raw entropy"
        );
        assert!(
            issue_server_verifier_handoff_with_secret(
                dir.path(),
                "cas-handoff-a",
                &dispatch.id,
                "parent-agent",
                "tool-use-b",
                b"other-secret",
            )
            .unwrap_err()
            .to_string()
            .contains("already awaiting SubagentStart"),
            "one parent cannot mint a second concurrent unbound handoff"
        );
        assert!(
            bind_server_verifier_handoff(dir.path(), "wrong-parent", "verifier-child").is_err(),
            "a different parent cannot bind the pending handoff"
        );

        let bound = bind_server_verifier_handoff(dir.path(), "parent-agent", "verifier-child")
            .expect("bind exact official child");
        assert_eq!(bound.id, handoff.id);
        assert_eq!(bound.verifier_agent_id.as_deref(), Some("verifier-child"));
        assert!(
            !cancel_unbound_server_verifier_handoff(dir.path(), "parent-agent", "tool-use-a")
                .expect("bound cleanup no-op"),
            "cleanup must never remove a bound audit row"
        );
        assert!(
            inspect_bound_server_verifier_handoff(
                dir.path(),
                "cas-handoff-a",
                "wrong-child",
                Some(&dispatch.id),
            )
            .is_err(),
            "a different child cannot inspect the bound handoff"
        );
        assert!(
            inspect_bound_server_verifier_handoff(
                dir.path(),
                "cas-handoff-other",
                "verifier-child",
                Some(&dispatch.id),
            )
            .is_err(),
            "the bound handoff cannot cross task scope"
        );
        assert!(
            inspect_bound_server_verifier_handoff(
                dir.path(),
                "cas-handoff-a",
                "verifier-child",
                Some("vdisp-wrong"),
            )
            .is_err(),
            "the bound handoff cannot cross dispatch scope"
        );

        let inspected = inspect_bound_server_verifier_handoff(
            dir.path(),
            "cas-handoff-a",
            "verifier-child",
            Some(&dispatch.id),
        )
        .expect("inspect exact bound handoff");
        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        consume_server_verifier_handoff_with_conn(
            &conn,
            &inspected.id,
            "cas-handoff-a",
            "verifier-child",
        )
        .expect("consume once");
        assert!(
            consume_server_verifier_handoff_with_conn(
                &conn,
                &inspected.id,
                "cas-handoff-a",
                "verifier-child",
            )
            .is_err(),
            "server handoff replay must fail"
        );
        drop(conn);

        let sentinel_text = std::str::from_utf8(sentinel).unwrap();
        for name in ["cas.db", "cas.db-wal", "cas.db-shm"] {
            let path = dir.path().join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !bytes
                        .windows(sentinel_text.len())
                        .any(|window| window == sentinel_text.as_bytes()),
                    "raw sentinel leaked into {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn server_handoff_cleanup_matches_only_exact_unbound_tool_use() {
        let (_store, dir) = create_test_store();
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-handoff-cleanup",
            "parent-agent",
            "parent-agent",
            Utc::now() + Duration::minutes(10),
        )
        .expect("dispatch");
        issue_server_verifier_handoff_with_secret(
            dir.path(),
            "cas-handoff-cleanup",
            &dispatch.id,
            "parent-agent",
            "tool-use-exact",
            b"cleanup-secret",
        )
        .expect("handoff");
        assert!(
            !cancel_unbound_server_verifier_handoff(dir.path(), "parent-agent", "tool-use-wrong")
                .expect("wrong cleanup")
        );
        assert!(
            cancel_unbound_server_verifier_handoff(dir.path(), "parent-agent", "tool-use-exact")
                .expect("exact cleanup")
        );
        assert!(
            bind_server_verifier_handoff(dir.path(), "parent-agent", "verifier-child").is_err(),
            "cleaned handoff cannot bind later"
        );
        let conn = Connection::open(dir.path().join("cas.db")).expect("db after cleanup");
        let pending_after_cleanup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM verification_handoffs WHERE state = 'pending'",
                [],
                |row| row.get(0),
            )
            .expect("pending after cleanup");
        assert_eq!(
            pending_after_cleanup, 0,
            "exact failure cleanup must not leave an orphan pending handoff"
        );
        drop(conn);

        let expired = issue_server_verifier_handoff_with_secret(
            dir.path(),
            "cas-handoff-cleanup",
            &dispatch.id,
            "parent-agent",
            "tool-use-expired",
            b"expired-secret",
        )
        .expect("expiring handoff");
        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        conn.execute(
            "UPDATE verification_capabilities SET expires_at = ?2 WHERE id = ?1",
            params![expired.id, (Utc::now() - Duration::seconds(1)).to_rfc3339()],
        )
        .expect("expire handoff");
        drop(conn);
        assert!(
            bind_server_verifier_handoff(dir.path(), "parent-agent", "verifier-child").is_err(),
            "expired handoff cannot bind"
        );
        let replacement = issue_server_verifier_handoff_with_secret(
            dir.path(),
            "cas-handoff-cleanup",
            &dispatch.id,
            "parent-agent",
            "tool-use-after-expiry",
            b"restart-secret",
        )
        .expect("expired unbound handoff is reaped before restart-safe issuance");
        let conn = Connection::open(dir.path().join("cas.db")).expect("db after restart");
        let (expired_rows, live_pending): (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM verification_handoffs
                     WHERE capability_id = ?1),
                    (SELECT COUNT(*) FROM verification_handoffs
                     WHERE capability_id = ?2 AND state = 'pending')",
                params![expired.id, replacement.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("restart handoff state");
        assert_eq!(
            expired_rows, 0,
            "restart/expiry recovery must reap the orphaned pending row"
        );
        assert_eq!(
            live_pending, 1,
            "restart/expiry recovery must leave exactly the replacement pending"
        );
    }

    #[test]
    fn verification_dispatch_is_durable_claimed_and_resolved_by_exact_authority() {
        let (_store, dir) = create_test_store();
        let deadline = Utc::now() + Duration::minutes(10);
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-dispatch-a",
            "worker-a",
            "supervisor-a",
            deadline,
        )
        .expect("create dispatch");
        assert_eq!(dispatch.state, VerificationDispatchState::Pending);
        assert_eq!(dispatch.owner_agent_id, "supervisor-a");
        assert_eq!(
            dispatch.recovery_action,
            VerificationRecoveryAction::SupervisorRedispatchOrDirect
        );

        assert!(
            create_verification_dispatch(
                dir.path(),
                "cas-dispatch-a",
                "worker-a",
                "other-owner",
                deadline,
            )
            .is_err(),
            "an active proof cycle cannot be rebound to a different owner"
        );
        assert!(
            claim_verification_dispatch(
                dir.path(),
                "cas-dispatch-a",
                "other-owner",
                "verifier-a",
                "vcap-a"
            )
            .is_err(),
            "a non-owner must not claim the dispatch"
        );

        let claimed = claim_verification_dispatch(
            dir.path(),
            "cas-dispatch-a",
            "supervisor-a",
            "verifier-a",
            "vcap-a",
        )
        .expect("claim dispatch");
        assert_eq!(claimed.state, VerificationDispatchState::Claimed);
        assert_eq!(claimed.verifier_agent_id.as_deref(), Some("verifier-a"));
        assert_eq!(claimed.capability_id.as_deref(), Some("vcap-a"));

        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        assert!(
            resolve_verification_dispatch_with_conn(
                &conn,
                &dispatch.id,
                "wrong-verifier",
                Some("vcap-a"),
                false,
            )
            .is_err()
        );
        let resolved = resolve_verification_dispatch_with_conn(
            &conn,
            &dispatch.id,
            "verifier-a",
            Some("vcap-a"),
            false,
        )
        .expect("resolve")
        .expect("dispatch");
        assert_eq!(resolved.state, VerificationDispatchState::Resolved);
        assert!(resolved.resolved_at.is_some());
    }

    #[test]
    fn concurrent_identical_dispatch_creation_is_cross_connection_idempotent() {
        use std::sync::{Arc, Barrier};

        let (_store, dir) = create_test_store();
        let event_schema = Connection::open(dir.path().join("cas.db")).expect("event schema db");
        event_schema
            .execute_batch(crate::event_store::EVENT_SCHEMA)
            .expect("event schema");
        drop(event_schema);
        let task_id = "cas-dispatch-concurrent";
        let boundary = VerificationProofBoundary::delivery(
            "wcr-dispatch-concurrent".to_string(),
            "wdt-dispatch-concurrent".to_string(),
        );
        let deadline = Utc::now() + Duration::minutes(10);
        let barrier = Arc::new(Barrier::new(16));

        let outcomes = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..16 {
                let db_path = dir.path().join("cas.db");
                let barrier = Arc::clone(&barrier);
                let boundary = boundary.clone();
                handles.push(scope.spawn(move || {
                    let conn = Connection::open(db_path).expect("independent sqlite connection");
                    conn.busy_timeout(std::time::Duration::from_secs(10))
                        .expect("busy timeout");
                    barrier.wait();
                    create_verification_dispatch_bound_with_conn(
                        &conn,
                        task_id,
                        "worker-a",
                        "supervisor-a",
                        &boundary,
                        deadline,
                        false,
                    )
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("dispatch thread must not panic"))
                .collect::<Vec<_>>()
        });

        let dispatches = outcomes
            .into_iter()
            .map(|outcome| outcome.expect("identical concurrent retry must converge"))
            .collect::<Vec<_>>();
        let dispatch_id = dispatches[0].id.clone();
        assert!(
            dispatches.iter().all(|dispatch| dispatch.id == dispatch_id),
            "all identical callers must receive the same durable dispatch"
        );

        let conn = Connection::open(dir.path().join("cas.db")).expect("inspect db");
        let counts = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM verification_dispatches WHERE task_id = ?1),
                    (SELECT COUNT(*) FROM verification_capabilities WHERE task_id = ?1),
                    (SELECT COUNT(*) FROM events WHERE entity_id = ?1)",
                params![task_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("durable dispatch boundary counts");
        assert_eq!(counts, (1, 0, 0));

        let conflicting = create_verification_dispatch_bound_with_conn(
            &conn,
            task_id,
            "worker-a",
            "other-supervisor",
            &boundary,
            deadline,
            false,
        )
        .expect_err("conflicting owner must remain rejected");
        assert!(
            conflicting
                .to_string()
                .contains("different verification proof boundary")
        );
        let counts_after = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM verification_dispatches WHERE task_id = ?1),
                    (SELECT COUNT(*) FROM verification_capabilities WHERE task_id = ?1),
                    (SELECT COUNT(*) FROM events WHERE entity_id = ?1)",
                params![task_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("post-conflict boundary counts");
        assert_eq!(counts_after, counts);
    }

    #[test]
    fn exact_proof_cycle_invalidation_cannot_touch_a_superseded_dispatch() {
        let (_store, dir) = create_test_store();
        let resolved = create_verification_dispatch(
            dir.path(),
            "cas-invalidate-race",
            "worker",
            "supervisor",
            Utc::now() + Duration::minutes(10),
        )
        .expect("resolved cycle");
        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        resolve_verification_dispatch_with_conn(&conn, &resolved.id, "supervisor", None, true)
            .expect("resolve cycle");
        drop(conn);
        let replacement = create_verification_dispatch(
            dir.path(),
            "cas-invalidate-race",
            "worker",
            "supervisor",
            Utc::now() + Duration::minutes(10),
        )
        .expect("newer pending cycle");

        let task_store = SqliteTaskStore::open(dir.path()).expect("task store");
        task_store.init().expect("task schema");
        let mut task = Task::new(
            "cas-invalidate-race".to_string(),
            "atomic scope recovery".to_string(),
        );
        task.status = TaskStatus::InProgress;
        task_store.add(&task).expect("task");
        let expected_task = task.clone();
        task.status = TaskStatus::Open;

        assert!(
            invalidate_verification_dispatch_and_reopen_task_exact(
                dir.path(),
                &resolved.id,
                &task,
                TaskStatus::InProgress,
            )
            .is_err(),
            "recovery naming an older proof must not invalidate or reopen through a newer cycle"
        );
        assert_eq!(
            task_store.get(&task.id).unwrap().status,
            expected_task.status,
            "failed exact invalidation must roll back the task transition"
        );
        assert_eq!(
            get_verification_dispatch(dir.path(), &resolved.id)
                .unwrap()
                .state,
            VerificationDispatchState::Resolved
        );
        assert_eq!(
            get_verification_dispatch(dir.path(), &replacement.id)
                .unwrap()
                .state,
            VerificationDispatchState::Pending
        );
    }

    #[test]
    fn supervisor_review_transition_rolls_back_dispatch_on_status_mismatch() {
        let (_store, dir) = create_test_store();
        let task_store = SqliteTaskStore::open(dir.path()).expect("task store");
        task_store.init().expect("task schema");
        let task = Task::new(
            "cas-supervisor-review-race".to_string(),
            "atomic pending review".to_string(),
        );
        task_store.add(&task).expect("task");

        let mut pending = task.clone();
        pending.status = TaskStatus::PendingSupervisorReview;
        pending.pending_verification = true;
        assert!(
            pend_task_for_supervisor_review_with_dispatch(
                dir.path(),
                &pending,
                TaskStatus::InProgress,
                "worker",
                "supervisor",
                Utc::now() + Duration::minutes(10),
            )
            .is_err(),
            "a stale status expectation must reject the whole transition"
        );

        assert_eq!(task_store.get(&task.id).unwrap().status, TaskStatus::Open);
        assert!(
            get_latest_verification_dispatch(dir.path(), &task.id)
                .expect("dispatch lookup")
                .is_none(),
            "failed transition must not leave an orphan dispatch"
        );
    }

    #[test]
    fn verification_dispatch_corruption_fails_loudly() {
        let (_store, dir) = create_test_store();
        let deadline = Utc::now() + Duration::minutes(10);
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-corrupt",
            "worker",
            "supervisor",
            deadline,
        )
        .expect("create");
        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        conn.execute(
            "UPDATE verification_dispatches SET state = 'mystery' WHERE id = ?1",
            params![dispatch.id],
        )
        .expect("corrupt state");
        assert!(
            get_latest_verification_dispatch(dir.path(), "cas-corrupt").is_err(),
            "invalid durable state must not default to pending"
        );

        conn.execute(
            "UPDATE verification_dispatches
             SET state = 'pending', recovery_action = 'trust_caller'
             WHERE id = ?1",
            params![dispatch.id],
        )
        .expect("corrupt recovery");
        assert!(
            get_latest_verification_dispatch(dir.path(), "cas-corrupt").is_err(),
            "invalid durable recovery metadata must not default to an authorized path"
        );
    }

    #[test]
    fn registered_supervisor_override_is_explicit_and_worker_cannot_steal_dispatch() {
        let (_store, dir) = create_test_store();
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-recovery",
            "worker",
            "unavailable-owner",
            Utc::now() - Duration::minutes(1),
        )
        .expect("create");
        timeout_verification_dispatch(dir.path(), "cas-recovery", Utc::now())
            .expect("persist exact timeout");
        let conn = Connection::open(dir.path().join("cas.db")).expect("db");
        assert!(
            resolve_verification_dispatch_with_conn(
                &conn,
                &dispatch.id,
                "unavailable-owner",
                None,
                false,
            )
            .is_err(),
            "the original owner has no capability-free authority unless the server derives registered-supervisor-direct"
        );
        assert!(
            resolve_verification_dispatch_with_conn(&conn, &dispatch.id, "worker", None, false)
                .is_err(),
            "ordinary workers cannot steal a dispatch by omitting a capability"
        );
        let recovered = resolve_verification_dispatch_with_conn(
            &conn,
            &dispatch.id,
            "registered-supervisor",
            None,
            true,
        )
        .expect("supervisor recovery")
        .expect("dispatch");
        assert_eq!(recovered.state, VerificationDispatchState::Resolved);
    }

    #[test]
    fn verification_dispatch_timeout_is_exact_task_only() {
        let (_store, dir) = create_test_store();
        let past = Utc::now() - Duration::minutes(1);
        let future = Utc::now() + Duration::minutes(10);
        create_verification_dispatch(
            dir.path(),
            "cas-timeout-a",
            "worker-a",
            "supervisor-a",
            past,
        )
        .expect("create due dispatch");
        create_verification_dispatch(
            dir.path(),
            "cas-timeout-b",
            "worker-b",
            "supervisor-b",
            future,
        )
        .expect("create live dispatch");

        let timed_out = timeout_verification_dispatch(dir.path(), "cas-timeout-a", Utc::now())
            .expect("timeout")
            .expect("due dispatch");
        assert_eq!(timed_out.state, VerificationDispatchState::TimedOut);
        assert!(
            timeout_verification_dispatch(dir.path(), "cas-timeout-b", Utc::now())
                .expect("not due")
                .is_none()
        );
        assert_eq!(
            get_latest_verification_dispatch(dir.path(), "cas-timeout-b")
                .expect("load")
                .expect("dispatch")
                .state,
            VerificationDispatchState::Pending
        );
    }

    #[test]
    fn verifier_authored_payload_is_sanitized_at_every_write_boundary() {
        let (store, dir) = create_test_store();
        let supervisor_id = "supervisor-private";
        {
            let conn = store.conn.lock().expect("db");
            conn.execute_batch(crate::agent_store::AGENT_SCHEMA)
                .expect("agent schema");
            let mut supervisor = Agent::new(supervisor_id.to_string(), supervisor_id.to_string());
            supervisor.role = AgentRole::Supervisor;
            register_agent_with_conn(&conn, &supervisor).expect("register supervisor");
        }
        let dispatch = create_verification_dispatch(
            dir.path(),
            "cas-private",
            "worker-private",
            supervisor_id,
            Utc::now() + Duration::minutes(10),
        )
        .expect("dispatch");
        let raw_capability =
            "vcap-fedcba9876543210fedcba9876543210.abcdef0123456789abcdef0123456789";
        let raw_path = "/home/verifier/private-proof.json";
        let raw_secret = "ghp_verifier-authored-secret";
        let raw_control = "\u{1b}[31mverifier-control";
        let mut verification = Verification::rejected(
            "ver-private".to_string(),
            "cas-private".to_string(),
            format!("Approved using {raw_capability}"),
            vec![VerificationIssue::blocking(
                raw_path.to_string(),
                Some(12),
                "security".to_string(),
                raw_secret.to_string(),
                raw_control.to_string(),
                Some("password=verifier-secret".to_string()),
            )],
        );
        verification.provenance = VerificationProvenance::SupervisorDirect;
        verification.agent_id = Some(supervisor_id.to_string());
        verification.issuer_agent_id = Some(supervisor_id.to_string());
        verification.dispatch_id = Some(dispatch.id);
        verification.files_reviewed = vec![raw_path.to_string(), "src/lib.rs".to_string()];

        store.add(&verification).expect("add unsafe verification");

        let row = store.get("ver-private").expect("read sanitized row");
        let serialized = serde_json::to_string(&row).expect("serialize row");
        assert!(serialized.contains("[REDACTED_SECRET]"));
        assert!(serialized.contains("[REDACTED_PATH]"));
        assert!(serialized.contains("[REDACTED_CONTROL]"));
        assert!(serialized.contains("src/lib.rs"));

        let conn = store.conn.lock().expect("db");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let path = dir.path().join(format!("cas.db{suffix}"));
            if let Ok(bytes) = std::fs::read(path) {
                for unsafe_value in [
                    raw_capability,
                    raw_path,
                    raw_secret,
                    raw_control,
                    "verifier-secret",
                ] {
                    assert!(
                        !bytes
                            .windows(unsafe_value.len())
                            .any(|window| window == unsafe_value.as_bytes()),
                        "unsafe verifier content reached SQLite {suffix}: {unsafe_value:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn legacy_verification_rows_remain_readable_without_new_authority() {
        let (store, _dir) = create_test_store();
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO verifications
             (id, task_id, agent_id, verification_type, status, confidence, summary,
              files_reviewed, duration_ms, created_at)
             VALUES (?1, ?2, NULL, 'task', 'approved', NULL, ?3, '[]', NULL, ?4)",
            params![
                "ver-legacy",
                "cas-legacy",
                "pre-provenance row",
                Utc::now().to_rfc3339()
            ],
        )
        .expect("insert legacy-shaped row");
        drop(conn);

        let row = store.get("ver-legacy").expect("legacy row readable");
        assert_eq!(row.provenance, VerificationProvenance::Legacy);
        assert!(row.capability_id.is_none());
        assert!(row.issuer_agent_id.is_none());
    }
}
