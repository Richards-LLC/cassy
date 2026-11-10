//! Request journal and v1 client for Slack-thread authority.
//!
//! The task/epic lease client has different visibility and outage semantics.
//! This module keeps thread ownership on a separate, fail-closed contract.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const CONTRACT_SCHEMA: &str = "cassy.coordination/v1";
pub const TOTAL_REMOTE_BUDGET: Duration = Duration::from_secs(8);
pub const CALL_REMOTE_BUDGET: Duration = Duration::from_secs(5);
/// cas-5fe0 (GH #1292): the observed proxy reconnect took 5–15 s.
pub const VIOLET_RECONNECT_WINDOW: Duration = Duration::from_secs(15);
/// cas-8ccd: the longest one reconciled operation can run (a reconnect wait,
/// then a fresh Cloud budget), plus margin, so a reconcile lease outlives it.
pub const RECONCILE_LEASE: Duration = Duration::from_secs(
    VIOLET_RECONNECT_WINDOW.as_secs() + TOTAL_REMOTE_BUDGET.as_secs() + 5,
);
// A daemon reconciliation job has the shared eight-second budget. Reconcile
// one operation per job so a multi-row batch cannot multiply that wait.
pub const RECONCILE_BATCH_LIMIT: usize = 1;

pub const EXTERNAL_WORK_SCHEMA_STATEMENTS: &[&str] = &[
    r#"
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
)
"#,
    "CREATE INDEX IF NOT EXISTS idx_external_work_due
     ON external_work_operations(stage, next_retry_ms, lease_until_ms)",
];

const EXTERNAL_WORK_TRIGGER_SCHEMA: &str = r#"
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
    ReplyAuthorityRequired,
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReplyAuthorityRequired => write!(f,
                "this managed thread has no current reply intent for this session; check claim_status and register a reply intent before posting"),
            Self::Storage(message) => write!(f, "external-work journal failed: {message}"),
            Self::IdempotencyConflict { request_id } => write!(
                f,
                "request_id {request_id} already has different immutable input"
            ),
            Self::Missing(request_id) => {
                write!(f, "no local operation for request_id {request_id}")
            }
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
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let schema = EXTERNAL_WORK_SCHEMA_STATEMENTS.join(";\n");
        conn.execute_batch(&schema)
            .and_then(|()| conn.execute_batch(EXTERNAL_WORK_TRIGGER_SCHEMA))
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
    pub fn set_wire_request(&self, request_id: &str, wire: &Value) -> Result<(), JournalError> {
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

    pub fn begin_attempt(&self, request_id: &str) -> Result<bool, JournalError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let changed = conn
            .execute(
                "UPDATE external_work_operations SET stage='dispatching', attempts=attempts+1,
                 updated_at=?2 WHERE request_id=?1 AND wire_json IS NOT NULL
                 AND (kind != 'claim_recovery' OR stage = 'awaiting_readback')",
                params![request_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        if changed == 0 && load_operation(&conn, request_id)?.is_none() {
            return Err(JournalError::Missing(request_id.to_string()));
        }
        Ok(changed == 1)
    }

    /// Reserve one recovery claim under the original caller's immutable request.
    /// The child cannot dispatch until Violet has settled the named reply.
    fn reserve_reply_recovery(
        &self,
        request_id: &str,
        held: &Value,
    ) -> Result<JournalOperation, JournalError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| JournalError::Storage(e.to_string()))?;
        let parent = load_operation(&tx, request_id)?
            .ok_or_else(|| JournalError::Missing(request_id.into()))?;
        let saved: Value = parent
            .response_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| JournalError::Storage(e.to_string()))?
            .unwrap_or(Value::Null);
        let child_id = if let Some(id) = saved
            .pointer("/_reply_recovery/request_id")
            .and_then(Value::as_str)
        {
            id.to_string()
        } else {
            let id = Uuid::new_v4().to_string();
            let mut wire: Value =
                serde_json::from_str(parent.wire_json.as_deref().ok_or_else(|| {
                    JournalError::Storage("recovery requires a frozen claim".into())
                })?)
                .map_err(|e| JournalError::Storage(e.to_string()))?;
            wire["request_id"] = json!(id);
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "INSERT INTO external_work_operations
                (request_id,kind,input_digest,input_json,wire_json,stage,created_at,updated_at)
                VALUES (?1,'claim_recovery',?2,?3,?4,'awaiting_readback',?5,?5)",
                params![
                    id,
                    parent.input_digest,
                    parent.input_json,
                    canonical_json(&wire)?,
                    now
                ],
            )
            .map_err(|e| JournalError::Storage(e.to_string()))?;
            let mut response = held.clone();
            response["_reply_recovery"] = json!({"request_id":id,"held":held});
            tx.execute(
                "UPDATE external_work_operations SET response_json=?2,stage='retry_wait',
                next_retry_ms=NULL,updated_at=?3 WHERE request_id=?1",
                params![request_id, canonical_json(&response)?, now],
            )
            .map_err(|e| JournalError::Storage(e.to_string()))?;
            id
        };
        let child =
            load_operation(&tx, &child_id)?.ok_or_else(|| JournalError::Missing(child_id))?;
        tx.commit()
            .map_err(|e| JournalError::Storage(e.to_string()))?;
        Ok(child)
    }

    /// Keep the recovery pointer private and durable even when the child returns
    /// an uncertain result. Held readbacks are retryable without reposting.
    fn record_reply_recovery(
        &self,
        request_id: &str,
        response: &Value,
        retry: Option<Duration>,
    ) -> Result<(), JournalError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let next_retry = retry.map(|delay| {
            Utc::now()
                .timestamp_millis()
                .saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX))
        });
        conn.execute("UPDATE external_work_operations SET response_json=json_set(?2,'$._reply_recovery',
                json_extract(response_json,'$._reply_recovery')),stage=?3,next_retry_ms=?4,
                lease_token=NULL,lease_until_ms=NULL,updated_at=?5 WHERE request_id=?1 AND stage != 'terminal'",
            params![request_id, canonical_json(response)?, if retry.is_some() { "retry_wait" } else { "terminal" },
                next_retry, Utc::now().to_rfc3339()])
            .map_err(|e| JournalError::Storage(e.to_string()))?;
        Ok(())
    }

    pub fn record_result(
        &self,
        request_id: &str,
        response: &Value,
        retry_after: Option<Duration>,
    ) -> Result<(), JournalError> {
        let response_json = canonical_json(response)?;
        let code = response
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let retry = matches!(
            code,
            "pending" | "outcome_unknown" | "authority_unavailable"
        );
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
                 WHERE request_id=?1
                   AND json_extract(COALESCE(response_json, '{}'), '$._reply_recovery') IS NULL",
                params![
                    request_id,
                    stage,
                    response_json,
                    retry_at,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        if changed == 0 && load_operation(&conn, request_id)?.is_none() {
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

    /// Local evidence for degraded stakeholder warnings, never shared authority.
    pub fn read_stakeholder_contact_evidence(cas_dir: &Path) -> Result<Vec<Value>, JournalError> {
        let path = cas_dir.join("cas.db");
        if !path.exists() { return Ok(Vec::new()); }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='external_work_operations')", [], |row| row.get(0)
        ).map_err(|error| JournalError::Storage(error.to_string()))?;
        if !exists { return Ok(Vec::new()); }
        Self { conn: Arc::new(Mutex::new(conn)) }.stakeholder_contact_evidence()
    }

    /// Existing journal rows only; does not create a second evidence store.
    pub fn stakeholder_contact_evidence(&self) -> Result<Vec<Value>, JournalError> {
        let conn = self.conn.lock().map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        let mut query = conn.prepare(
            "SELECT response_json FROM external_work_operations WHERE kind='stakeholder_contact' AND response_json IS NOT NULL ORDER BY updated_at DESC, rowid DESC"
        ).map_err(|error| JournalError::Storage(error.to_string()))?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| JournalError::Storage(error.to_string()))?;
        rows.map(|row| {
            let row = row.map_err(|error| JournalError::Storage(error.to_string()))?;
            serde_json::from_str(&row).map_err(|error| JournalError::Storage(error.to_string()))
        }).collect()
    }

    /// Advisory task preflight over the existing request journal. The service
    /// refreshes authority through Violet/Cloud; this cache never grants a post
    /// or a claim. Follow later renew/release/transfer receipts for the resource.
    pub(crate) fn task_source_receipt(&self, source: &str, project: &str) -> Result<Option<Value>, JournalError> {
        fn alias(source: &str) -> Option<String> {
            let url = url::Url::parse(source).ok()?;
            let parts = url.path_segments()?.collect::<Vec<_>>();
            if url.scheme() != "https" || !url.host_str()?.ends_with(".slack.com")
                || parts.len() != 3 || parts[0] != "archives" { return None; }
            let root = url.query_pairs().find(|(key, _)| key == "thread_ts")
                .map(|(_, value)| value.replace('.', ""))
                .or_else(|| parts[2].strip_prefix('p').map(str::to_owned))?;
            if root.is_empty() || !root.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
            Some(format!("{}/{}/{root}", url.host_str()?, parts[1]))
        }
        let Some(source) = alias(source) else { return Ok(None); };
        let rows: Vec<(String, String)> = {
            let conn = self.conn.lock().map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
            let mut query = conn.prepare("SELECT input_json, response_json FROM external_work_operations WHERE response_json IS NOT NULL ORDER BY created_at DESC, rowid DESC")
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| JournalError::Storage(error.to_string()))?
                .collect::<Result<_, _>>().map_err(|error| JournalError::Storage(error.to_string()))?
        };
        let rows = rows.into_iter().filter_map(|(input, response)| Some((
            serde_json::from_str::<Value>(&input).ok()?, serde_json::from_str::<Value>(&response).ok()?
        ))).filter(|(input, _)| input["project_id"].as_str() == Some(project)).collect::<Vec<_>>();
        let resource = rows.iter().find_map(|(input, response)| {
            (input["external_ref"].as_str().and_then(alias).as_deref() == Some(source.as_str()))
                .then(|| response["receipt"]["resource"]["id"].as_str().map(str::to_owned)).flatten()
        });
        let Some(resource) = resource else { return Ok(None); };
        let receipt = rows.into_iter().find_map(|(_, response)| {
            let receipt = &response["receipt"];
            (receipt["resource"]["id"].as_str() == Some(resource.as_str())).then(|| receipt.clone())
        });
        Ok(receipt.filter(|receipt| {
            receipt["resource"]["state"] == "claimed"
                && receipt["visible_owner"]["registration_id"].is_string()
                && receipt["expires_at"].as_str().and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                    .is_some_and(|at| at > Utc::now())
        }))
    }

    pub fn has_due(&self, now_ms: i64) -> Result<bool, JournalError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM external_work_operations
                WHERE kind NOT IN ('stakeholder_contact','stakeholder_reservation','claim_recovery')
                  AND stage IN ('prepared','retry_wait','dispatching')
                  AND (next_retry_ms IS NULL OR next_retry_ms <= ?1)
                  AND (lease_until_ms IS NULL OR lease_until_ms <= ?1)
             )",
            [now_ms],
            |row| row.get(0),
        )
        .map_err(|error| JournalError::Storage(error.to_string()))
    }

    /// Atomically lease a bounded batch of due reconciliation operations.
    pub fn lease_due(
        &self,
        limit: usize,
        now_ms: i64,
        lease_for: Duration,
    ) -> Result<Vec<JournalOperation>, JournalError> {
        self.lease_due_for_claimant(limit, now_ms, lease_for, None)
    }

    fn lease_due_for_claimant(
        &self,
        limit: usize,
        now_ms: i64,
        lease_for: Duration,
        registration_id: Option<&str>,
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
                 WHERE kind NOT IN ('stakeholder_contact','stakeholder_reservation','claim_recovery')
                   AND stage IN ('prepared','retry_wait','dispatching')
                       AND (?3 IS NULL OR kind != 'claim'
                            OR json_extract(input_json, '$.actor.registration_id') = ?3)
                       AND (next_retry_ms IS NULL OR next_retry_ms <= ?1)
                       AND (lease_until_ms IS NULL OR lease_until_ms <= ?1)
                     ORDER BY COALESCE(next_retry_ms, 0), created_at LIMIT ?2",
                )
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            statement
                .query_map(params![now_ms, limit as i64, registration_id], |row| row.get::<_, String>(0))
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

impl SqliteRequestJournal {
    /// cas-8239 (GH #1300): the `violet_post` fields of `registration_id`'s
    /// current reply intent for the thread rooted at `reply_to`, so a reply
    /// can be posted without copying claim fields by hand. Only completed
    /// operations count, in the order they were made. A reply intent stays
    /// current until a later release, transfer, or a claim or accept at
    /// another generation retires it; a renewal extends its claim's expiry.
    /// A known managed thread without a live caller intent is refused; only
    /// an unknown thread yields `None` for ordinary Violet posting.
    pub fn current_reply_post(
        &self,
        registration_id: &str,
        reply_to: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Value>, JournalError> {
        let operations: Vec<(String, String, String)> = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| JournalError::Storage("connection lock poisoned".into()))?;
            let mut statement = conn
                .prepare(
                    "SELECT kind, input_json, response_json FROM external_work_operations
                     WHERE stage = 'terminal' AND response_json IS NOT NULL
                     ORDER BY created_at, rowid",
                )
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|error| JournalError::Storage(error.to_string()))?;
            rows.collect::<Result<_, _>>().map_err(|error| JournalError::Storage(error.to_string()))?
        };
        // resource id -> (violet_post block, claim generation, claim expiry)
        let mut current: std::collections::HashMap<String, (Value, u64, Option<DateTime<Utc>>)> =
            std::collections::HashMap::new();
        let mut managed_thread = false;
        let expiry = |receipt: &Value| {
            receipt["expires_at"]
                .as_str()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.with_timezone(&Utc))
        };
        for (kind, input, response) in operations {
            let (Ok(input), Ok(response)) =
                (serde_json::from_str::<Value>(&input), serde_json::from_str::<Value>(&response))
            else {
                continue;
            };
            let receipt = &response["receipt"];
            managed_thread |= receipt["resource"]["kind"] == "slack_thread"
                && receipt["resource"]["external_ref"]["root_ts"] == reply_to;
            let Some(resource) = input["resource_id"].as_str().or(receipt["resource"]["id"].as_str()) else {
                continue;
            };
            let resource = resource.to_string();
            let generation = receipt["generation"].as_u64();
            // A newer claim or a peer's release can supersede our cached
            // intent. Its receipt remains authoritative even when this actor
            // did not perform the mutation or the result is `acquired`.
            if current.get(&resource).is_some_and(|entry| {
                generation.is_some_and(|generation| generation != entry.1)
                    || receipt["resource"]["state"].as_str().is_some_and(|state| state != "claimed")
                    || receipt["visible_owner"]["registration_id"].as_str().is_some_and(|owner| owner != registration_id)
            }) {
                current.remove(&resource);
            }
            if input["actor"]["registration_id"] != registration_id || response["code"] != "completed" {
                continue;
            }
            match kind.as_str() {
                "reply_intent" => {
                    let Some(expected) = input["expected_generation"].as_u64() else { continue };
                    // Expiry is judged below, after any later renewal.
                    let ready = with_reply_ready(response.clone(), &resource, expected, DateTime::<Utc>::MIN_UTC);
                    match ready.get("violet_post") {
                        Some(block) => {
                            current.insert(resource, (block.clone(), expected, expiry(receipt)));
                        }
                        None => {
                            current.remove(&resource);
                        }
                    }
                }
                "renew_claim" => {
                    if let Some(entry) = current.get_mut(&resource)
                        && generation == Some(entry.1)
                    {
                        entry.2 = expiry(receipt).or(entry.2);
                    }
                }
                "release_claim" | "transfer_claim" => {
                    current.remove(&resource);
                }
                _ => {
                    if current.get(&resource).is_some_and(|entry| generation != Some(entry.1)) {
                        current.remove(&resource);
                    }
                }
            }
        }
        let intent = current
            .into_values()
            .filter(|(block, _, expires)| block["reply_to"] == reply_to && expires.is_some_and(|at| at > now))
            .map(|(block, _, _)| block)
            .next();
        if managed_thread && intent.is_none() {
            return Err(JournalError::ReplyAuthorityRequired);
        }
        Ok(intent)
    }
}

/// Resolve file-reply authority for either supported client before allocating
/// an upload. The registration comes from the authenticated caller, never args.
pub(crate) fn reply_post(
    cas_root: &Path,
    registration_id: &str,
    reply_to: &str,
) -> Result<Option<Value>, ExternalWorkError> {
    SqliteRequestJournal::open(cas_root)
        .and_then(|journal| journal.current_reply_post(registration_id, reply_to, Utc::now()))
        .map_err(journal_error)
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
    /// cas-5fe0 (GH #1292): the upstream connection dropped and the proxy is
    /// reconnecting; a read-only call may be retried shortly.
    pub reconnecting: bool,
}

impl RemoteError {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: false,
            reconnecting: false,
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: true,
            reconnecting: false,
        }
    }

    /// A dropped upstream that the proxy is reconnecting (cas-5fe0).
    pub fn reconnecting(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: false,
            reconnecting: true,
        }
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
    /// A read-only GET; like `post`, Cloud's refusal bodies come back as values.
    fn get(&self, path: &str, query: &[(&str, &str)], timeout: Duration) -> Result<Value, RemoteError> {
        let _ = (path, query, timeout);
        Err(RemoteError::unavailable("this coordination port has no GET"))
    }
}

pub trait VioletThreadResolver: Send + Sync {
    /// Resolve only through Violet's authenticated `violet_read` contract and
    /// return its normalized, verified thread identity.
    fn resolve(&self, external_ref: &str, timeout: Duration) -> Result<Value, RemoteError>;
    /// Read back and settle an existing reply intent through Violet. Never post.
    fn reconcile_reply(&self, intent_id: &str, timeout: Duration) -> Result<Value, RemoteError> {
        let _ = (intent_id, timeout);
        Err(RemoteError::unavailable("Violet reply readback is unavailable"))
    }
}

/// Arguments shared by both thread identity resolvers.
pub(crate) fn violet_identity_read_args(external_ref: &str) -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([
        ("external_ref".into(), json!(external_ref)),
        ("include_threads".into(), json!(false)),
    ])
}

/// cas-85c1 (GH #1292): the one mapping from a proxied `violet_read` call to
/// the resolver result, shared by the MCP and daemon resolvers so both mark
/// a dropped, reconnecting upstream the same way (via the proxy's structural
/// classifier) and the service retries it within its reconnect window.
#[cfg(feature = "mcp-proxy")]
pub(crate) fn violet_proxy_outcome(
    outcome: Result<anyhow::Result<Value>, tokio::time::error::Elapsed>,
) -> Result<Value, RemoteError> {
    violet_proxy_payload(outcome).and_then(decode_violet_result)
}

#[cfg(feature = "mcp-proxy")]
pub(crate) fn violet_proxy_payload(
    outcome: Result<anyhow::Result<Value>, tokio::time::error::Elapsed>,
) -> Result<Value, RemoteError> {
    match outcome {
        Ok(Ok(value)) => violet_payload(value),
        Ok(Err(error)) => {
            let message = format!(
                "Violet could not resolve the Slack permalink: {}",
                cmcp_core::describe_upstream_call_error(&error)
            );
            Err(if cmcp_core::upstream_call_is_reconnectable(&error) {
                RemoteError::reconnecting(message)
            } else {
                RemoteError::unavailable(message)
            })
        }
        Err(_) => Err(RemoteError::timeout("Violet thread resolution timed out")),
    }
}

/// Extract either deployed MCP envelope without interpreting the tool's payload.
fn violet_payload(value: Value) -> Result<Value, RemoteError> {
    value
        .get("structuredContent")
        .or_else(|| value.get("structured_content"))
        .cloned()
        .or_else(|| {
            value
                .get("content")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find_map(|item| {
                        item.get("text")
                            .and_then(Value::as_str)
                            .and_then(|text| serde_json::from_str::<Value>(text).ok())
                    })
                })
        })
        .or_else(|| (value.get("ok").is_some() || value.get("code").is_some()).then(|| value.clone()))
        .ok_or_else(|| RemoteError::unavailable("Violet returned no structured response"))
}

/// Normalize Violet's identity-only `violet_read` response for the claim service.
pub(crate) fn decode_violet_result(value: Value) -> Result<Value, RemoteError> {
    let payload = violet_payload(value)?;
    if payload.get("ok").is_some() {
        if payload.get("ok").and_then(Value::as_bool) == Some(false) {
            let refusal = payload.get("error").cloned().unwrap_or(Value::Null);
            let code = refusal
                .get("code")
                .and_then(Value::as_str)
                .filter(|code| *code != "completed")
                .unwrap_or("identity_refused");
            let guidance = refusal
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Violet refused to verify this Slack thread identity.");
            return Ok(json!({
                "schema": CONTRACT_SCHEMA,
                "code": code,
                "guidance": guidance,
            }));
        }
        if payload.get("ok").and_then(Value::as_bool) != Some(true)
            || payload.get("schema_version").and_then(Value::as_u64) != Some(1)
        {
            return Err(RemoteError::unavailable(
                "Violet returned an unsupported identity response schema",
            ));
        }

        let identity = payload.get("identity").ok_or_else(|| {
            RemoteError::unavailable("Violet omitted the verified Slack thread identity")
        })?;
        let identity_fields = ["workspace_id", "channel_id", "root_thread_ts"];
        if identity_fields.iter().any(|field| {
            identity
                .get(*field)
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
        }) {
            return Err(RemoteError::unavailable(
                "Violet returned an incomplete verified Slack thread identity",
            ));
        }
        return Ok(json!({
            "schema": CONTRACT_SCHEMA,
            "code": "completed",
            "thread_identity": {
                "provider": "slack",
                "workspace_id": identity["workspace_id"],
                "channel_id": identity["channel_id"],
                "root_ts": identity["root_thread_ts"],
            }
        }));
    }

    // Some internal test doubles already speak the normalized interface. Keep
    // accepting that service-level shape; production proxy responses are
    // normalized above from the deployed Violet contract.
    if payload.get("code").and_then(Value::as_str) == Some("completed")
        && payload.get("thread_identity").is_some()
    {
        return Ok(payload);
    }

    Err(RemoteError::unavailable(
        "Violet returned no structured thread identity",
    ))
}

#[derive(Clone)]
pub struct HttpCloudCoordinationPort {
    endpoint: String,
    token: String,
}

impl HttpCloudCoordinationPort {
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
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
        let response = crate::cloud::mutation_request("POST", &url)
            .timeout(timeout)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(payload);
        match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => Self::decode_response(response),
            Err(ureq::Error::Transport(error)) => {
                let message = error.to_string();
                let timed_out = message.to_ascii_lowercase().contains("timed out");
                Err(RemoteError { message, timed_out, reconnecting: false })
            }
        }
    }

    fn lookup(
        &self,
        request_id: &str,
        registration_id: &str,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        self.get(
            &format!("/api/coordination/operations/{request_id}"),
            &[("registration_id", registration_id)],
            timeout,
        )
    }

    fn get(&self, path: &str, query: &[(&str, &str)], timeout: Duration) -> Result<Value, RemoteError> {
        let mut request = ureq::get(&format!("{}{}", self.endpoint, path))
            .timeout(timeout)
            .set("Authorization", &format!("Bearer {}", self.token));
        for (name, value) in query {
            request = request.query(name, value);
        }
        match request.call() {
            Ok(response) | Err(ureq::Error::Status(_, response)) => Self::decode_response(response),
            Err(ureq::Error::Transport(error)) => {
                let message = error.to_string();
                let timed_out = message.to_ascii_lowercase().contains("timed out");
                Err(RemoteError { message, timed_out, reconnecting: false })
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
    Claim {
        external_ref: String,
        request_id: Option<String>,
    },
    ClaimStatus {
        request_id: String,
    },
    ReleaseClaim {
        resource_id: String,
        expected_generation: u64,
        request_id: Option<String>,
    },
    TransferClaim {
        resource_id: String,
        expected_generation: u64,
        to_slot_id: String,
        offer_ttl_secs: Option<u32>,
        request_id: Option<String>,
    },
    AcceptClaim {
        handoff_id: String,
        expected_generation: u64,
        request_id: Option<String>,
    },
    /// cas-f869: extend a live claim's idle lease (Cloud `…/renew`).
    RenewClaim {
        resource_id: String,
        expected_generation: u64,
        request_id: Option<String>,
    },
    /// cas-f869 (GH #1272): register a reply intent under the current claim
    /// (Cloud `…/reply-intents`); `occurrence_id` is optional in the contract.
    ReplyIntent {
        resource_id: String,
        expected_generation: u64,
        occurrence_id: Option<String>,
        request_id: Option<String>,
    },
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
    /// cas-5fe0 (GH #1292): how long the read-only Violet resolution keeps
    /// retrying while the proxy reconnects a dropped upstream.
    pub violet_reconnect_window: Duration,
    /// cas-f869: the invocation clock a reply-intent's claim expiry is judged
    /// against (tests pin it).
    pub now: fn() -> DateTime<Utc>,
}

impl ExternalWorkService {
    pub fn new(
        journal: SqliteRequestJournal,
        cloud: Arc<dyn CloudCoordinationPort>,
        violet: Arc<dyn VioletThreadResolver>,
        context: OperationContext,
    ) -> Self {
        Self {
            journal,
            cloud,
            violet,
            context,
            total_budget: TOTAL_REMOTE_BUDGET,
            violet_reconnect_window: VIOLET_RECONNECT_WINDOW,
            now: Utc::now,
        }
    }

    pub fn execute(&self, command: ExternalWorkCommand) -> Result<Value, ExternalWorkError> {
        match command {
            ExternalWorkCommand::ClaimStatus { request_id } => self.claim_status(&request_id).map(explain_unmanaged),
            claim @ ExternalWorkCommand::Claim { .. } => self.mutate(claim).map(explain_unmanaged),
            ExternalWorkCommand::ReplyIntent {
                ref resource_id,
                expected_generation,
                ..
            } => {
                let resource_id = resource_id.clone();
                let result = self.mutate(command)?;
                Ok(with_reply_ready(result, &resource_id, expected_generation, (self.now)()))
            }
            other => self.mutate(other),
        }
    }

    /// Resolve a permalink through Violet. A dropped upstream that the proxy
    /// is reconnecting is retried, read-only, until it answers or
    /// [`Self::violet_reconnect_window`] passes; any other failure returns at
    /// once. A persistent outage still fails, naming how long it was retried.
    ///
    /// cas-8ccd: the first read spends the request's one remote-wait budget
    /// (`deadline`), exactly as the spec's eight seconds. Only when a read
    /// meets a reconnecting upstream does the claim wait longer: retries are
    /// bounded by the reconnect window (each call cut to what is left of it),
    /// and a read that then succeeds hands the Cloud calls a fresh budget.
    /// Returns the result and the deadline the Cloud calls must honour.
    fn resolve_violet(&self, external_ref: &str, deadline: Instant) -> (Result<Value, RemoteError>, Instant) {
        let started = Instant::now();
        let first = deadline.saturating_duration_since(started).min(CALL_REMOTE_BUDGET);
        let failure = match self.violet.resolve(external_ref, first) {
            Err(failure) if failure.reconnecting => failure,
            other => return (other, deadline),
        };
        let window_end = started + self.violet_reconnect_window;
        let poll = (self.violet_reconnect_window / 6)
            .clamp(Duration::from_millis(10), Duration::from_secs(2));
        let mut failure = failure;
        while Instant::now() + poll < window_end {
            std::thread::sleep(poll);
            let left = window_end.saturating_duration_since(Instant::now()).min(CALL_REMOTE_BUDGET);
            match self.violet.resolve(external_ref, left) {
                Err(next) if next.reconnecting => failure = next,
                other => return (other, Instant::now() + self.total_budget),
            }
        }
        failure.message = format!(
            "{} (still unavailable after retrying for {}s)",
            failure.message,
            started.elapsed().as_secs()
        );
        (Err(failure), deadline)
    }

    fn mutate(&self, command: ExternalWorkCommand) -> Result<Value, ExternalWorkError> {
        command.validate()?;
        let kind = command.kind().to_string();
        let request_id = valid_request_id(command.request_id())?;
        let input = command.input(&self.context);
        let (mut row, created) = match self.journal.prepare(&request_id, &kind, &input) {
            Ok(prepared) => prepared,
            Err(JournalError::IdempotencyConflict { request_id }) => {
                let mut response = failure_response(
                    &request_id,
                    "idempotency_conflict",
                    "request_input_changed",
                    "This request ID was already used with different input. Reuse the original input or allocate a new request ID for a new intent.",
                    "use_original_input",
                );
                if let Some(previous) = self.journal.get(&request_id).map_err(journal_error)?
                    && let Some(response_json) = previous.response_json
                    && let Ok(previous_response) = serde_json::from_str::<Value>(&response_json)
                    && let Some(operation_id) = previous_response.pointer("/receipt/operation_id")
                {
                    response["receipt"]["operation_id"] = operation_id.clone();
                }
                return Ok(response);
            }
            Err(error) => return Err(journal_error(error)),
        };
        if !created {
            if row.stage == "terminal" {
                return self.recover_reply(&row, row_response(&row)?, Instant::now() + self.total_budget);
            }
            if row.stage == "dispatching" || row.stage == "retry_wait" && row.wire_json.is_some() {
                return self.reconcile(&row);
            }
        }

        let mut deadline = Instant::now() + self.total_budget;
        if kind == "claim" && row.wire_json.is_none() {
            let external_ref = command
                .claim_external_ref()
                .ok_or_else(|| error("invalid_request", "claim requires external_ref"))?;
            let (resolution, cloud_deadline) = self.resolve_violet(external_ref, deadline);
            deadline = cloud_deadline;
            let resolved = match resolution {
                Ok(value) => value,
                Err(error) => {
                    let result = failure_response(
                        &request_id,
                        if error.timed_out {
                            "authority_unavailable"
                        } else {
                            "authority_unavailable"
                        },
                        "violet_unavailable",
                        &error.message,
                        "retry_later",
                    );
                    self.journal
                        .record_result(&request_id, &result, Some(Duration::from_secs(2)))
                        .map_err(journal_error)?;
                    return Ok(result);
                }
            };
            let code = resolved
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if code != "completed" {
                let mut refused = resolved;
                ensure_local_request_receipt(&mut refused, &request_id);
                self.journal
                    .record_result(&request_id, &refused, None)
                    .map_err(journal_error)?;
                return Ok(refused);
            }
            let identity = resolved.get("thread_identity").ok_or_else(|| {
                error(
                    "invalid_response",
                    "Violet response omitted verified thread identity",
                )
            })?;
            let identity_fields = ["provider", "workspace_id", "channel_id", "root_ts"];
            if identity_fields
                .iter()
                .any(|field| identity.get(*field).and_then(Value::as_str).is_none())
            {
                return Err(error(
                    "invalid_response",
                    "Violet response has incomplete verified thread identity",
                ));
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
            self.journal
                .set_wire_request(&request_id, &wire)
                .map_err(journal_error)?;
            row = self
                .journal
                .get(&request_id)
                .map_err(journal_error)?
                .ok_or_else(|| error("journal_error", "prepared operation disappeared"))?;
        } else if row.wire_json.is_none() {
            let wire = command.wire(&request_id, &self.context);
            self.journal
                .set_wire_request(&request_id, &wire)
                .map_err(journal_error)?;
            row = self
                .journal
                .get(&request_id)
                .map_err(journal_error)?
                .ok_or_else(|| error("journal_error", "prepared operation disappeared"))?;
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
        let payload: Value = serde_json::from_str(
            row.wire_json
                .as_deref()
                .ok_or_else(|| error("journal_error", "provider request was not journaled"))?,
        )
        .map_err(|e| {
            error(
                "journal_error",
                format!("saved provider request is invalid: {e}"),
            )
        })?;
        let timeout = call_timeout(deadline)?;
        if !self.journal.begin_attempt(&row.request_id).map_err(journal_error)? {
            return self.reconcile_with_deadline(row, deadline);
        }
        let result = match self.cloud.post(&path, &payload, timeout) {
            Ok(mut response) => {
                if response.get("code").and_then(Value::as_str).is_none() {
                    failure_response(
                        &row.request_id,
                        "outcome_unknown",
                        "malformed_response",
                        "Cloud returned no stable operation code; reconcile the original request ID.",
                        "reconcile",
                    )
                } else {
                    ensure_local_request_receipt(&mut response, &row.request_id);
                    response
                }
            }
            Err(error) => failure_response(
                &row.request_id,
                "outcome_unknown",
                if error.timed_out {
                    "request_timeout"
                } else {
                    "cloud_unavailable"
                },
                &error.message,
                "reconcile",
            ),
        };
        let retry = matches!(
            result["code"].as_str(),
            Some("pending" | "outcome_unknown" | "authority_unavailable")
        );
        self.journal
            .record_result(
                &row.request_id,
                &result,
                retry.then_some(Duration::from_secs(2)),
            )
            .map_err(journal_error)?;
        self.recover_reply(row, result, deadline)
    }

    /// Cloud alone identifies an expired, departed owner. Violet only reads
    /// and settles its reply; the new claim still goes through Cloud authority.
    fn recover_reply(
        &self,
        row: &JournalOperation,
        result: Value,
        deadline: Instant,
    ) -> Result<Value, ExternalWorkError> {
        if row.kind != "claim" {
            return Ok(result);
        }
        let saved: Value = row
            .response_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| error("journal_error", e.to_string()))?
            .unwrap_or(Value::Null);
        let recovery = saved.get("_reply_recovery");
        if recovery.is_some()
            && (row.stage == "terminal"
                || result["code"] == "held"
                    && row
                        .next_retry_ms
                        .is_some_and(|at| at > Utc::now().timestamp_millis()))
        {
            return Ok(result);
        }
        let held = recovery
            .map(|metadata| &metadata["held"])
            .unwrap_or(&result);
        let wire: Value = row
            .wire_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| error("journal_error", e.to_string()))?
            .unwrap_or(Value::Null);
        let receipt = &held["receipt"];
        let intent = &receipt["reply_intent"];
        let intent_id = intent["id"].as_str().unwrap_or_default();
        let generation = receipt["generation"]
            .as_u64()
            .filter(|generation| *generation > 0);
        let date = |value: &Value| {
            value
                .as_str()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        };
        let expired = date(&receipt["expires_at"])
            .zip(date(&receipt["observed_at"]))
            .is_some_and(|(expires, observed)| expires <= observed);
        if held["schema"] != CONTRACT_SCHEMA
            || held["code"] != "held"
            || held["reason"] != "reply_unresolved"
            || receipt["next_action"] != "reconcile"
            || intent["status"] != "outcome_unknown"
            || valid_uuid(intent_id).is_err()
            || generation.is_none()
            || intent["claim_generation"].as_u64() != generation
            || receipt["request_id"] != row.request_id
            || wire["actor"]["registration_id"] != self.context.registration_id
            || wire["project_id"] != self.context.project_id
            || receipt["resource"]["kind"] != "slack_thread"
            || receipt["resource"]["state"] != "claimed"
            || receipt["resource"]["project_id"] != wire["project_id"]
            || receipt["resource"]["external_ref"] != wire["external_ref"]
            || receipt["resource"]["id"]
                .as_str()
                .is_none_or(|id| valid_uuid(id).is_err())
            || receipt["visible_owner"]["is_caller"] != false
            || receipt["visible_owner"]["registration_id"]
                .as_str()
                .is_none_or(|owner| owner.trim().is_empty() || owner == self.context.registration_id)
            || !expired
        {
            return Ok(result);
        }
        let child = self
            .journal
            .reserve_reply_recovery(&row.request_id, held)
            .map_err(journal_error)?;
        let result = if child.stage == "awaiting_readback" {
            let readback = call_timeout(deadline)
                .ok()
                .and_then(|timeout| self.violet.reconcile_reply(intent_id, timeout).ok());
            let settled = readback.as_ref().is_some_and(|reply| {
                let terminal = matches!(reply["state"].as_str(), Some("posted" | "rejected"));
                let acknowledged = reply["ok"] == true
                    && (reply["state"] != "posted"
                        || reply["message_ts"]
                            .as_str()
                            .is_some_and(|ts| !ts.is_empty()));
                let already_settled =
                    reply["ok"] == false && reply["reason"] == "intent_not_unresolved";
                terminal
                    && (acknowledged || already_settled)
                    && reply["coordination_operation_id"] == intent_id
                    && reply
                        .get("channel_id")
                        .is_none_or(|id| id == &wire["external_ref"]["channel_id"])
                    && reply
                        .get("thread_ts")
                        .is_none_or(|ts| ts == &wire["external_ref"]["root_ts"])
            });
            if !settled {
                let delay = readback
                    .as_ref()
                    .and_then(|reply| reply["retry_after"].as_u64())
                    .unwrap_or(2)
                    .max(2);
                self.journal
                    .record_reply_recovery(&row.request_id, held, Some(Duration::from_secs(delay)))
                    .map_err(journal_error)?;
                return Ok(held.clone());
            }
            let input: Value = serde_json::from_str(&row.input_json)
                .map_err(|e| error("journal_error", e.to_string()))?;
            let command = ExternalWorkCommand::Claim {
                external_ref: input["external_ref"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                request_id: Some(child.request_id.clone()),
            };
            // Failure to start before the shared deadline leaves the reserved
            // child undispatched; the next attempt can safely read back again.
            if call_timeout(deadline).is_err() {
                self.journal
                    .record_reply_recovery(&row.request_id, held, Some(Duration::from_secs(2)))
                    .map_err(journal_error)?;
                return Ok(held.clone());
            }
            self.dispatch(&child, &command, deadline)?
        } else if child.stage == "terminal" {
            row_response(&child)?
        } else {
            self.reconcile_with_deadline(&child, deadline)?
        };
        let retry = matches!(
            result["code"].as_str(),
            Some("pending" | "outcome_unknown" | "authority_unavailable")
        );
        self.journal
            .record_reply_recovery(
                &row.request_id,
                &result,
                retry.then_some(Duration::from_secs(2)),
            )
            .map_err(journal_error)?;
        Ok(result)
    }

    pub fn claim_status(&self, request_id: &str) -> Result<Value, ExternalWorkError> {
        let request_id = valid_uuid(request_id)?;
        let Some(row) = self.journal.get(&request_id).map_err(journal_error)? else {
            let deadline = Instant::now() + self.total_budget;
            let timeout = call_timeout(deadline)?;
            let mut result = self
                .cloud
                .lookup(&request_id, &self.context.registration_id, timeout)
                .map_err(|e| error("authority_unavailable", e.message))?;
            ensure_local_request_receipt(&mut result, &request_id);
            return Ok(result);
        };
        let input: Value = serde_json::from_str(&row.input_json)
            .map_err(|e| error("journal_error", e.to_string()))?;
        if input
            .pointer("/actor/registration_id")
            .and_then(Value::as_str)
            != Some(self.context.registration_id.as_str())
        {
            return Err(error(
                "not_authorized",
                "operation belongs to a different registered supervisor",
            ));
        }
        if row.stage == "terminal" {
            return self.recover_reply(&row, row_response(&row)?, Instant::now() + self.total_budget);
        }
        if row.kind == "claim" && row.wire_json.is_none() {
            let ref_link = input
                .get("external_ref")
                .and_then(Value::as_str)
                .ok_or_else(|| error("journal_error", "claim permalink missing from journal"))?;
            let (resolved, deadline) = self.resolve_violet(ref_link, Instant::now() + self.total_budget);
            let resolved = resolved.map_err(|e| error("authority_unavailable", e.message))?;
            if resolved.get("code").and_then(Value::as_str) != Some("completed") {
                let mut response = resolved;
                ensure_local_request_receipt(&mut response, &request_id);
                self.journal
                    .record_result(&request_id, &response, None)
                    .map_err(journal_error)?;
                return Ok(response);
            }
            let identity = resolved.get("thread_identity").ok_or_else(|| {
                error(
                    "invalid_response",
                    "Violet response omitted verified thread identity",
                )
            })?;
            if ["provider", "workspace_id", "channel_id", "root_ts"]
                .iter()
                .any(|field| identity.get(*field).and_then(Value::as_str).is_none())
            {
                return Err(error(
                    "invalid_response",
                    "Violet response has incomplete verified thread identity",
                ));
            }
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
            self.journal
                .set_wire_request(&request_id, &wire)
                .map_err(journal_error)?;
            let row = self
                .journal
                .get(&request_id)
                .map_err(journal_error)?
                .ok_or_else(|| error("journal_error", "prepared operation disappeared"))?;
            let command = ExternalWorkCommand::Claim {
                external_ref: ref_link.to_string(),
                request_id: Some(request_id.clone()),
            };
            return self.dispatch(&row, &command, deadline);
        }
        self.reconcile(&row)
    }

    fn reconcile(&self, row: &JournalOperation) -> Result<Value, ExternalWorkError> {
        self.reconcile_with_deadline(row, Instant::now() + self.total_budget)
    }

    fn reconcile_with_deadline(&self, row: &JournalOperation, deadline: Instant) -> Result<Value, ExternalWorkError> {
        if row.kind == "claim" && row.response_json.as_deref()
            .and_then(|saved| serde_json::from_str::<Value>(saved).ok())
            .is_some_and(|saved| saved.get("_reply_recovery").is_some()) {
            return self.recover_reply(row, row_response(row)?, deadline);
        }
        self.journal
            .begin_attempt(&row.request_id)
            .map_err(journal_error)?;
        let lookup = self.cloud.lookup(
            &row.request_id,
            &self.context.registration_id,
            call_timeout(deadline)?,
        );
        let result = match lookup {
            Ok(mut response) => {
                ensure_local_request_receipt(&mut response, &row.request_id);
                response
            }
            Err(error) => failure_response(
                &row.request_id,
                "outcome_unknown",
                if error.timed_out {
                    "lookup_timeout"
                } else {
                    "cloud_unavailable"
                },
                &error.message,
                "reconcile",
            ),
        };
        let retry = matches!(
            result["code"].as_str(),
            Some("pending" | "outcome_unknown" | "authority_unavailable")
        );
        self.journal
            .record_result(
                &row.request_id,
                &result,
                retry.then_some(Duration::from_secs(2)),
            )
            .map_err(journal_error)?;
        self.recover_reply(row, result, deadline)
    }

    /// One bounded background batch. Callers submit this on an external job
    /// lane; the loop and hooks never call it synchronously.
    pub fn reconcile_due(&self, now_ms: i64) -> Result<Vec<Value>, ExternalWorkError> {
        let rows = self
            .journal
            .lease_due_for_claimant(RECONCILE_BATCH_LIMIT, now_ms, RECONCILE_LEASE, Some(&self.context.registration_id))
            .map_err(journal_error)?;
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let result = if row.kind == "claim" {
                self.claim_status(&row.request_id)?
            } else {
                self.reconcile(&row)?
            };
            self.journal
                .clear_lease(&row.request_id)
                .map_err(journal_error)?;
            results.push(result);
        }
        Ok(results)
    }
}

impl ExternalWorkCommand {
    fn validate(&self) -> Result<(), ExternalWorkError> {
        let path_uuid = match self {
            Self::ReleaseClaim { resource_id, .. }
            | Self::TransferClaim { resource_id, .. }
            | Self::RenewClaim { resource_id, .. }
            | Self::ReplyIntent { resource_id, .. } => Some((resource_id, "resource_id")),
            Self::AcceptClaim { handoff_id, .. } => Some((handoff_id, "handoff_id")),
            _ => None,
        };
        if let Some((value, field)) = path_uuid {
            validate_uuid_field(value, field)?;
        }
        if let Self::TransferClaim { to_slot_id, .. } = self {
            validate_uuid_field(to_slot_id, "to_slot_id")?;
        }
        if let Self::ReplyIntent {
            occurrence_id: Some(occurrence_id),
            ..
        } = self
        {
            validate_uuid_field(occurrence_id, "occurrence_id")?;
        }
        let generation = match self {
            Self::ReleaseClaim {
                expected_generation,
                ..
            }
            | Self::TransferClaim {
                expected_generation,
                ..
            }
            | Self::AcceptClaim {
                expected_generation,
                ..
            }
            | Self::RenewClaim {
                expected_generation,
                ..
            }
            | Self::ReplyIntent {
                expected_generation,
                ..
            } => Some(*expected_generation),
            _ => None,
        };
        if generation == Some(0) {
            return Err(error(
                "invalid_request",
                "expected_generation must be positive",
            ));
        }
        Ok(())
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Claim { .. } => "claim",
            Self::ClaimStatus { .. } => "claim_status",
            Self::ReleaseClaim { .. } => "release_claim",
            Self::TransferClaim { .. } => "transfer_claim",
            Self::AcceptClaim { .. } => "accept_claim",
            Self::RenewClaim { .. } => "renew_claim",
            Self::ReplyIntent { .. } => "reply_intent",
        }
    }

    fn request_id(&self) -> Option<&str> {
        match self {
            Self::Claim { request_id, .. }
            | Self::ReleaseClaim { request_id, .. }
            | Self::TransferClaim { request_id, .. }
            | Self::AcceptClaim { request_id, .. }
            | Self::RenewClaim { request_id, .. }
            | Self::ReplyIntent { request_id, .. } => request_id.as_deref(),
            Self::ClaimStatus { request_id } => Some(request_id),
        }
    }

    fn input(&self, context: &OperationContext) -> Value {
        let mut input = match self {
            Self::Claim { external_ref, .. } => json!({"external_ref": external_ref}),
            Self::ClaimStatus { request_id } => json!({"request_id": request_id}),
            Self::ReleaseClaim {
                resource_id,
                expected_generation,
                ..
            }
            | Self::RenewClaim {
                resource_id,
                expected_generation,
                ..
            } => json!({"resource_id": resource_id, "expected_generation": expected_generation}),
            Self::ReplyIntent {
                resource_id,
                expected_generation,
                occurrence_id,
                ..
            } => json!({"resource_id": resource_id, "expected_generation": expected_generation, "occurrence_id": occurrence_id}),
            Self::TransferClaim {
                resource_id,
                expected_generation,
                to_slot_id,
                offer_ttl_secs,
                ..
            } => {
                json!({"resource_id": resource_id, "expected_generation": expected_generation, "to_slot_id": to_slot_id, "offer_ttl_secs": offer_ttl_secs.unwrap_or(600)})
            }
            Self::AcceptClaim {
                handoff_id,
                expected_generation,
                ..
            } => json!({"handoff_id": handoff_id, "expected_generation": expected_generation}),
        };
        input["project_id"] = json!(context.project_id);
        input["actor"] = json!({"registration_id": context.registration_id});
        input["schema"] = json!(CONTRACT_SCHEMA);
        input
    }

    fn wire(&self, request_id: &str, context: &OperationContext) -> Value {
        let mut value = match self {
            Self::Claim { .. } | Self::ClaimStatus { .. } => json!({}),
            Self::ReleaseClaim {
                expected_generation,
                ..
            }
            | Self::TransferClaim {
                expected_generation,
                ..
            }
            | Self::AcceptClaim {
                expected_generation,
                ..
            }
            | Self::RenewClaim {
                expected_generation,
                ..
            }
            | Self::ReplyIntent {
                expected_generation,
                ..
            } => json!({"expected_generation": expected_generation}),
        };
        value["schema"] = json!(CONTRACT_SCHEMA);
        value["request_id"] = json!(request_id);
        value["actor"] = json!({"registration_id": context.registration_id});
        match self {
            Self::Claim { .. } | Self::ClaimStatus { .. } => {}
            Self::ReleaseClaim { .. } => {}
            Self::TransferClaim {
                to_slot_id,
                offer_ttl_secs,
                ..
            } => {
                value["to_slot_id"] = json!(to_slot_id);
                value["offer_ttl_secs"] = json!(offer_ttl_secs.unwrap_or(600));
            }
            Self::AcceptClaim { .. } | Self::RenewClaim { .. } => {}
            Self::ReplyIntent { occurrence_id, .. } => {
                if let Some(occurrence_id) = occurrence_id {
                    value["occurrence_id"] = json!(occurrence_id);
                }
            }
        }
        value
    }

    fn path(&self) -> String {
        match self {
            Self::Claim { .. } | Self::ClaimStatus { .. } => {
                "/api/coordination/external-work/claim".to_string()
            }
            Self::ReleaseClaim { resource_id, .. } => {
                format!("/api/coordination/external-work/{resource_id}/release")
            }
            Self::TransferClaim { resource_id, .. } => {
                format!("/api/coordination/external-work/{resource_id}/handoffs")
            }
            Self::AcceptClaim { handoff_id, .. } => {
                format!("/api/coordination/handoffs/{handoff_id}/accept")
            }
            Self::RenewClaim { resource_id, .. } => {
                format!("/api/coordination/external-work/{resource_id}/renew")
            }
            Self::ReplyIntent { resource_id, .. } => {
                format!("/api/coordination/external-work/{resource_id}/reply-intents")
            }
        }
    }

    fn claim_external_ref(&self) -> Option<&str> {
        match self {
            Self::Claim { external_ref, .. } => Some(external_ref),
            _ => None,
        }
    }
}

/// cas-f869 (GH #1272): the exact `violet_post` fields for a registered reply
/// intent, taken only from Cloud's receipt and checked against the request.
/// `receipt.operation_id` is the registration operation, never the post's
/// `coordination_operation_id`; a receipt that does not match the requested
/// resource and generation yields a refusal instead of a post block. The
/// claim's authoritative `expires_at` is checked against `now` on every call,
/// including a journal replay: an expired, missing or unreadable expiry yields
/// a refusal with the original receipt kept, never a renew or a reclaim.
fn with_reply_ready(mut result: Value, resource_id: &str, expected_generation: u64, now: DateTime<Utc>) -> Value {
    if result["code"] != "completed" {
        return result;
    }
    let receipt = &result["receipt"];
    let intent = &receipt["reply_intent"];
    let block = (|| {
        let intent_id = intent["id"].as_str().filter(|id| Uuid::parse_str(id).is_ok())?;
        let claim_generation = intent["claim_generation"].as_u64()?;
        let resource = &receipt["resource"];
        let checks = intent["status"] == "registered"
            && claim_generation == expected_generation
            && receipt["generation"].as_u64() == Some(claim_generation)
            && resource["id"] == resource_id
            && receipt["operation_id"].as_str() != Some(intent_id);
        if !checks {
            return None;
        }
        Some(json!({
            "channel": resource["external_ref"]["channel_id"].as_str()?,
            "reply_to": resource["external_ref"]["root_ts"].as_str()?,
            "coordination_operation_id": intent_id,
            "expected_generation": claim_generation,
        }))
    })();
    let request_id = result["receipt"]["request_id"].as_str().unwrap_or_default().to_string();
    let refuse = |result: Value, code: &str, reason: &str, guidance: &str, next: &str| {
        let mut refused = failure_response(&request_id, code, reason, guidance, next);
        refused["cloud_response"] = result;
        refused
    };
    if block.is_some() {
        let expires = receipt["expires_at"]
            .as_str()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.with_timezone(&Utc));
        match expires {
            None => {
                return refuse(
                    result,
                    "invalid_response",
                    "claim_expiry_unreadable",
                    "Cloud's reply-intent receipt has no readable claim expires_at, so there is nothing safe to post. Do not post; check claim_status, then register again.",
                    "reconcile",
                );
            }
            Some(expires_at) if expires_at <= now => {
                return refuse(
                    result,
                    "rejected",
                    "claim_expired",
                    &format!(
                        "The claim this reply intent was registered under lapsed at {}. Do not post: renew_claim if the thread is still yours (claim_not_active means claim it again explicitly), then register a new reply intent.",
                        expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    ),
                    "renew_or_reclaim",
                );
            }
            Some(_) => {}
        }
    }
    match block {
        Some(block) => {
            let claim = json!({
                "resource_id": resource_id,
                "generation": receipt["generation"],
                "expires_at": receipt["expires_at"],
            });
            result["violet_post"] = block;
            result["claim"] = claim;
            result["guidance"] = json!(format!(
                "{} Post with violet_post using the violet_post fields here plus your text and one new request_id (reuse that request_id only to retry the same reply). Never use receipt.operation_id as coordination_operation_id; on outcome_unknown reconcile, never post again.",
                result["guidance"].as_str().unwrap_or("Reply intent registered.")
            ));
            result
        }
        None => refuse(
            result,
            "invalid_response",
            "reply_intent_receipt_mismatch",
            "Cloud's reply-intent receipt does not match the requested thread and claim generation, so there is nothing safe to post. Do not post; check claim_status, then register again.",
            "reconcile",
        ),
    }
}

fn valid_request_id(candidate: Option<&str>) -> Result<String, ExternalWorkError> {
    match candidate {
        Some(id) => valid_uuid(id),
        None => Ok(Uuid::new_v4().to_string()),
    }
}

fn valid_uuid(value: &str) -> Result<String, ExternalWorkError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| error("invalid_request", "request_id must be a UUID"))?;
    let canonical = parsed.to_string();
    if value != canonical {
        return Err(error(
            "invalid_request",
            "request_id must be a lowercase canonical UUID",
        ));
    }
    Ok(canonical)
}

fn validate_uuid_field(value: &str, field: &str) -> Result<(), ExternalWorkError> {
    let parsed = Uuid::parse_str(value).map_err(|_| {
        let message = if field == "occurrence_id" {
            "occurrence_id is optional; use a caller-generated UUID, not a Slack message ts; omit it or pass a UUID".to_string()
        } else {
            format!("{field} must be a UUID")
        };
        error("invalid_request", message)
    })?;
    if value != parsed.to_string() {
        return Err(error(
            "invalid_request",
            format!("{field} must be a lowercase canonical UUID"),
        ));
    }
    Ok(())
}

fn call_timeout(deadline: Instant) -> Result<Duration, ExternalWorkError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(error(
            "authority_unavailable",
            "remote wait budget is exhausted",
        ));
    }
    Ok(left.min(CALL_REMOTE_BUDGET))
}

/// cas-b1d0 (GH #1266): Cloud refuses a claim with `coordination_scope_required`
/// / `binding_unverified` when this project has no verified binding on the
/// channel (Cloud deliberately does not say whether the channel is unbound or
/// bound elsewhere), and with `binding_unmanaged` when the binding is not
/// managed. Either way this project has no claim layer there, and supervisors
/// read the bare refusal and its `link_team_project` as "no reply authority".
/// Say what it means instead: no claim exists or is needed, and the reply goes
/// through Violet v1 under Violet's own channel policy. Cloud's guidance and
/// next action are kept beside it. Every other refusal (unlinked project,
/// revoked membership, strict activation, transport) is left untouched.
fn explain_unmanaged(mut result: Value) -> Value {
    let unmanaged = result["code"] == "coordination_scope_required"
        && matches!(result["reason"].as_str(), Some("binding_unverified" | "binding_unmanaged"));
    if !unmanaged {
        return result;
    }
    result["managed"] = json!(false);
    result["cloud_guidance"] = result["guidance"].take();
    result["guidance"] = json!(
        "unmanaged: this project has no managed binding on this Slack channel, so there is no claim to hold \
         and none is needed. Reply normally through Violet v1 (violet_post), within Violet's own channel policy. \
         Binding setup is an operator-only Cloud step, not a reply prerequisite; do not escalate it to reply."
    );
    if result["receipt"].is_object() {
        result["receipt"]["cloud_next_action"] = result["receipt"]["next_action"].take();
        result["receipt"]["next_action"] = json!("reply_via_violet_v1");
    }
    result
}

fn row_response(row: &JournalOperation) -> Result<Value, ExternalWorkError> {
    row.response_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| error("journal_error", format!("saved response is invalid: {e}")))
        .map(|result| {
            let mut response = result.unwrap_or_else(|| {
                failure_response(
                    &row.request_id,
                    "pending",
                    "not_dispatched",
                    "The request is journaled and waiting for dispatch.",
                    "reconcile",
                )
            });
            if let Some(object) = response.as_object_mut() { object.remove("_reply_recovery"); }
            response
        })
}

fn ensure_local_request_receipt(response: &mut Value, request_id: &str) {
    if !response.is_object() {
        *response = json!({});
    }
    if response.get("schema").is_none() {
        response["schema"] = json!(CONTRACT_SCHEMA);
    }
    if !response.get("receipt").is_some_and(Value::is_object) {
        response["receipt"] = json!({});
    }
    if response
        .pointer("/receipt/request_id")
        .is_none_or(Value::is_null)
    {
        response["receipt"]["request_id"] = json!(request_id);
    }
    if response
        .pointer("/receipt/observed_at")
        .is_none_or(Value::is_null)
    {
        response["receipt"]["observed_at"] = json!(Utc::now().to_rfc3339());
    }
    if response
        .pointer("/receipt/next_action")
        .is_none_or(Value::is_null)
    {
        let code = response["code"].as_str().unwrap_or_default();
        response["receipt"]["next_action"] = json!(if matches!(
            code,
            "pending" | "outcome_unknown" | "authority_unavailable"
        ) {
            "reconcile"
        } else {
            "none"
        });
    }
}

fn failure_response(
    request_id: &str,
    code: &str,
    reason: &str,
    guidance: &str,
    next_action: &str,
) -> Value {
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

fn journal_error(journal_error: JournalError) -> ExternalWorkError {
    match journal_error {
        JournalError::ReplyAuthorityRequired => error("managed_reply_required", journal_error.to_string()),
        JournalError::IdempotencyConflict { request_id } => error(
            "idempotency_conflict",
            format!("request_id {request_id} was used with different input"),
        ),
        JournalError::Missing(request_id) => error(
            "not_found",
            format!("no local operation for request_id {request_id}"),
        ),
        JournalError::Storage(message) => error("journal_error", message),
    }
}

fn error(code: &str, message: impl Into<String>) -> ExternalWorkError {
    ExternalWorkError {
        code: code.to_string(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct _ContractAnchor {
    schema: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const REQUEST_ID: &str = "00000001-2222-4222-8222-000000000001";
    const RESOURCE_ID: &str = "6d1f0c3a-2b4e-4f6a-8c9d-0e1f2a3b4c5d";

    struct FakeCloud {
        journal: SqliteRequestJournal,
        posts: AtomicUsize,
        lookups: Mutex<Vec<String>>,
        post_result: Mutex<Result<Value, RemoteError>>,
        lookup_result: Mutex<Result<Value, RemoteError>>,
        posted: Mutex<Vec<(String, Value)>>,
        /// cas-8ccd: the timeout each post was given.
        post_timeouts: Mutex<Vec<Duration>>,
    }

    impl FakeCloud {
        fn new(journal: SqliteRequestJournal) -> Self {
            Self {
                journal,
                posts: AtomicUsize::new(0),
                lookups: Mutex::new(Vec::new()),
                post_result: Mutex::new(Ok(acquired_response(REQUEST_ID))),
                lookup_result: Mutex::new(Ok(acquired_response(REQUEST_ID))),
                posted: Mutex::new(Vec::new()),
                post_timeouts: Mutex::new(Vec::new()),
            }
        }
    }

    impl CloudCoordinationPort for FakeCloud {
        fn post(
            &self,
            path: &str,
            payload: &Value,
            timeout: Duration,
        ) -> Result<Value, RemoteError> {
            self.post_timeouts.lock().unwrap().push(timeout);
            let id = payload
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let row = self
                .journal
                .get(id)
                .expect("journal is readable")
                .expect("request is persisted before Cloud");
            assert!(
                row.wire_json.is_some(),
                "wire request is frozen before dispatch"
            );
            assert_eq!(row.stage, "dispatching");
            assert_eq!(row.attempts, 1);
            self.posts.fetch_add(1, Ordering::SeqCst);
            self.posted
                .lock()
                .unwrap()
                .push((path.to_string(), payload.clone()));
            self.post_result.lock().unwrap().clone()
        }

        fn lookup(
            &self,
            request_id: &str,
            _registration_id: &str,
            _timeout: Duration,
        ) -> Result<Value, RemoteError> {
            self.lookups.lock().unwrap().push(request_id.to_string());
            self.lookup_result.lock().unwrap().clone()
        }
    }

    struct FakeViolet {
        calls: AtomicUsize,
        result: Mutex<Result<Value, RemoteError>>,
    }

    impl FakeViolet {
        fn ok() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                result: Mutex::new(Ok(json!({
                    "schema": CONTRACT_SCHEMA,
                    "code": "completed",
                    "thread_identity": {
                        "provider": "slack", "workspace_id": "T07ABCDEF",
                        "channel_id": "C08COORD01", "root_ts": "1760616000.123456",
                        "identity_proof": {"kind": "violet_installation", "proof_ref": "opaque"}
                    }
                }))),
            }
        }
    }

    impl VioletThreadResolver for FakeViolet {
        fn resolve(&self, _external_ref: &str, _timeout: Duration) -> Result<Value, RemoteError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.lock().unwrap().clone()
        }
    }

    #[test]
    fn identity_claim_read_avoids_digest_size_cap_cas_0288() {
        let args = violet_identity_read_args("https://example.slack.com/archives/C08COORD01/p1760616000123456");
        let receipt = if args.get("include_threads") == Some(&json!(false)) {
            json!({"ok":true,"schema_version":1,"identity":{
                "workspace_id":"T07ABCDEF","channel_id":"C08COORD01","root_thread_ts":"1760616000.123456"}})
        } else { json!({"ok":false,"error":{"code":"size_cap_exceeded"}}) };
        let result = decode_violet_result(json!({"structuredContent":receipt})).unwrap();
        assert_eq!(result["code"], "completed");
        assert_eq!(result["thread_identity"]["root_ts"], "1760616000.123456");
    }

    fn acquired_response(request_id: &str) -> Value {
        json!({
            "schema": CONTRACT_SCHEMA,
            "code": "acquired",
            "guidance": "You own this thread until it is completed, released or handed off.",
            "receipt": {
                "operation_id": "00000b01-1111-4111-8111-000000000b01",
                "request_id": request_id,
                "resource": {"id": RESOURCE_ID, "kind": "slack_thread", "state": "claimed"},
                "visible_owner": {"slot_id": "a1a1a1a1-0000-4000-8000-00000000000a", "registration_id": "reg-supervisor-alpha-01"},
                "generation": 1,
                "expires_at": "2026-10-16T14:05:00.000Z",
                "observed_at": "2026-10-16T14:00:00.000Z",
                "next_action": "proceed"
            }
        })
    }

    fn service(
        cloud: Arc<FakeCloud>,
        violet: Arc<FakeViolet>,
        journal: SqliteRequestJournal,
    ) -> ExternalWorkService {
        ExternalWorkService::new(
            journal,
            cloud,
            violet,
            OperationContext {
                project_id: "github.com/richards-llc/cassy".into(),
                registration_id: "reg-supervisor-alpha-01".into(),
            },
        )
    }

    fn journal() -> SqliteRequestJournal {
        SqliteRequestJournal::open_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn fixture(name: &str) -> Value {
        let source = match name {
            "claim" => include_str!(
                "../../tests/fixtures/coordination_contract/v1/external_work--claim.json"
            ),
            "release" => include_str!(
                "../../tests/fixtures/coordination_contract/v1/external_work--release.json"
            ),
            "handoff" => include_str!(
                "../../tests/fixtures/coordination_contract/v1/external_work--handoff_offer.json"
            ),
            "accept" => {
                include_str!("../../tests/fixtures/coordination_contract/v1/handoffs--accept.json")
            }
            "renew" => include_str!(
                "../../tests/fixtures/coordination_contract/v1/external_work--renew.json"
            ),
            "reply_intents" => include_str!(
                "../../tests/fixtures/coordination_contract/v1/external_work--reply_intents.json"
            ),
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(source).unwrap()
    }

    #[test]
    fn violet_resolver_decodes_the_deployed_identity_only_contract_fixture() {
        let capture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/coordination_contract/v1/captured/violet-production-2026-10-16.json"
        ))
        .unwrap();
        let cases = capture["cases"].as_array().unwrap();

        let from_identity_object = decode_violet_result(cases[0]["response"].clone()).unwrap();
        let from_permalink = decode_violet_result(cases[1]["response"].clone()).unwrap();
        let expected_identity = json!({
            "schema": CONTRACT_SCHEMA,
            "code": "completed",
            "thread_identity": {
                "provider": "slack",
                "workspace_id": "T011K7W2FK8",
                "channel_id": "C0B44GUKDK2",
                "root_ts": "1792161452.497159",
            }
        });
        assert_eq!(from_identity_object, expected_identity);
        assert_eq!(from_permalink, expected_identity);

        let foreign_workspace = decode_violet_result(cases[2]["response"].clone()).unwrap();
        assert_eq!(foreign_workspace["code"], "foreign_workspace");
        assert_ne!(foreign_workspace["code"], "completed");
    }

    fn claim(request_id: &str, external_ref: &str) -> ExternalWorkCommand {
        ExternalWorkCommand::Claim {
            external_ref: external_ref.to_string(),
            request_id: Some(request_id.to_string()),
        }
    }

    /// Cloud owns dead/expired eligibility. The client must read back the
    /// named intent, then acquire with a new id, never dispatch another reply.
    #[test]
    fn dead_expired_reply_readback_reclaims_without_duplicate_and_live_holder_blocks_cas_f039() {
        struct RecoveryCloud {
            journal: SqliteRequestJournal,
            settled: Arc<Mutex<bool>>,
            held: Value,
            acquired: Value,
            timeout: bool,
            posts: Mutex<Vec<Value>>,
            lookups: Mutex<Vec<String>>,
        }
        impl CloudCoordinationPort for RecoveryCloud {
            fn post(&self, path: &str, payload: &Value, _: Duration) -> Result<Value, RemoteError> {
                assert_eq!(path, "/api/coordination/external-work/claim");
                let id = payload["request_id"].as_str().unwrap();
                assert!(self.journal.get(id).unwrap().unwrap().wire_json.is_some());
                self.posts.lock().unwrap().push(payload.clone());
                if self.timeout && *self.settled.lock().unwrap() {
                    return Err(RemoteError::timeout("committed, response lost"));
                }
                let mut result = if *self.settled.lock().unwrap() {
                    self.acquired.clone()
                } else {
                    self.held.clone()
                };
                result["receipt"]["request_id"] = payload["request_id"].clone();
                Ok(result)
            }
            fn lookup(&self, id: &str, _: &str, _: Duration) -> Result<Value, RemoteError> {
                assert!(self.timeout);
                assert_ne!(id, REQUEST_ID);
                assert_eq!(self.posts.lock().unwrap()[1]["request_id"], id);
                self.lookups.lock().unwrap().push(id.to_string());
                let mut result = self.acquired.clone();
                result["receipt"]["request_id"] = json!(id);
                Ok(result)
            }
        }
        struct RecoveryViolet {
            settled: Arc<Mutex<bool>>,
            outcome: &'static str,
            reads: Mutex<Vec<String>>,
        }
        impl VioletThreadResolver for RecoveryViolet {
            fn resolve(&self, link: &str, budget: Duration) -> Result<Value, RemoteError> {
                FakeViolet::ok().resolve(link, budget)
            }
            fn reconcile_reply(&self, intent_id: &str, _: Duration) -> Result<Value, RemoteError> {
                let mut reads = self.reads.lock().unwrap();
                reads.push(intent_id.to_string());
                if self.outcome == "readback_timeout" {
                    return Err(RemoteError::timeout("read failed"));
                }
                let settled = self.outcome != "unknown" || reads.len() > 1;
                *self.settled.lock().unwrap() = settled;
                let already_settled = self.outcome.starts_with("settled_");
                let state = if !settled {
                    "outcome_unknown"
                } else if self.outcome.ends_with("rejected") {
                    "rejected"
                } else {
                    "posted"
                };
                let mut reply = json!({"ok": !already_settled && settled,
                    "state": state, "coordination_operation_id": intent_id,
                    "message_ts": if state == "posted" && !already_settled { json!("1760616010.000001") } else { Value::Null },
                    "reason": if already_settled { "intent_not_unresolved" } else { "retry_later" },
                    "retry_after": 120});
                match self.outcome {
                    "wrong_intent" => reply["coordination_operation_id"] = json!(REQUEST_ID),
                    "wrong_thread" => reply["thread_ts"] = json!("1760616011.000001"),
                    "wrong_channel" => reply["channel_id"] = json!("COTHER"),
                    "missing_ts" => reply["message_ts"] = Value::Null,
                    _ => {}
                }
                // Both MCP envelope forms use the production readback decoder.
                violet_payload(if already_settled {
                    json!({"content":[{"type":"text","text":reply.to_string()}]})
                } else {
                    json!({"structuredContent":reply})
                })
            }
        }
        let cases = fixture("claim")["cases"].as_array().unwrap().clone();
        let payload = |code: &str, reason: Option<&str>| {
            cases
                .iter()
                .map(|case| &case["response"]["payload"])
                .find(|value| {
                    value["code"] == code && reason.is_none_or(|reason| value["reason"] == reason)
                })
                .unwrap()
                .clone()
        };
        let unresolved = payload("held", Some("reply_unresolved"));
        let intent = unresolved["receipt"]["reply_intent"]["id"]
            .as_str()
            .unwrap();
        let acquired = cases
            .iter()
            .map(|case| &case["response"]["payload"])
            .find(|value| value["code"] == "acquired" && value["receipt"]["generation"] == 2)
            .unwrap()
            .clone();
        for outcome in [
            "posted",
            "rejected",
            "settled_posted",
            "settled_rejected",
            "unknown",
            "live",
            "wrong_intent",
            "wrong_thread",
            "wrong_channel",
            "missing_ts",
            "wrong_generation",
            "unexpired",
            "wrong_scope",
            "readback_timeout",
            "claim_timeout",
        ] {
            let journal = journal();
            let foreign_id = "000000f0-2222-4222-8222-0000000000f0";
            if outcome == "unknown" {
                // An older operation from a departed registration cannot
                // starve this caller's readback retry or acquire its authority.
                journal.prepare(foreign_id, "claim", &json!({
                    "external_ref":"https://workspace.slack.com/archives/C08COORD01/p1760616000123456",
                    "actor":{"registration_id":"reg-departed-supervisor"}
                })).unwrap();
            }
            let settled = Arc::new(Mutex::new(false));
            let mut held = if outcome == "live" {
                payload("held", Some("owner_holds_claim"))
            } else {
                unresolved.clone()
            };
            match outcome {
                "wrong_generation" => {
                    held["receipt"]["reply_intent"]["claim_generation"] = json!(2)
                }
                "unexpired" => held["receipt"]["expires_at"] = json!("2026-10-17T14:00:00.000Z"),
                "wrong_scope" => {
                    held["receipt"]["resource"]["external_ref"]["channel_id"] = json!("COTHER")
                }
                _ => {}
            }
            let cloud = Arc::new(RecoveryCloud {
                journal: journal.clone(),
                settled: settled.clone(),
                held,
                acquired: acquired.clone(),
                timeout: outcome == "claim_timeout",
                posts: Mutex::new(Vec::new()),
                lookups: Mutex::new(Vec::new()),
            });
            let violet = Arc::new(RecoveryViolet {
                settled,
                outcome,
                reads: Mutex::new(Vec::new()),
            });
            let service = ExternalWorkService::new(
                journal.clone(),
                cloud.clone(),
                violet.clone(),
                OperationContext {
                    project_id: "github.com/richards-llc/cassy".into(),
                    registration_id: "reg-supervisor-beta-07".into(),
                },
            );
            let command = claim(
                REQUEST_ID,
                "https://workspace.slack.com/archives/C08COORD01/p1760616000123456",
            );
            let started_ms = Utc::now().timestamp_millis();
            let first = service.execute(command.clone()).unwrap();
            let terminal = matches!(
                outcome,
                "posted" | "rejected" | "settled_posted" | "settled_rejected" | "claim_timeout"
            );
            assert_eq!(
                first["code"],
                if outcome == "claim_timeout" {
                    "outcome_unknown"
                } else if terminal {
                    "acquired"
                } else {
                    "held"
                },
                "{outcome}: {first}"
            );
            // Reconstruct the service against the same durable journal: no new
            // request or duplicate mutation may appear after a process restart.
            let restarted = ExternalWorkService::new(
                journal.clone(),
                cloud.clone(),
                violet.clone(),
                service.context.clone(),
            );
            let result = if outcome == "claim_timeout" {
                restarted.claim_status(REQUEST_ID).unwrap()
            } else {
                first.clone()
            };
            assert_eq!(
                restarted.execute(command).unwrap(),
                result,
                "{outcome}: replay changes its outcome"
            );
            {
                let posts = cloud.posts.lock().unwrap();
                assert_eq!(posts.len(), if terminal { 2 } else { 1 }, "{outcome}");
                if terminal {
                    assert_ne!(posts[0]["request_id"], posts[1]["request_id"]);
                    assert_eq!(posts[0]["external_ref"], posts[1]["external_ref"]);
                    assert_eq!(posts[0]["actor"], posts[1]["actor"]);
                    assert_eq!(result["receipt"]["generation"], 2);
                    assert_eq!(*violet.reads.lock().unwrap(), vec![intent.to_string()]);
                } else if matches!(
                    outcome,
                    "live" | "wrong_generation" | "unexpired" | "wrong_scope"
                ) {
                    assert!(violet.reads.lock().unwrap().is_empty());
                } else {
                    assert_eq!(violet.reads.lock().unwrap().len(), 1);
                }
            }
            assert_eq!(
                cloud.lookups.lock().unwrap().len(),
                usize::from(outcome == "claim_timeout")
            );
            if outcome == "unknown" {
                let waiting = journal.get(REQUEST_ID).unwrap().unwrap();
                assert_eq!(waiting.stage, "retry_wait");
                assert!(waiting.next_retry_ms.unwrap() >= started_ms + 120_000);
                journal
                    .record_result(REQUEST_ID, &unresolved, None)
                    .unwrap();
                assert_eq!(journal.get(REQUEST_ID).unwrap().unwrap(), waiting);
                // Inject a due schedule; no sleep or wall-clock race.
                journal
                    .conn
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE external_work_operations SET next_retry_ms=0 WHERE request_id=?1",
                        [REQUEST_ID],
                    )
                    .unwrap();
                let results = restarted
                    .reconcile_due(Utc::now().timestamp_millis())
                    .unwrap();
                assert_eq!(results.len(), 1);
                assert_eq!(results[0]["code"], "acquired");
                assert_eq!(journal.get(foreign_id).unwrap().unwrap().stage, "prepared");
                assert_eq!(cloud.posts.lock().unwrap().len(), 2);
                assert_eq!(violet.reads.lock().unwrap().len(), 2);
                journal
                    .record_result(REQUEST_ID, &unresolved, None)
                    .unwrap();
                journal
                    .record_reply_recovery(REQUEST_ID, &unresolved, Some(Duration::from_secs(2)))
                    .unwrap();
                assert_eq!(
                    restarted.claim_status(REQUEST_ID).unwrap()["code"],
                    "acquired"
                );
            }
        }
    }

    /// Violet results returned in order, then the last one forever.
    struct SequenceViolet {
        calls: AtomicUsize,
        results: Mutex<Vec<Result<Value, RemoteError>>>,
        delay: Duration,
    }

    impl VioletThreadResolver for SequenceViolet {
        fn resolve(&self, _external_ref: &str, _timeout: Duration) -> Result<Value, RemoteError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let mut results = self.results.lock().unwrap();
            if results.len() > 1 { results.remove(0) } else { results[0].clone() }
        }
    }

    /// cas-5fe0 (GH #1292): a dropped Violet upstream that reconnects inside
    /// the window lets the claim succeed with no manual retry; a persistent
    /// drop fails clearly with nothing sent to Cloud; any other Violet
    /// failure is not retried.
    #[test]
    fn claim_retries_violet_through_a_reconnect_and_fails_clearly_when_it_stays_down_cas_5fe0() {
        let link = "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456";
        let identity = FakeViolet::ok().result.into_inner().unwrap();
        let dropped = || Err(RemoteError::reconnecting("tool call 'violet_read' on 'violet' failed: Transport closed"));
        for (results, expect_code, expect_calls, expect_posts) in [
            (vec![dropped(), dropped(), identity.clone()], "acquired", 3, 1),
            (vec![dropped()], "authority_unavailable", 0, 0),
            (vec![Err(RemoteError::unavailable("no MCP proxy is configured"))], "authority_unavailable", 1, 0),
        ] {
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            let violet = Arc::new(SequenceViolet { calls: AtomicUsize::new(0), results: Mutex::new(results), delay: Duration::ZERO });
            let mut service = service(cloud.clone(), Arc::new(FakeViolet::ok()), journal);
            service.violet = violet.clone();
            service.violet_reconnect_window = Duration::from_millis(120);
            let result = service.execute(claim(REQUEST_ID, link)).unwrap();
            assert_eq!(result["code"], expect_code, "{result}");
            assert_eq!(cloud.posts.load(Ordering::SeqCst), expect_posts);
            let calls = violet.calls.load(Ordering::SeqCst);
            if expect_calls == 0 {
                assert!(calls > 1, "a persistent drop is retried within the window");
                assert!(result["guidance"].as_str().unwrap().contains("after retrying"), "{result}");
            } else {
                assert_eq!(calls, expect_calls);
            }
        }
    }

    /// cas-b1d0 (GH #1266): Cloud's unverified-binding claim refusal reads as
    /// "unmanaged, reply through Violet v1" (claim and claim_status alike),
    /// with Cloud's own guidance kept; an unlinked project stays a bare refusal.
    #[test]
    fn unmanaged_channel_claim_says_reply_via_violet_v1_and_other_refusals_stay_closed_cas_b1d0() {
        let link = "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456";
        for (case, unmanaged) in [("ambiguous or unmanaged binding", true), ("unlinked personal project", false)] {
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            let refusal = case_response("claim", case);
            *cloud.post_result.lock().unwrap() = Ok(refusal.clone());
            let service = service(cloud, Arc::new(FakeViolet::ok()), journal);
            let claimed = service.execute(claim(REQUEST_ID, link)).unwrap();
            let status = service.execute(ExternalWorkCommand::ClaimStatus { request_id: REQUEST_ID.into() }).unwrap();
            for result in [claimed, status] {
                assert_eq!(result["code"], refusal["code"], "{case}");
                if unmanaged {
                    assert_eq!(result["managed"], false);
                    assert_eq!(result["receipt"]["next_action"], "reply_via_violet_v1");
                    assert_eq!(result["receipt"]["cloud_next_action"], "link_team_project");
                    assert_eq!(result["cloud_guidance"], refusal["guidance"]);
                    assert!(result["guidance"].as_str().unwrap().contains("Violet v1"), "{result}");
                } else {
                    assert_eq!(result["guidance"], refusal["guidance"], "{case}");
                    assert_eq!(result["receipt"]["next_action"], refusal["receipt"]["next_action"]);
                    assert!(result.get("managed").is_none());
                }
            }
        }
    }

    /// cas-8ccd (1): a first-try Violet read spends the request's one remote
    /// budget, so Cloud gets only what is left of it; only a read that waited
    /// out a reconnect hands Cloud a fresh budget.
    #[test]
    fn claim_budget_is_shared_unless_a_reconnect_wait_happened_cas_8ccd() {
        let link = "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456";
        let identity = FakeViolet::ok().result.into_inner().unwrap();
        let dropped = || Err(RemoteError::reconnecting("Transport closed"));
        let budget = Duration::from_millis(600);
        let delay = Duration::from_millis(250);
        for (results, fresh) in [(vec![identity.clone()], false), (vec![dropped(), identity.clone()], true)] {
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            let mut service = service(cloud.clone(), Arc::new(FakeViolet::ok()), journal);
            service.violet = Arc::new(SequenceViolet { calls: AtomicUsize::new(0), results: Mutex::new(results), delay });
            service.total_budget = budget;
            service.violet_reconnect_window = Duration::from_millis(400);
            assert_eq!(service.execute(claim(REQUEST_ID, link)).unwrap()["code"], "acquired");
            let given = cloud.post_timeouts.lock().unwrap()[0];
            if fresh {
                assert!(given > budget - Duration::from_millis(100), "after a reconnect wait Cloud gets a fresh budget: {given:?}");
            } else {
                assert!(given <= budget - delay, "a first-try read spends the shared budget: {given:?}");
            }
        }
    }

    /// cas-8ccd (2): the reconcile lease outlives the worst-case operation, so
    /// a second pickup inside it finds nothing.
    #[test]
    fn reconcile_lease_covers_the_worst_case_run_cas_8ccd() {
        assert!(RECONCILE_LEASE > VIOLET_RECONNECT_WINDOW + TOTAL_REMOTE_BUDGET);
        let journal = journal();
        journal.prepare(REQUEST_ID, "claim", &json!({"external_ref": "x"})).unwrap();
        let now = 1_000_000;
        assert_eq!(journal.lease_due(RECONCILE_BATCH_LIMIT, now, RECONCILE_LEASE).unwrap().len(), 1);
        let worst = (VIOLET_RECONNECT_WINDOW + TOTAL_REMOTE_BUDGET).as_millis() as i64;
        assert!(journal.lease_due(RECONCILE_BATCH_LIMIT, now + worst, RECONCILE_LEASE).unwrap().is_empty());
        let expiry = now + RECONCILE_LEASE.as_millis() as i64;
        assert_eq!(journal.lease_due(RECONCILE_BATCH_LIMIT, expiry, RECONCILE_LEASE).unwrap().len(), 1);
    }

    /// A resolver that classifies through the shared proxy mapping: proxied
    /// transport drops first, then Violet's real answer.
    #[cfg(feature = "mcp-proxy")]
    struct ProxiedDrops {
        calls: AtomicUsize,
        drops: usize,
    }

    #[cfg(feature = "mcp-proxy")]
    impl VioletThreadResolver for ProxiedDrops {
        fn resolve(&self, _external_ref: &str, _timeout: Duration) -> Result<Value, RemoteError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let outcome = if call < self.drops {
                Ok(Err(anyhow::Error::from(rmcp::service::ServiceError::TransportClosed)
                    .context("tool call 'violet_read' on 'violet' failed")))
            } else {
                let capture: Value = serde_json::from_str(include_str!(
                    "../../tests/fixtures/coordination_contract/v1/captured/violet-production-2026-10-16.json"
                ))
                .unwrap();
                Ok(Ok(capture["cases"][0]["response"].clone()))
            };
            violet_proxy_outcome(outcome)
        }
    }

    /// cas-85c1 (GH #1292): a daemon reconcile of a journaled claim retries a
    /// dropped upstream within the window and succeeds; a drop that persists
    /// fails clearly and posts nothing to Cloud. The resolver is a test double
    /// that feeds proxied rmcp errors through `violet_proxy_outcome`, the
    /// mapping every production resolver uses (see the guard test below).
    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn daemon_reconcile_retries_a_proxied_violet_drop_cas_85c1() {
        let link = "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456";
        for (drops, succeeds) in [(2, true), (usize::MAX, false)] {
            let journal = journal();
            let context = OperationContext {
                project_id: "github.com/richards-llc/cassy".into(),
                registration_id: "reg-supervisor-alpha-01".into(),
            };
            journal.prepare(REQUEST_ID, "claim", &claim(REQUEST_ID, link).input(&context)).unwrap();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            let violet = Arc::new(ProxiedDrops { calls: AtomicUsize::new(0), drops });
            let mut service = ExternalWorkService::new(journal, cloud.clone(), violet.clone(), context);
            service.violet_reconnect_window = Duration::from_millis(150);
            let outcome = service.reconcile_due(chrono::Utc::now().timestamp_millis());
            if succeeds {
                assert_eq!(outcome.unwrap()[0]["code"], "acquired");
                assert_eq!(violet.calls.load(Ordering::SeqCst), 3);
                assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
            } else {
                let error = outcome.unwrap_err().to_string();

                assert!(violet.calls.load(Ordering::SeqCst) > 1);
                assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
            }
        }
    }

    /// cas-85c1: every production Violet resolver maps proxied outcomes
    /// through `violet_proxy_outcome`, so the CLI, MCP and daemon paths all
    /// retry a reconnecting upstream. A new copy of the old mapping fails this.
    #[test]
    fn every_violet_resolver_uses_the_shared_proxy_mapping_cas_85c1() {
        // pin: the resolvers wrap a live ProxyEngine that a unit test cannot
        // drive into a reconnecting drop, so the shared-mapping contract is
        // checked on their source; the behaviour is proven by the daemon test.
        for (path, source) in [
            ("cli/coordination.rs", include_str!("../cli/coordination.rs")), // pin: CLI resolver
            ("mcp/tools/service/coordination_external_work.rs", include_str!("../mcp/tools/service/coordination_external_work.rs")), // pin: MCP resolver
            ("ui/factory/daemon/runtime/external_work_reconcile_jobs.rs", include_str!("../ui/factory/daemon/runtime/external_work_reconcile_jobs.rs")), // pin: daemon resolver
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            assert!(production.contains("violet_proxy_outcome(result)"), "{path} must use the shared mapping");
            assert!(
                !production.contains("Ok(Ok(value)) => decode_violet_result(value)"),
                "{path} keeps its own copy of the proxied-outcome mapping"
            );
        }
    }

    #[test]
    fn request_is_durable_before_dispatch_and_receipt_is_returned() {
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        let violet = Arc::new(FakeViolet::ok());
        let result = service(cloud.clone(), violet, journal.clone())
            .execute(claim(
                REQUEST_ID,
                "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456",
            ))
            .unwrap();
        assert_eq!(result["code"], "acquired");
        assert_eq!(
            result
                .pointer("/receipt/request_id")
                .and_then(Value::as_str),
            Some(REQUEST_ID)
        );
        assert_eq!(
            result
                .pointer("/receipt/generation")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
        assert_eq!(journal.get(REQUEST_ID).unwrap().unwrap().stage, "terminal");
        let expected = fixture("claim");
        let posted = cloud.posted.lock().unwrap();
        assert_eq!(posted[0].0, expected["path"]);
        assert_eq!(posted[0].1, expected["cases"][0]["request"]["payload"]);
    }

    #[test]
    fn changed_input_with_same_request_id_is_rejected_before_any_second_effect() {
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        let violet = Arc::new(FakeViolet::ok());
        let service = service(cloud.clone(), violet.clone(), journal);
        service
            .execute(claim(
                REQUEST_ID,
                "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456",
            ))
            .unwrap();
        let result = service
            .execute(claim(
                REQUEST_ID,
                "https://richards-llc.slack.com/archives/C08COORD01/p1760617000000001",
            ))
            .unwrap();
        assert_eq!(result["code"], "idempotency_conflict");
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
        assert_eq!(violet.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn commit_then_timeout_reconciles_by_the_original_request_id_without_reposting() {
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        *cloud.post_result.lock().unwrap() =
            Err(RemoteError::timeout("timed out after Cloud committed"));
        let violet = Arc::new(FakeViolet::ok());
        let service = service(cloud.clone(), violet, journal);
        let first = service
            .execute(claim(
                REQUEST_ID,
                "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456",
            ))
            .unwrap();
        assert_eq!(first["code"], "outcome_unknown");
        let second = service
            .execute(claim(
                REQUEST_ID,
                "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456",
            ))
            .unwrap();
        assert_eq!(second["code"], "acquired");
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
        assert_eq!(*cloud.lookups.lock().unwrap(), vec![REQUEST_ID]);
    }

    #[test]
    fn claim_status_completes_a_journaled_claim_that_never_reached_cloud() {
        let journal = journal();
        let context = OperationContext {
            project_id: "github.com/richards-llc/cassy".into(),
            registration_id: "reg-supervisor-alpha-01".into(),
        };
        let original = claim(
            REQUEST_ID,
            "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456",
        );
        journal
            .prepare(REQUEST_ID, "claim", &original.input(&context))
            .unwrap();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        let violet = Arc::new(FakeViolet::ok());
        let service =
            ExternalWorkService::new(journal.clone(), cloud.clone(), violet.clone(), context);
        let result = service
            .execute(ExternalWorkCommand::ClaimStatus {
                request_id: REQUEST_ID.into(),
            })
            .unwrap();
        assert_eq!(result["code"], "acquired");
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
        assert_eq!(cloud.lookups.lock().unwrap().len(), 0);
        assert_eq!(violet.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn release_forwards_the_expected_generation_and_preserves_stale_result() {
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        *cloud.post_result.lock().unwrap() = Ok(json!({
            "schema": CONTRACT_SCHEMA,
            "code": "stale_generation",
            "guidance": "A newer owner holds generation 2; this release does nothing.",
            "receipt": {"request_id": REQUEST_ID, "generation": 2, "next_action": "none"}
        }));
        let service = service(cloud.clone(), Arc::new(FakeViolet::ok()), journal);
        let result = service
            .execute(ExternalWorkCommand::ReleaseClaim {
                resource_id: RESOURCE_ID.into(),
                expected_generation: 1,
                request_id: Some(REQUEST_ID.into()),
            })
            .unwrap();
        assert_eq!(result["code"], "stale_generation");
        let posted = cloud.posted.lock().unwrap();
        assert!(posted[0].0.ends_with(&format!("/{RESOURCE_ID}/release")));
        assert_eq!(posted[0].1["expected_generation"], 1);
    }

    #[test]
    fn mutation_routes_and_bodies_match_the_v1_fixtures() {
        let cases = [
            (
                "release",
                ExternalWorkCommand::ReleaseClaim {
                    resource_id: RESOURCE_ID.into(),
                    expected_generation: 1,
                    request_id: Some("00000021-2222-4222-8222-000000000021".into()),
                },
            ),
            (
                "handoff",
                ExternalWorkCommand::TransferClaim {
                    resource_id: RESOURCE_ID.into(),
                    expected_generation: 1,
                    to_slot_id: "b2b2b2b2-0000-4000-8000-00000000000b".into(),
                    offer_ttl_secs: Some(600),
                    request_id: Some("00000041-2222-4222-8222-000000000041".into()),
                },
            ),
            (
                "accept",
                ExternalWorkCommand::AcceptClaim {
                    handoff_id: "00004a0f-1111-4111-8111-000000004a0f".into(),
                    expected_generation: 1,
                    request_id: Some("00000051-2222-4222-8222-000000000051".into()),
                },
            ),
        ];
        for (name, command) in cases {
            let fixture = fixture(name);
            let request = &fixture["cases"][0]["request"];
            let registration_id = if name == "accept" {
                "reg-supervisor-beta-07"
            } else {
                "reg-supervisor-alpha-01"
            };
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            let service = ExternalWorkService::new(
                journal,
                cloud.clone(),
                Arc::new(FakeViolet::ok()),
                OperationContext {
                    project_id: "github.com/richards-llc/cassy".into(),
                    registration_id: registration_id.into(),
                },
            );
            service.execute(command).unwrap();
            let posted = cloud.posted.lock().unwrap();
            let path = if name == "accept" {
                fixture["path"]
                    .as_str()
                    .unwrap()
                    .replace("{id}", request["path_params"]["id"].as_str().unwrap())
            } else {
                fixture["path"]
                    .as_str()
                    .unwrap()
                    .replace("{id}", request["path_params"]["id"].as_str().unwrap())
            };
            assert_eq!(posted[0].0, path, "{name} route differs from fixture");
            assert_eq!(
                posted[0].1, request["payload"],
                "{name} body differs from fixture"
            );
        }
    }

    #[test]
    fn violet_outage_fails_closed_without_cloud_mutation() {
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        let violet = Arc::new(FakeViolet::ok());
        *violet.result.lock().unwrap() = Err(RemoteError::unavailable("Violet is offline"));
        let result = service(cloud.clone(), violet.clone(), journal.clone())
            .execute(claim(
                REQUEST_ID,
                "https://richards-llc.slack.com/archives/C08COORD01/p1760616000123456",
            ))
            .unwrap();
        assert_eq!(result["code"], "authority_unavailable");
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
        assert_eq!(
            journal.get(REQUEST_ID).unwrap().unwrap().stage,
            "retry_wait"
        );
    }

    /// cas-f869: the named fixture case's response payload.
    fn case_response(name: &str, case: &str) -> Value {
        fixture(name)["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == case)
            .unwrap_or_else(|| panic!("no {name} case {case}"))["response"]["payload"]
            .clone()
    }

    /// The vendored receipts' claim expires 2026-10-16T14:05:00Z.
    fn before_expiry() -> DateTime<Utc> {
        "2026-10-16T14:02:00Z".parse().unwrap()
    }

    fn at_expiry() -> DateTime<Utc> {
        "2026-10-16T14:05:00Z".parse().unwrap()
    }

    fn reply_intent(request_id: &str, occurrence_id: Option<&str>) -> ExternalWorkCommand {
        ExternalWorkCommand::ReplyIntent {
            resource_id: RESOURCE_ID.into(),
            expected_generation: 1,
            occurrence_id: occurrence_id.map(str::to_string),
            request_id: Some(request_id.into()),
        }
    }

    #[test]
    fn reply_intent_explains_occurrence_id_slack_timestamp_cas_be34() {
        let journal = journal();
        let service = service(
            Arc::new(FakeCloud::new(journal.clone())),
            Arc::new(FakeViolet::ok()),
            journal,
        );

        let error = service
            .execute(reply_intent(REQUEST_ID, Some("1760616000.123456")))
            .unwrap_err();

    }

    /// cas-f869 (GH #1272): reply_intent sends Cloud's exact wire request and
    /// hands back violet_post fields built from receipt.reply_intent.id and
    /// its claim generation, never receipt.operation_id. A replay of the same
    /// request answers from the journal without a second Cloud call.
    #[test]
    fn reply_intent_yields_violet_post_fields_from_the_intent_not_the_operation_cas_f869() {
        let expected = fixture("reply_intents");
        let case = &expected["cases"][0];
        let request_id = case["request"]["payload"]["request_id"].as_str().unwrap();
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        *cloud.post_result.lock().unwrap() = Ok(case["response"]["payload"].clone());
        let mut service = service(cloud.clone(), Arc::new(FakeViolet::ok()), journal);
        service.now = before_expiry;
        let command = || reply_intent(request_id, Some("00000cc2-1111-4111-8111-000000000cc2"));

        let first = service.execute(command()).unwrap();
        let replay = service.execute(command()).unwrap();

        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1, "a replay never re-registers");
        let posted = cloud.posted.lock().unwrap();
        assert_eq!(posted[0].0, expected["path"].as_str().unwrap().replace("{id}", RESOURCE_ID));
        assert_eq!(posted[0].1, case["request"]["payload"]);
        for result in [&first, &replay] {
            assert_eq!(
                result["violet_post"],
                json!({
                    "channel": "C08COORD01",
                    "reply_to": "1760616000.123456",
                    "coordination_operation_id": "00007e91-1111-4111-8111-000000007e91",
                    "expected_generation": 1,
                })
            );
            assert_ne!(result["violet_post"]["coordination_operation_id"], result["receipt"]["operation_id"]);
            assert_eq!(result["claim"]["expires_at"], "2026-10-16T14:05:00.000Z");
        }

        // The same journaled intent replayed once its claim has lapsed: no post
        // fields, the original receipt kept, and no renew or claim call.
        service.now = at_expiry;
        let expired = service.execute(command()).unwrap();
        assert_eq!((expired["code"].as_str(), expired["reason"].as_str()), (Some("rejected"), Some("claim_expired")));
        assert_eq!(expired["receipt"]["next_action"], "renew_or_reclaim");
        assert!(expired.get("violet_post").is_none());
        assert_eq!(expired["cloud_response"]["receipt"]["reply_intent"]["id"], "00007e91-1111-4111-8111-000000007e91");
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1, "no renew, claim or re-registration");
    }

    /// cas-f869: occurrence_id is optional, so the wire omits it; a receipt
    /// that does not match the requested claim yields no post fields.
    #[test]
    fn reply_intent_without_occurrence_and_mismatched_receipts_cas_f869() {
        let registered = case_response("reply_intents", "registered");
        let first_journal = journal();
        let cloud = Arc::new(FakeCloud::new(first_journal.clone()));
        *cloud.post_result.lock().unwrap() = Ok(registered.clone());
        let mut first = service(cloud.clone(), Arc::new(FakeViolet::ok()), first_journal);
        first.now = before_expiry;
        let result = first
            .execute(reply_intent("00000071-2222-4222-8222-000000000071", None))
            .unwrap();
        assert!(cloud.posted.lock().unwrap()[0].1.get("occurrence_id").is_none());
        assert!(result["violet_post"].is_object());

        let mutate = |pointer: &str, value: Value| {
            let mut response = registered.clone();
            *response.pointer_mut(pointer).unwrap() = value;
            response
        };
        for (n, response) in [
            mutate("/receipt/reply_intent/claim_generation", json!(2)),
            mutate("/receipt/resource/id", json!("7e1f0c3a-2b4e-4f6a-8c9d-0e1f2a3b4c5d")),
            mutate("/receipt/reply_intent/id", json!("00000b61-1111-4111-8111-000000000b61")),
            mutate("/receipt/reply_intent/status", json!("rejected")),
        ]
        .into_iter()
        .enumerate()
        {
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            *cloud.post_result.lock().unwrap() = Ok(response);
            let result = service(cloud, Arc::new(FakeViolet::ok()), journal)
                .execute(reply_intent(&format!("00000072-2222-4222-8222-00000000007{n}"), None))
                .unwrap();
            assert_eq!(result["code"], "invalid_response", "case {n}");
            assert_eq!(result["reason"], "reply_intent_receipt_mismatch");
            assert!(result.get("violet_post").is_none(), "case {n}: nothing safe to post");
        }

        // A receipt whose claim expiry is missing or unreadable is refused.
        for (n, expires_at) in [Value::Null, json!("not-a-time")].into_iter().enumerate() {
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            *cloud.post_result.lock().unwrap() = Ok(mutate("/receipt/expires_at", expires_at));
            let mut service = service(cloud, Arc::new(FakeViolet::ok()), journal);
            service.now = before_expiry;
            let result = service
                .execute(reply_intent(&format!("00000074-2222-4222-8222-00000000007{n}"), None))
                .unwrap();
            assert_eq!((result["code"].as_str(), result["reason"].as_str()), (Some("invalid_response"), Some("claim_expiry_unreadable")));
            assert!(result.get("violet_post").is_none());
        }
    }

    /// cas-8239 (GH #1300): `cas violet post` takes a managed reply's claim
    /// fields from the caller's own live reply intent in the journal. Another
    /// session's intent, a lapsed claim or a released claim is refused locally;
    /// unknown threads retain ordinary Violet posting.
    #[test]
    fn current_reply_post_is_only_the_callers_live_intent_cas_8239() {
        let case = &fixture("reply_intents")["cases"][0];
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        let mut service = service(cloud.clone(), Arc::new(FakeViolet::ok()), journal.clone());
        service.now = before_expiry;
        let (me, root) = ("reg-supervisor-alpha-01", "1760616000.123456");
        *cloud.post_result.lock().unwrap() = Ok(fixture("claim")["cases"][0]["response"]["payload"].clone());
        service.execute(claim(REQUEST_ID, "https://example.slack.com/archives/C08COORD01/p1760616000123456")).unwrap();
        assert_eq!(journal.current_reply_post(me, root, before_expiry()).unwrap_err(), JournalError::ReplyAuthorityRequired);
        *cloud.post_result.lock().unwrap() = Ok(case["response"]["payload"].clone());
        let request_id = case["request"]["payload"]["request_id"].as_str().unwrap();
        service.execute(reply_intent(request_id, Some("00000cc2-1111-4111-8111-000000000cc2"))).unwrap();

        let post = journal.current_reply_post(me, root, before_expiry()).unwrap().expect("the live intent");
        assert_eq!(post["coordination_operation_id"], "00007e91-1111-4111-8111-000000007e91");
        assert_eq!(post["expected_generation"], 1);
        assert_eq!(journal.current_reply_post("reg-supervisor-beta-07", root, before_expiry()).unwrap_err(), JournalError::ReplyAuthorityRequired);
        assert!(journal.current_reply_post(me, "1760616999.000001", before_expiry()).unwrap().is_none());
        assert_eq!(journal.current_reply_post(me, root, at_expiry()).unwrap_err(), JournalError::ReplyAuthorityRequired);

        let mut newer_claim = fixture("claim")["cases"][0]["response"]["payload"].clone();
        newer_claim["receipt"]["generation"] = json!(2);
        newer_claim["receipt"]["request_id"] = json!("00000002-2222-4222-8222-000000000002");
        *cloud.post_result.lock().unwrap() = Ok(newer_claim);
        service.execute(claim("00000002-2222-4222-8222-000000000002", "https://example.slack.com/archives/C08COORD01/p1760616000123456")).unwrap();
        assert_eq!(journal.current_reply_post(me, root, before_expiry()).unwrap_err(), JournalError::ReplyAuthorityRequired);

        let release = &fixture("release")["cases"][0];
        let mut released = release["response"]["payload"].clone();
        released["receipt"]["generation"] = json!(2);
        *cloud.post_result.lock().unwrap() = Ok(released);
        service
            .execute(ExternalWorkCommand::ReleaseClaim {
                resource_id: RESOURCE_ID.into(),
                expected_generation: 2,
                request_id: Some(release["request"]["payload"]["request_id"].as_str().unwrap().into()),
            })
            .unwrap();
        assert_eq!(journal.current_reply_post(me, root, before_expiry()).unwrap_err(), JournalError::ReplyAuthorityRequired);
    }

    /// cas-f869: Cloud's refusals pass through with their next step and no
    /// post fields; renew follows the fixture wire, and a lapsed claim is a
    /// named refusal with no automatic reclaim.
    #[test]
    fn reply_intent_refusals_and_renew_never_reclaim_cas_f869() {
        for (n, case) in ["stale pre-claim wake cannot reply", "unresolved reply pins the thread"].into_iter().enumerate() {
            let journal = journal();
            let cloud = Arc::new(FakeCloud::new(journal.clone()));
            let refusal = case_response("reply_intents", case);
            *cloud.post_result.lock().unwrap() = Ok(refusal.clone());
            let result = service(cloud, Arc::new(FakeViolet::ok()), journal)
                .execute(reply_intent(&format!("00000073-2222-4222-8222-00000000007{n}"), None))
                .unwrap();
            assert_eq!((&result["code"], &result["reason"]), (&refusal["code"], &refusal["reason"]));
            assert_eq!(result["receipt"]["next_action"], refusal["receipt"]["next_action"]);
            assert!(result.get("violet_post").is_none());
        }

        let expected = fixture("renew");
        let case = &expected["cases"][0];
        let journal = journal();
        let cloud = Arc::new(FakeCloud::new(journal.clone()));
        *cloud.post_result.lock().unwrap() = Ok(case_response("renew", "expired idle claim"));
        let violet = Arc::new(FakeViolet::ok());
        let result = service(cloud.clone(), violet.clone(), journal)
            .execute(ExternalWorkCommand::RenewClaim {
                resource_id: RESOURCE_ID.into(),
                expected_generation: 1,
                request_id: Some(case["request"]["payload"]["request_id"].as_str().unwrap().into()),
            })
            .unwrap();
        let posted = cloud.posted.lock().unwrap();
        assert_eq!(posted.len(), 1, "renew only; never an automatic claim");
        assert_eq!(posted[0].0, expected["path"].as_str().unwrap().replace("{id}", RESOURCE_ID));
        assert_eq!(posted[0].1, case["request"]["payload"]);
        assert_eq!((result["code"].as_str(), result["reason"].as_str()), (Some("rejected"), Some("claim_not_active")));
        assert_eq!(violet.calls.load(Ordering::SeqCst), 0);
    }
}
