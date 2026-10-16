//! Request journal and v1 client for Slack-thread authority.
//!
//! The task/epic lease client has different visibility and outage semantics.
//! This module keeps thread ownership on a separate, fail-closed contract.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const CONTRACT_SCHEMA: &str = "cassy.coordination/v1";
pub const TOTAL_REMOTE_BUDGET: Duration = Duration::from_secs(8);
pub const CALL_REMOTE_BUDGET: Duration = Duration::from_secs(5);
pub const RECONCILE_BATCH_LIMIT: usize = 8;

pub const EXTERNAL_WORK_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS external_work_operations (
    request_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    input_json TEXT NOT NULL,
    wire_json TEXT,
    stage TEXT NOT NULL,
    response_json TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_ms INTEGER,
    lease_token TEXT,
    lease_until_ms INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_external_work_due
    ON external_work_operations(stage, next_retry_ms, lease_until_ms);
CREATE TRIGGER IF NOT EXISTS external_work_request_immutable
BEFORE UPDATE OF request_id, kind, input_digest, input_json ON external_work_operations
BEGIN SELECT RAISE(ABORT, 'external work request identity is immutable'); END;
CREATE TRIGGER IF NOT EXISTS external_work_wire_immutable
BEFORE UPDATE OF wire_json ON external_work_operations
WHEN OLD.wire_json IS NOT NULL AND NEW.wire_json IS NOT OLD.wire_json
BEGIN SELECT RAISE(ABORT, 'external work wire request is immutable'); END;
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalOperation {
    pub request_id: String,
    pub kind: String,
    pub input_digest: String,
    pub input_json: String,
    pub wire_json: Option<String>,
    pub stage: String,
    pub response_json: Option<String>,
    pub attempts: u32,
    pub next_retry_ms: Option<i64>,
}

#[derive(Clone)]
pub struct SqliteRequestJournal {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalError {
    Storage(String),
    IdempotencyConflict { request_id: String },
    Missing(String),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "external-work journal failed: {message}"),
            Self::IdempotencyConflict { request_id } => write!(
                f,
                "request_id {request_id} already has different immutable input"
            ),
            Self::Missing(request_id) => write!(f, "no local operation for request_id {request_id}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl SqliteRequestJournal {
    pub fn open(cas_dir: &Path) -> Result<Self, JournalError> {
        let connection = cas_store::shared_db::shared_connection(&cas_dir.join("cas.db"))
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        let journal = Self { conn: connection };
        journal.init()?;
        Ok(journal)
    }

    pub fn open_connection(connection: Connection) -> Result<Self, JournalError> {
        let journal = Self {
            conn: Arc::new(Mutex::new(connection)),
        };
        journal.init()?;
        Ok(journal)
    }

    pub fn init(&self) -> Result<(), JournalError> {
        self.conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?
            .execute_batch(EXTERNAL_WORK_SCHEMA)
            .map_err(|error| JournalError::Storage(error.to_string()))
    }

    /// Persist the immutable request before any provider call. Reusing a
    /// request ID with changed input is rejected locally.
    pub fn prepare(
        &self,
        request_id: &str,
        kind: &str,
        input: &Value,
    ) -> Result<(JournalOperation, bool), JournalError> {
        let input_json = canonical_json(input)?;
        let digest = digest(&input_json);
        let now = Utc::now().to_rfc3339();
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let transaction = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        let existing = load_operation(&transaction, request_id)?;
        let created = if let Some(existing) = existing {
            if existing.kind != kind || existing.input_digest != digest {
                return Err(JournalError::IdempotencyConflict {
                    request_id: request_id.to_string(),
                });
            }
            false
        } else {
            transaction
                .execute(
                    "INSERT INTO external_work_operations
                     (request_id, kind, input_digest, input_json, stage, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, ?5)",
                    params![request_id, kind, digest, input_json, now],
                )
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            true
        };
        let operation = load_operation(&transaction, request_id)?.ok_or_else(|| {
            JournalError::Storage("prepared operation disappeared in transaction".into())
        })?;
        transaction
            .commit()
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        Ok((operation, created))
    }

    /// Freeze the actual provider body before dispatch. It can be set once;
    /// later retries always use the original body and request ID.
    pub fn set_wire_request(
        &self,
        request_id: &str,
        wire: &Value,
    ) -> Result<(), JournalError> {
        let wire_json = canonical_json(wire)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let changed = conn
            .execute(
                "UPDATE external_work_operations SET wire_json = ?2, stage = 'ready', updated_at = ?3
                 WHERE request_id = ?1 AND (wire_json IS NULL OR wire_json = ?2)",
                params![request_id, wire_json, Utc::now().to_rfc3339()],
            )
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        if changed == 0 {
            let row = load_operation(&*conn, request_id)?;
            return match row {
                Some(_) => Err(JournalError::Storage(
                    "attempted to change frozen provider request".into(),
                )),
                None => Err(JournalError::Missing(request_id.to_string())),
            };
        }
        Ok(())
    }

    pub fn begin_attempt(&self, request_id: &str) -> Result<(), JournalError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let changed = conn
            .execute(
                "UPDATE external_work_operations SET stage='dispatching', attempts=attempts+1,
                 updated_at=?2 WHERE request_id=?1 AND wire_json IS NOT NULL",
                params![request_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        if changed == 0 {
            return Err(JournalError::Missing(request_id.to_string()));
        }
        Ok(())
    }

    pub fn record_result(
        &self,
        request_id: &str,
        response: &Value,
        retry_after: Option<Duration>,
    ) -> Result<(), JournalError> {
        let response_json = canonical_json(response)?;
        let code = response.get("code").and_then(Value::as_str).unwrap_or_default();
        let retry = matches!(code, "pending" | "outcome_unknown" | "authority_unavailable");
        let stage = if retry { "retry_wait" } else { "terminal" };
        let retry_at = retry.then(|| {
            Utc::now().timestamp_millis()
                + retry_after.unwrap_or(Duration::from_secs(2)).as_millis() as i64
        });
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let changed = conn
            .execute(
                "UPDATE external_work_operations SET stage=?2, response_json=?3,
                 next_retry_ms=?4, lease_token=NULL, lease_until_ms=NULL, updated_at=?5
                 WHERE request_id=?1",
                params![request_id, stage, response_json, retry_at, Utc::now().to_rfc3339()],
            )
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        if changed == 0 {
            return Err(JournalError::Missing(request_id.to_string()));
        }
        Ok(())
    }

    pub fn get(&self, request_id: &str) -> Result<Option<JournalOperation>, JournalError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        load_operation(&*conn, request_id)
    }

    /// Atomically lease a bounded batch of due reconciliation operations.
    pub fn lease_due(
        &self,
        limit: usize,
        now_ms: i64,
        lease_for: Duration,
    ) -> Result<Vec<JournalOperation>, JournalError> {
        let limit = limit.clamp(1, RECONCILE_BATCH_LIMIT);
        let token = Uuid::new_v4().to_string();
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let transaction = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        let request_ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT request_id FROM external_work_operations
                     WHERE stage IN ('prepared','retry_wait','dispatching')
                       AND (next_retry_ms IS NULL OR next_retry_ms <= ?1)
                       AND (lease_until_ms IS NULL OR lease_until_ms <= ?1)
                     ORDER BY COALESCE(next_retry_ms, 0), created_at LIMIT ?2",
                )
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            statement
                .query_map(params![now_ms, limit as i64], |row| row.get::<_, String>(0))
                .map_err(|error| JournalError::Storage(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| JournalError::Storage(error.to_string()))?
        };
        let until = now_ms + lease_for.as_millis() as i64;
        let mut rows = Vec::with_capacity(request_ids.len());
        for request_id in request_ids {
            transaction
                .execute(
                    "UPDATE external_work_operations SET lease_token=?2, lease_until_ms=?3
                     WHERE request_id=?1",
                    params![request_id, token, until],
                )
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            if let Some(row) = load_operation(&transaction, &request_id)? {
                rows.push(row);
            }
        }
        transaction
            .commit()
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        Ok(rows)
    }

    pub fn clear_lease(&self, request_id: &str) -> Result<(), JournalError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        conn.execute(
            "UPDATE external_work_operations SET lease_token=NULL, lease_until_ms=NULL
             WHERE request_id=?1",
            [request_id],
        )
        .map_err(|error| JournalError::Storage(error.to_string()))?;
        Ok(())
    }
}

fn load_operation(
    conn: &Connection,
    request_id: &str,
) -> Result<Option<JournalOperation>, JournalError> {
    conn.query_row(
        "SELECT request_id, kind, input_digest, input_json, wire_json, stage,
                response_json, attempts, next_retry_ms
         FROM external_work_operations WHERE request_id=?1",
        [request_id],
        |row| {
            Ok(JournalOperation {
                request_id: row.get(0)?,
                kind: row.get(1)?,
                input_digest: row.get(2)?,
                input_json: row.get(3)?,
                wire_json: row.get(4)?,
                stage: row.get(5)?,
                response_json: row.get(6)?,
                attempts: row.get(7)?,
                next_retry_ms: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(|error| JournalError::Storage(error.to_string()))
}

fn canonical_json(value: &Value) -> Result<String, JournalError> {
    serde_json::to_string(value).map_err(|error| JournalError::Storage(error.to_string()))
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    pub message: String,
    pub timed_out: bool,
}

impl RemoteError {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self { message: message.into(), timed_out: false }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self { message: message.into(), timed_out: true }
    }
}

pub trait CloudCoordinationPort: Send + Sync {
    fn post(&self, path: &str, payload: &Value, timeout: Duration) -> Result<Value, RemoteError>;
    fn lookup(
        &self,
        request_id: &str,
        registration_id: &str,
        timeout: Duration,
    ) -> Result<Value, RemoteError>;
}

pub trait VioletThreadResolver: Send + Sync {
    /// Resolve only through Violet's authenticated `violet_read` contract.
    fn resolve(&self, external_ref: &str, timeout: Duration) -> Result<Value, RemoteError>;
}

#[derive(Clone)]
pub struct HttpCloudCoordinationPort {
    endpoint: String,
    token: String,
}

impl HttpCloudCoordinationPort {
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into().trim_end_matches('/').to_string(), token: token.into() }
    }

    fn decode_response(response: ureq::Response) -> Result<Value, RemoteError> {
        response
            .into_json()
            .map_err(|error| RemoteError::unavailable(format!("invalid Cloud response: {error}")))
    }
}

impl CloudCoordinationPort for HttpCloudCoordinationPort {
    fn post(&self, path: &str, payload: &Value, timeout: Duration) -> Result<Value, RemoteError> {
        let url = format!("{}{}", self.endpoint, path);
        let response = ureq::post(&url)
            .timeout(timeout)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(payload);
        match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => Self::decode_response(response),
            Err(ureq::Error::Transport(error)) => {
                let message = error.to_string();
                let timed_out = message.to_ascii_lowercase().contains("timed out");
                Err(RemoteError { message, timed_out })
            }
        }
    }

    fn lookup(
        &self,
        request_id: &str,
        registration_id: &str,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        let url = format!("{}/api/coordination/operations/{request_id}", self.endpoint);
        let response = ureq::get(&url)
            .timeout(timeout)
            .query("registration_id", registration_id)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call();
        match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => Self::decode_response(response),
            Err(ureq::Error::Transport(error)) => {
                let message = error.to_string();
                let timed_out = message.to_ascii_lowercase().contains("timed out");
                Err(RemoteError { message, timed_out })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct OperationContext {
    pub project_id: String,
    /// The authenticated Cloud registration for this active Cassy session.
    pub registration_id: String,
}

#[derive(Debug, Clone)]
pub enum ExternalWorkCommand {
    Claim { external_ref: String, request_id: Option<String> },
    ClaimStatus { request_id: String },
    ReleaseClaim { resource_id: String, expected_generation: u64, request_id: Option<String> },
    TransferClaim {
        resource_id: String,
        expected_generation: u64,
        to_slot_id: String,
        offer_ttl_secs: Option<u32>,
        request_id: Option<String>,
    },
    AcceptClaim { handoff_id: String, expected_generation: u64, request_id: Option<String> },
}

#[derive(Debug, Clone)]
pub struct ExternalWorkError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for ExternalWorkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ExternalWorkError {}

#[derive(Clone)]
pub struct ExternalWorkService {
    pub journal: SqliteRequestJournal,
    pub cloud: Arc<dyn CloudCoordinationPort>,
    pub violet: Arc<dyn VioletThreadResolver>,
    pub context: OperationContext,
    pub total_budget: Duration,
}

impl ExternalWorkService {
    pub fn new(
        journal: SqliteRequestJournal,
        cloud: Arc<dyn CloudCoordinationPort>,
        violet: Arc<dyn VioletThreadResolver>,
        context: OperationContext,
    ) -> Self {
        Self { journal, cloud, violet, context, total_budget: TOTAL_REMOTE_BUDGET }
    }

    pub fn execute(&self, command: ExternalWorkCommand) -> Result<Value, ExternalWorkError> {
        match command {
            ExternalWorkCommand::ClaimStatus { request_id } => self.claim_status(&request_id),
            other => self.mutate(other),
        }
    }

    fn mutate(&self, command: ExternalWorkCommand) -> Result<Value, ExternalWorkError> {
        let kind = command.kind().to_string();
        let request_id = valid_request_id(command.request_id())?;
        let input = command.input(&self.context);
        let (mut row, created) = self
            .journal
            .prepare(&request_id, &kind, &input)
            .map_err(journal_error)?;
        if !created {
            if row.stage == "terminal" {
                return row_response(&row);
            }
            if row.stage == "dispatching" || row.stage == "retry_wait" && row.wire_json.is_some() {
                return self.reconcile(&row);
            }
        }

        let deadline = Instant::now() + self.total_budget;
        if kind == "claim" && row.wire_json.is_none() {
            let external_ref = command.claim_external_ref().ok_or_else(|| error("invalid_request", "claim requires external_ref"))?;
            let timeout = call_timeout(deadline)?;
            let resolved = match self.violet.resolve(external_ref, timeout) {
                Ok(value) => value,
                Err(error) => {
                    let result = failure_response(
                        &request_id,
                        if error.timed_out { "authority_unavailable" } else { "authority_unavailable" },
                        "violet_unavailable",
                        &error.message,
                        "retry_later",
                    );
                    self.journal.record_result(&request_id, &result, Some(Duration::from_secs(2))).map_err(journal_error)?;
                    return Ok(result);
                }
            };
            let code = resolved.get("code").and_then(Value::as_str).unwrap_or_default();
            if code != "completed" {
                let mut refused = resolved;
                ensure_local_request_receipt(&mut refused, &request_id);
                self.journal.record_result(&request_id, &refused, None).map_err(journal_error)?;
                return Ok(refused);
            }
            let identity = resolved.get("thread_identity").ok_or_else(|| error("invalid_response", "Violet response omitted verified thread identity"))?;
            let identity_fields = ["provider", "workspace_id", "channel_id", "root_ts"];
            if identity_fields.iter().any(|field| identity.get(*field).and_then(Value::as_str).is_none()) {
                return Err(error("invalid_response", "Violet response has incomplete verified thread identity"));
            }
            // v1's Cloud fixture binds the verified Slack channel to the
            // authenticated project but has no identity_proof body field.
            let cloud_identity = json!({
                "provider": identity["provider"],
                "workspace_id": identity["workspace_id"],
                "channel_id": identity["channel_id"],
                "root_ts": identity["root_ts"],
            });
            let wire = json!({
                "schema": CONTRACT_SCHEMA,
                "request_id": request_id,
                "project_id": self.context.project_id,
                "external_ref": cloud_identity,
                "actor": {"registration_id": self.context.registration_id},
            });
            self.journal.set_wire_request(&request_id, &wire).map_err(journal_error)?;
            row = self.journal.get(&request_id).map_err(journal_error)?.ok_or_else(|| error("journal_error", "prepared operation disappeared"))?;
        } else if row.wire_json.is_none() {
            let wire = command.wire(&request_id, &self.context);
            self.journal.set_wire_request(&request_id, &wire).map_err(journal_error)?;
            row = self.journal.get(&request_id).map_err(journal_error)?.ok_or_else(|| error("journal_error", "prepared operation disappeared"))?;
        }

        self.dispatch(&row, &command, deadline)
    }

    fn dispatch(
        &self,
        row: &JournalOperation,
        command: &ExternalWorkCommand,
        deadline: Instant,
    ) -> Result<Value, ExternalWorkError> {
        let path = command.path();
        let payload: Value = serde_json::from_str(row.wire_json.as_deref().ok_or_else(|| error("journal_error", "provider request was not journaled"))?)
            .map_err(|e| error("journal_error", format!("saved provider request is invalid: {e}")))?;
        let timeout = call_timeout(deadline)?;
        self.journal.begin_attempt(&row.request_id).map_err(journal_error)?;
        let result = match self.cloud.post(&path, &payload, timeout) {
            Ok(mut response) => {
                if response.get("code").and_then(Value::as_str).is_none() {
                    failure_response(&row.request_id, "outcome_unknown", "malformed_response", "Cloud returned no stable operation code; reconcile the original request ID.", "reconcile")
                } else {
                    ensure_local_request_receipt(&mut response, &row.request_id);
                    response
                }
            }
            Err(error) => failure_response(
                &row.request_id,
                "outcome_unknown",
                if error.timed_out { "request_timeout" } else { "cloud_unavailable" },
                &error.message,
                "reconcile",
            ),
        };
        let retry = matches!(result["code"].as_str(), Some("pending" | "outcome_unknown" | "authority_unavailable"));
        self.journal.record_result(&row.request_id, &result, retry.then_some(Duration::from_secs(2))).map_err(journal_error)?;
        Ok(result)
    }

    pub fn claim_status(&self, request_id: &str) -> Result<Value, ExternalWorkError> {
        let request_id = valid_uuid(request_id)?;
        let Some(row) = self.journal.get(&request_id).map_err(journal_error)? else {
            let deadline = Instant::now() + self.total_budget;
            let timeout = call_timeout(deadline)?;
            return self.cloud.lookup(&request_id, &self.context.registration_id, timeout)
                .map_err(|e| error("authority_unavailable", e.message));
        };
        let input: Value = serde_json::from_str(&row.input_json).map_err(|e| error("journal_error", e.to_string()))?;
        if input.pointer("/actor/registration_id").and_then(Value::as_str) != Some(self.context.registration_id.as_str()) {
            return Err(error("not_authorized", "operation belongs to a different registered supervisor"));
        }
        if row.stage == "terminal" {
            return row_response(&row);
        }
        if row.kind == "claim" && row.wire_json.is_none() {
            let ref_link = input.get("external_ref").and_then(Value::as_str).ok_or_else(|| error("journal_error", "claim permalink missing from journal"))?;
            let timeout = call_timeout(Instant::now() + self.total_budget)?;
            let resolved = self.violet.resolve(ref_link, timeout).map_err(|e| error("authority_unavailable", e.message))?;
            let identity = resolved.get("thread_identity").ok_or_else(|| error("invalid_response", "Violet response omitted verified thread identity"))?;
            let wire = json!({
                "schema": CONTRACT_SCHEMA,
                "request_id": request_id,
                "project_id": self.context.project_id,
                "external_ref": {
                    "provider": identity["provider"], "workspace_id": identity["workspace_id"],
                    "channel_id": identity["channel_id"], "root_ts": identity["root_ts"]
                },
                "actor": {"registration_id": self.context.registration_id},
            });
            self.journal.set_wire_request(&request_id, &wire).map_err(journal_error)?;
        }
        self.reconcile(&row)
    }

    fn reconcile(&self, row: &JournalOperation) -> Result<Value, ExternalWorkError> {
        let deadline = Instant::now() + self.total_budget;
        let lookup = self.cloud.lookup(&row.request_id, &self.context.registration_id, call_timeout(deadline)?);
        let result = match lookup {
            Ok(mut response) => {
                ensure_local_request_receipt(&mut response, &row.request_id);
                response
            }
            Err(error) => failure_response(
                &row.request_id,
                "outcome_unknown",
                if error.timed_out { "lookup_timeout" } else { "cloud_unavailable" },
                &error.message,
                "reconcile",
            ),
        };
        let retry = matches!(result["code"].as_str(), Some("pending" | "outcome_unknown" | "authority_unavailable"));
        self.journal.record_result(&row.request_id, &result, retry.then_some(Duration::from_secs(2))).map_err(journal_error)?;
        Ok(result)
    }

    /// One bounded background batch. Callers submit this on an external job
    /// lane; the loop and hooks never call it synchronously.
    pub fn reconcile_due(&self, now_ms: i64) -> Result<Vec<Value>, ExternalWorkError> {
        let rows = self.journal.lease_due(RECONCILE_BATCH_LIMIT, now_ms, Duration::from_secs(15)).map_err(journal_error)?;
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let result = self.reconcile(&row)?;
            self.journal.clear_lease(&row.request_id).map_err(journal_error)?;
            results.push(result);
        }
        Ok(results)
    }
}

impl ExternalWorkCommand {
    fn kind(&self) -> &'static str {
        match self {
            Self::Claim { .. } => "claim",
            Self::ClaimStatus { .. } => "claim_status",
            Self::ReleaseClaim { .. } => "release_claim",
            Self::TransferClaim { .. } => "transfer_claim",
            Self::AcceptClaim { .. } => "accept_claim",
        }
    }

    fn request_id(&self) -> Option<&str> {
        match self {
            Self::Claim { request_id, .. }
            | Self::ReleaseClaim { request_id, .. }
            | Self::TransferClaim { request_id, .. }
            | Self::AcceptClaim { request_id, .. } => request_id.as_deref(),
            Self::ClaimStatus { request_id } => Some(request_id),
        }
    }

    fn input(&self, context: &OperationContext) -> Value {
        let mut input = match self {
            Self::Claim { external_ref, .. } => json!({"external_ref": external_ref}),
            Self::ClaimStatus { request_id } => json!({"request_id": request_id}),
            Self::ReleaseClaim { resource_id, expected_generation, .. } => json!({"resource_id": resource_id, "expected_generation": expected_generation}),
            Self::TransferClaim { resource_id, expected_generation, to_slot_id, offer_ttl_secs, .. } => json!({"resource_id": resource_id, "expected_generation": expected_generation, "to_slot_id": to_slot_id, "offer_ttl_secs": offer_ttl_secs.unwrap_or(600)}),
            Self::AcceptClaim { handoff_id, expected_generation, .. } => json!({"handoff_id": handoff_id, "expected_generation": expected_generation}),
        };
        input["project_id"] = json!(context.project_id);
        input["actor"] = json!({"registration_id": context.registration_id});
        input["schema"] = json!(CONTRACT_SCHEMA);
        input
    }

    fn wire(&self, request_id: &str, context: &OperationContext) -> Value {
        let mut value = match self {
            Self::Claim { .. } | Self::ClaimStatus { .. } => json!({}),
            Self::ReleaseClaim { expected_generation, .. }
            | Self::TransferClaim { expected_generation, .. }
            | Self::AcceptClaim { expected_generation, .. } => json!({"expected_generation": expected_generation}),
        };
        value["schema"] = json!(CONTRACT_SCHEMA);
        value["request_id"] = json!(request_id);
        value["actor"] = json!({"registration_id": context.registration_id});
        match self {
            Self::Claim { .. } | Self::ClaimStatus { .. } => {}
            Self::ReleaseClaim { .. } => {}
            Self::TransferClaim { to_slot_id, offer_ttl_secs, .. } => {
                value["to_slot_id"] = json!(to_slot_id);
                value["offer_ttl_secs"] = json!(offer_ttl_secs.unwrap_or(600));
            }
            Self::AcceptClaim { .. } => {}
        }
        value
    }

    fn path(&self) -> String {
        match self {
            Self::Claim { .. } | Self::ClaimStatus { .. } => "/api/coordination/external-work/claim".to_string(),
            Self::ReleaseClaim { resource_id, .. } => format!("/api/coordination/external-work/{resource_id}/release"),
            Self::TransferClaim { resource_id, .. } => format!("/api/coordination/external-work/{resource_id}/handoffs"),
            Self::AcceptClaim { handoff_id, .. } => format!("/api/coordination/handoffs/{handoff_id}/accept"),
        }
    }

    fn claim_external_ref(&self) -> Option<&str> {
        match self { Self::Claim { external_ref, .. } => Some(external_ref), _ => None }
    }
}

fn valid_request_id(candidate: Option<&str>) -> Result<String, ExternalWorkError> {
    match candidate {
        Some(id) => valid_uuid(id),
        None => Ok(Uuid::new_v4().to_string()),
    }
}

fn valid_uuid(value: &str) -> Result<String, ExternalWorkError> {
    let parsed = Uuid::parse_str(value).map_err(|_| error("invalid_request", "request_id must be a UUID"))?;
    let canonical = parsed.to_string();
    if value != canonical {
        return Err(error("invalid_request", "request_id must be a lowercase canonical UUID"));
    }
    Ok(canonical)
}

fn call_timeout(deadline: Instant) -> Result<Duration, ExternalWorkError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() { return Err(error("authority_unavailable", "remote wait budget is exhausted")); }
    Ok(left.min(CALL_REMOTE_BUDGET))
}

fn row_response(row: &JournalOperation) -> Result<Value, ExternalWorkError> {
    row.response_json.as_deref().map(serde_json::from_str)
        .transpose().map_err(|e| error("journal_error", format!("saved response is invalid: {e}")))
        .map(|result| result.unwrap_or_else(|| failure_response(&row.request_id, "pending", "not_dispatched", "The request is journaled and waiting for dispatch.", "reconcile")))
}

fn ensure_local_request_receipt(response: &mut Value, request_id: &str) {
    if !response.is_object() { *response = json!({}); }
    if response.get("schema").is_none() { response["schema"] = json!(CONTRACT_SCHEMA); }
    if !response.get("receipt").is_some_and(Value::is_object) { response["receipt"] = json!({}); }
    if response.pointer("/receipt/request_id").is_none_or(Value::is_null) {
        response["receipt"]["request_id"] = json!(request_id);
    }
    if response.pointer("/receipt/observed_at").is_none_or(Value::is_null) {
        response["receipt"]["observed_at"] = json!(Utc::now().to_rfc3339());
    }
    if response.pointer("/receipt/next_action").is_none_or(Value::is_null) {
        let code = response["code"].as_str().unwrap_or_default();
        response["receipt"]["next_action"] = json!(if matches!(code, "pending" | "outcome_unknown" | "authority_unavailable") { "reconcile" } else { "none" });
    }
}

fn failure_response(request_id: &str, code: &str, reason: &str, guidance: &str, next_action: &str) -> Value {
    json!({
        "schema": CONTRACT_SCHEMA,
        "code": code,
        "reason": reason,
        "guidance": guidance,
        "receipt": {
            "operation_id": null,
            "request_id": request_id,
            "resource": null,
            "visible_owner": null,
            "generation": null,
            "expires_at": null,
            "observed_at": Utc::now().to_rfc3339(),
            "next_action": next_action,
        }
    })
}

fn journal_error(error: JournalError) -> ExternalWorkError {
    match error {
        JournalError::IdempotencyConflict { request_id } => error("idempotency_conflict", format!("request_id {request_id} was used with different input")),
        JournalError::Missing(request_id) => error("not_found", format!("no local operation for request_id {request_id}")),
        JournalError::Storage(message) => error("journal_error", message),
    }
}

fn error(code: &str, message: impl Into<String>) -> ExternalWorkError {
    ExternalWorkError { code: code.to_string(), message: message.into() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct _ContractAnchor {
    schema: String,
}
