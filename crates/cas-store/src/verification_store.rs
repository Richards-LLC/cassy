//! Verification storage for task quality gates
//!
//! Stores verification results in SQLite. Verifications are created when
//! attempting to close a task, with a Haiku subagent reviewing the work.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{
    IssueSeverity, Verification, VerificationDispatch, VerificationDispatchState,
    VerificationIssue, VerificationProofBoundary, VerificationProvenance,
    VerificationRecoveryAction, VerificationStatus, VerificationType, VerifierCapability,
};

// Helper to convert lock errors
fn lock_err<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Parse("Failed to acquire lock".to_string())
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

CREATE TABLE IF NOT EXISTS verification_dispatches (
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

CREATE INDEX IF NOT EXISTS idx_verifications_task ON verifications(task_id);
CREATE INDEX IF NOT EXISTS idx_verifications_status ON verifications(status);
CREATE INDEX IF NOT EXISTS idx_verifications_created ON verifications(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_verification_issues_verification ON verification_issues(verification_id);
CREATE INDEX IF NOT EXISTS idx_verification_capabilities_task
    ON verification_capabilities(task_id, consumed_at, expires_at);
CREATE INDEX IF NOT EXISTS idx_verification_dispatches_task
    ON verification_dispatches(task_id, requested_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_verification_dispatches_active_task
    ON verification_dispatches(task_id)
    WHERE state IN ('pending', 'claimed');
"#;

const VERIFIER_CAPABILITY_TTL_MINUTES: i64 = 30;

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
pub fn add_verification_with_conn(conn: &Connection, verification: &Verification) -> Result<()> {
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
    save_verification_issues_with_conn(conn, verification)?;

    Ok(())
}

/// Save verification issues using an existing connection (for cross-store transactions).
pub fn save_verification_issues_with_conn(
    conn: &Connection,
    verification: &Verification,
) -> Result<()> {
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
    let state_value: String = row.get(8)?;
    let state = VerificationDispatchState::from_str(&state_value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let recovery_value: String = row.get(12)?;
    let recovery_action =
        VerificationRecoveryAction::from_str(&recovery_value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(VerificationDispatch {
        id: row.get(0)?,
        task_id: row.get(1)?,
        receipt_id: row.get(2)?,
        delivery_transaction_id: row.get(3)?,
        requester_agent_id: row.get(4)?,
        owner_agent_id: row.get(5)?,
        verifier_agent_id: row.get(6)?,
        capability_id: row.get(7)?,
        state,
        requested_at: parse_time(9)?,
        deadline_at: parse_time(10)?,
        resolved_at: parse_optional_time(11)?,
        recovery_action,
    })
}

pub fn get_latest_verification_dispatch_with_conn(
    conn: &Connection,
    task_id: &str,
) -> Result<Option<VerificationDispatch>> {
    conn.query_row(
        "SELECT id, task_id, receipt_id, delivery_transaction_id, requester_agent_id,
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
        "SELECT id, task_id, receipt_id, delivery_transaction_id, requester_agent_id,
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

/// Create an exact dispatch on a caller-owned SQLite transaction.
///
/// This is used to make delivery receipt, transaction, and proof-boundary
/// intent one atomic persistence step.
pub fn create_verification_dispatch_bound_with_conn(
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
         (id, task_id, receipt_id, delivery_transaction_id, requester_agent_id,
          owner_agent_id, verifier_agent_id, capability_id, state, requested_at,
          deadline_at, resolved_at, recovery_action)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?8, ?9, NULL, ?10)",
        params![
            dispatch.id,
            dispatch.task_id,
            dispatch.receipt_id,
            dispatch.delivery_transaction_id,
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
    let changed = conn.execute(
        "UPDATE verification_dispatches
         SET state = 'timed_out', resolved_at = ?2
         WHERE id = (
             SELECT id FROM verification_dispatches
             WHERE task_id = ?1 AND state IN ('pending', 'claimed')
                   AND deadline_at <= ?2
             ORDER BY requested_at DESC, id DESC
             LIMIT 1
         )",
        params![task_id, now.to_rfc3339()],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    get_latest_verification_dispatch_with_conn(&conn, task_id)
}

/// Invalidate a reusable resolved proof cycle when task rework begins.
pub fn invalidate_verification_dispatch_for_new_cycle(
    cas_dir: &Path,
    task_id: &str,
) -> Result<Option<VerificationDispatch>> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    let conn = store.conn.lock().map_err(lock_err)?;
    let Some(dispatch) = get_latest_verification_dispatch_with_conn(&conn, task_id)? else {
        return Ok(None);
    };
    if !matches!(
        dispatch.state,
        VerificationDispatchState::Resolved | VerificationDispatchState::TimedOut
    ) {
        return Ok(Some(dispatch));
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
    get_latest_verification_dispatch_with_conn(&conn, task_id)
}

/// Mint an unbound, one-time verifier capability.
///
/// The caller must deliver `token` only to the child task-verifier. The
/// database stores its domain-separated SHA-256 digest, never the raw value.
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
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute_batch(VERIFICATION_SCHEMA)?;
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

        let files_reviewed_json = serde_json::to_string(&verification.files_reviewed)
            .unwrap_or_else(|_| "[]".to_string());

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

        // Save issues
        self.save_issues(&conn, verification)?;

        Ok(())
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
        self.save_issues(&conn, verification)?;

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
    use tempfile::TempDir;

    fn create_test_store() -> (SqliteVerificationStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = SqliteVerificationStore::open(dir.path()).unwrap();
        (store, dir)
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
