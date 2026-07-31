use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::str::FromStr;

use cas_types::{
    VerificationDispatch, VerificationProofBoundary, WorkerCompletionReceipt,
    WorkerCompletionReceiptInput, WorkerDeliveryEvent, WorkerDeliveryState,
    WorkerDeliveryTransaction,
};

use crate::error::StoreError;
use crate::{Result, SQLITE_BUSY_TIMEOUT};

pub const DELIVERY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS worker_completion_receipts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    worker_agent_id TEXT NOT NULL,
    worker_name TEXT NOT NULL,
    repo_selector TEXT NOT NULL,
    source_branch TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    merge_base_sha TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    target_sha TEXT NOT NULL,
    proof_reference TEXT NOT NULL,
    scope_summary TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_worker_completion_receipts_task
    ON worker_completion_receipts(task_id, created_at DESC);
CREATE TRIGGER IF NOT EXISTS worker_completion_receipts_immutable_update
BEFORE UPDATE ON worker_completion_receipts
BEGIN SELECT RAISE(ABORT, 'worker completion receipts are immutable'); END;
CREATE TRIGGER IF NOT EXISTS worker_completion_receipts_immutable_delete
BEFORE DELETE ON worker_completion_receipts
BEGIN SELECT RAISE(ABORT, 'worker completion receipts are immutable'); END;

CREATE TABLE IF NOT EXISTS worker_delivery_transactions (
    id TEXT PRIMARY KEY,
    receipt_id TEXT NOT NULL UNIQUE,
    task_id TEXT NOT NULL,
    state TEXT NOT NULL,
    supervisor_agent_id TEXT,
    verification_id TEXT,
    merge_commit_sha TEXT,
    last_error_code TEXT,
    last_error_detail TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(receipt_id) REFERENCES worker_completion_receipts(id)
);
CREATE INDEX IF NOT EXISTS idx_worker_delivery_transactions_task
    ON worker_delivery_transactions(task_id, created_at DESC);

CREATE TABLE IF NOT EXISTS worker_delivery_events (
    id TEXT PRIMARY KEY,
    transaction_id TEXT NOT NULL,
    state TEXT NOT NULL,
    actor_agent_id TEXT NOT NULL,
    detail TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(transaction_id, state),
    FOREIGN KEY(transaction_id) REFERENCES worker_delivery_transactions(id)
);
CREATE TRIGGER IF NOT EXISTS worker_delivery_events_append_only_update
BEFORE UPDATE ON worker_delivery_events
BEGIN SELECT RAISE(ABORT, 'worker delivery events are append-only'); END;
CREATE TRIGGER IF NOT EXISTS worker_delivery_events_append_only_delete
BEFORE DELETE ON worker_delivery_events
BEGIN SELECT RAISE(ABORT, 'worker delivery events are append-only'); END;
"#;

fn open(root: &Path) -> Result<Connection> {
    let conn = Connection::open(root.join("cas.db"))?;
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    conn.execute_batch(DELIVERY_SCHEMA)?;
    Ok(conn)
}

fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn build_worker_completion_receipt(
    input: &WorkerCompletionReceiptInput,
    worker_name: &str,
    created_at: DateTime<Utc>,
) -> WorkerCompletionReceipt {
    let hash = digest(&[
        &input.task_id,
        &input.worker_agent_id,
        worker_name,
        &input.repo_selector,
        &input.source_branch,
        &input.commit_sha,
        &input.merge_base_sha,
        &input.target_branch,
        &input.target_sha,
        &input.proof_reference,
        &input.scope_summary,
    ]);
    WorkerCompletionReceipt {
        id: format!("wcr-{}", &hash[..24]),
        task_id: input.task_id.clone(),
        worker_agent_id: input.worker_agent_id.clone(),
        worker_name: worker_name.to_string(),
        repo_selector: input.repo_selector.clone(),
        source_branch: input.source_branch.clone(),
        commit_sha: input.commit_sha.clone(),
        merge_base_sha: input.merge_base_sha.clone(),
        target_branch: input.target_branch.clone(),
        target_sha: input.target_sha.clone(),
        proof_reference: input.proof_reference.clone(),
        scope_summary: input.scope_summary.clone(),
        created_at,
    }
}

fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn parse_state(value: String) -> rusqlite::Result<WorkerDeliveryState> {
    WorkerDeliveryState::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn receipt_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerCompletionReceipt> {
    Ok(WorkerCompletionReceipt {
        id: row.get(0)?,
        task_id: row.get(1)?,
        worker_agent_id: row.get(2)?,
        worker_name: row.get(3)?,
        repo_selector: row.get(4)?,
        source_branch: row.get(5)?,
        commit_sha: row.get(6)?,
        merge_base_sha: row.get(7)?,
        target_branch: row.get(8)?,
        target_sha: row.get(9)?,
        proof_reference: row.get(10)?,
        scope_summary: row.get(11)?,
        created_at: parse_time(row.get(12)?)?,
    })
}

fn transaction_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerDeliveryTransaction> {
    Ok(WorkerDeliveryTransaction {
        id: row.get(0)?,
        receipt_id: row.get(1)?,
        task_id: row.get(2)?,
        state: parse_state(row.get(3)?)?,
        supervisor_agent_id: row.get(4)?,
        verification_id: row.get(5)?,
        merge_commit_sha: row.get(6)?,
        last_error_code: row.get(7)?,
        last_error_detail: row.get(8)?,
        created_at: parse_time(row.get(9)?)?,
        updated_at: parse_time(row.get(10)?)?,
    })
}

fn insert_event(
    conn: &Connection,
    transaction_id: &str,
    state: WorkerDeliveryState,
    actor_agent_id: &str,
    detail: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    let hash = digest(&[transaction_id, &state.to_string()]);
    conn.execute(
        "INSERT OR IGNORE INTO worker_delivery_events
         (id, transaction_id, state, actor_agent_id, detail, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            format!("wde-{}", &hash[..24]),
            transaction_id,
            state.to_string(),
            actor_agent_id,
            detail,
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn create_worker_delivery(
    root: &Path,
    receipt: &WorkerCompletionReceipt,
    state: WorkerDeliveryState,
    actor_agent_id: &str,
) -> Result<WorkerDeliveryTransaction> {
    let mut conn = open(root)?;
    let tx = conn.transaction()?;
    let transaction = create_worker_delivery_with_conn(&tx, receipt, state, actor_agent_id)?;
    tx.commit()?;
    Ok(transaction)
}

fn create_worker_delivery_with_conn(
    conn: &Connection,
    receipt: &WorkerCompletionReceipt,
    state: WorkerDeliveryState,
    actor_agent_id: &str,
) -> Result<WorkerDeliveryTransaction> {
    conn.execute(
        "INSERT OR IGNORE INTO worker_completion_receipts
         (id, task_id, worker_agent_id, worker_name, repo_selector, source_branch,
          commit_sha, merge_base_sha, target_branch, target_sha, proof_reference,
          scope_summary, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            receipt.id,
            receipt.task_id,
            receipt.worker_agent_id,
            receipt.worker_name,
            receipt.repo_selector,
            receipt.source_branch,
            receipt.commit_sha,
            receipt.merge_base_sha,
            receipt.target_branch,
            receipt.target_sha,
            receipt.proof_reference,
            receipt.scope_summary,
            receipt.created_at.to_rfc3339(),
        ],
    )?;
    let persisted = conn.query_row(
        "SELECT id, task_id, worker_agent_id, worker_name, repo_selector, source_branch,
                commit_sha, merge_base_sha, target_branch, target_sha, proof_reference,
                scope_summary, created_at
         FROM worker_completion_receipts WHERE id = ?1",
        params![receipt.id],
        receipt_from_row,
    )?;
    let same_immutable_payload = persisted.id == receipt.id
        && persisted.task_id == receipt.task_id
        && persisted.worker_agent_id == receipt.worker_agent_id
        && persisted.worker_name == receipt.worker_name
        && persisted.repo_selector == receipt.repo_selector
        && persisted.source_branch == receipt.source_branch
        && persisted.commit_sha == receipt.commit_sha
        && persisted.merge_base_sha == receipt.merge_base_sha
        && persisted.target_branch == receipt.target_branch
        && persisted.target_sha == receipt.target_sha
        && persisted.proof_reference == receipt.proof_reference
        && persisted.scope_summary == receipt.scope_summary;
    if !same_immutable_payload {
        return Err(StoreError::Parse(
            "immutable worker completion receipt mismatch".to_string(),
        ));
    }
    let now = Utc::now();
    let transaction_id = worker_delivery_transaction_id(&receipt.id);
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO worker_delivery_transactions
         (id, receipt_id, task_id, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            transaction_id,
            receipt.id,
            receipt.task_id,
            state.to_string(),
            now.to_rfc3339(),
        ],
    )?;
    let transaction = conn.query_row(
        "SELECT id, receipt_id, task_id, state, supervisor_agent_id, verification_id,
                merge_commit_sha, last_error_code, last_error_detail, created_at, updated_at
         FROM worker_delivery_transactions WHERE receipt_id = ?1",
        params![receipt.id],
        transaction_from_row,
    )?;
    if inserted == 1 {
        insert_event(conn, &transaction_id, state, actor_agent_id, None, now)?;
    } else if transaction.state != state {
        return Err(StoreError::Parse(format!(
            "worker delivery initial state mismatch: persisted {}, requested {}",
            transaction.state, state
        )));
    }
    Ok(transaction)
}

/// Atomically persist a new immutable delivery and its exact verification
/// dispatch. Git remains outside SQLite; this transaction only establishes
/// the monotonic intent that later Git/reconciliation steps consume.
pub fn create_worker_delivery_with_dispatch(
    root: &Path,
    receipt: &WorkerCompletionReceipt,
    state: WorkerDeliveryState,
    actor_agent_id: &str,
    owner_agent_id: &str,
    deadline_at: DateTime<Utc>,
) -> Result<(WorkerDeliveryTransaction, VerificationDispatch)> {
    let mut conn = open(root)?;
    conn.execute_batch(crate::verification_store::VERIFICATION_SCHEMA)?;
    let tx = conn.transaction()?;
    let boundary = VerificationProofBoundary::delivery(
        receipt.id.clone(),
        worker_delivery_transaction_id(&receipt.id),
    );
    let dispatch = crate::verification_store::create_verification_dispatch_bound_with_conn(
        &tx,
        &receipt.task_id,
        actor_agent_id,
        owner_agent_id,
        &boundary,
        deadline_at,
        false,
    )?;
    let transaction = create_worker_delivery_with_conn(&tx, receipt, state, actor_agent_id)?;
    tx.commit()?;
    Ok((transaction, dispatch))
}

/// Atomically revalidate the exact active task-lease generation before
/// persisting a new receipt, delivery transaction, or verification dispatch.
pub fn create_worker_delivery_with_dispatch_for_lease(
    root: &Path,
    receipt: &WorkerCompletionReceipt,
    state: WorkerDeliveryState,
    actor_agent_id: &str,
    expected_lease_epoch: u64,
    owner_agent_id: &str,
    deadline_at: DateTime<Utc>,
) -> Result<(WorkerDeliveryTransaction, VerificationDispatch)> {
    let mut conn = open(root)?;
    conn.execute_batch(crate::agent_store::AGENT_SCHEMA)?;
    conn.execute_batch(crate::verification_store::VERIFICATION_SCHEMA)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let authority = tx
        .query_row(
            "SELECT l.agent_id, l.epoch, l.expires_at, a.role, a.status
             FROM task_leases l
             JOIN agents a ON a.id = l.agent_id
             WHERE l.task_id = ?1 AND l.status = 'active'",
            params![receipt.task_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let valid = authority.is_some_and(|(agent_id, epoch, expires_at, role, status)| {
        receipt.worker_agent_id == actor_agent_id
            && agent_id == actor_agent_id
            && epoch == expected_lease_epoch as i64
            && role == "worker"
            && matches!(status.as_str(), "active" | "idle")
            && DateTime::parse_from_rfc3339(&expires_at)
                .map(|expires| expires.with_timezone(&Utc) > Utc::now())
                .unwrap_or(false)
    });
    if !valid {
        return Err(StoreError::Parse(
            "exact active task lease changed, expired, or no longer belongs to the authenticated worker session"
                .to_string(),
        ));
    }
    let boundary = VerificationProofBoundary::delivery(
        receipt.id.clone(),
        worker_delivery_transaction_id(&receipt.id),
    );
    let dispatch = crate::verification_store::create_verification_dispatch_bound_with_conn(
        &tx,
        &receipt.task_id,
        actor_agent_id,
        owner_agent_id,
        &boundary,
        deadline_at,
        false,
    )?;
    let transaction = create_worker_delivery_with_conn(&tx, receipt, state, actor_agent_id)?;
    tx.commit()?;
    Ok((transaction, dispatch))
}

pub fn worker_delivery_transaction_id(receipt_id: &str) -> String {
    receipt_id.replacen("wcr-", "wdt-", 1)
}

pub fn get_worker_delivery_by_receipt(
    root: &Path,
    receipt_id: &str,
) -> Result<Option<(WorkerCompletionReceipt, WorkerDeliveryTransaction)>> {
    let conn = open(root)?;
    conn.query_row(
        "SELECT r.id, r.task_id, r.worker_agent_id, r.worker_name, r.repo_selector,
                r.source_branch, r.commit_sha, r.merge_base_sha, r.target_branch,
                r.target_sha, r.proof_reference, r.scope_summary, r.created_at,
                t.id, t.receipt_id, t.task_id, t.state, t.supervisor_agent_id,
                t.verification_id, t.merge_commit_sha, t.last_error_code,
                t.last_error_detail, t.created_at, t.updated_at
         FROM worker_completion_receipts r
         JOIN worker_delivery_transactions t ON t.receipt_id = r.id
         WHERE r.id = ?1",
        params![receipt_id],
        |row| {
            let receipt = receipt_from_row(row)?;
            let transaction = WorkerDeliveryTransaction {
                id: row.get(13)?,
                receipt_id: row.get(14)?,
                task_id: row.get(15)?,
                state: parse_state(row.get(16)?)?,
                supervisor_agent_id: row.get(17)?,
                verification_id: row.get(18)?,
                merge_commit_sha: row.get(19)?,
                last_error_code: row.get(20)?,
                last_error_detail: row.get(21)?,
                created_at: parse_time(row.get(22)?)?,
                updated_at: parse_time(row.get(23)?)?,
            };
            Ok((receipt, transaction))
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_latest_worker_delivery(
    root: &Path,
    task_id: &str,
) -> Result<Option<(WorkerCompletionReceipt, WorkerDeliveryTransaction)>> {
    let conn = open(root)?;
    conn.query_row(
        "SELECT r.id, r.task_id, r.worker_agent_id, r.worker_name, r.repo_selector,
                r.source_branch, r.commit_sha, r.merge_base_sha, r.target_branch,
                r.target_sha, r.proof_reference, r.scope_summary, r.created_at,
                t.id, t.receipt_id, t.task_id, t.state, t.supervisor_agent_id,
                t.verification_id, t.merge_commit_sha, t.last_error_code,
                t.last_error_detail, t.created_at, t.updated_at
         FROM worker_completion_receipts r
         JOIN worker_delivery_transactions t ON t.receipt_id = r.id
         WHERE r.task_id = ?1 ORDER BY r.created_at DESC LIMIT 1",
        params![task_id],
        |row| {
            let receipt = receipt_from_row(row)?;
            let transaction = WorkerDeliveryTransaction {
                id: row.get(13)?,
                receipt_id: row.get(14)?,
                task_id: row.get(15)?,
                state: parse_state(row.get(16)?)?,
                supervisor_agent_id: row.get(17)?,
                verification_id: row.get(18)?,
                merge_commit_sha: row.get(19)?,
                last_error_code: row.get(20)?,
                last_error_detail: row.get(21)?,
                created_at: parse_time(row.get(22)?)?,
                updated_at: parse_time(row.get(23)?)?,
            };
            Ok((receipt, transaction))
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn transition_worker_delivery(
    root: &Path,
    transaction_id: &str,
    expected: &[WorkerDeliveryState],
    state: WorkerDeliveryState,
    actor_agent_id: &str,
    supervisor_agent_id: Option<&str>,
    verification_id: Option<&str>,
    merge_commit_sha: Option<&str>,
    error: Option<(&str, &str)>,
) -> Result<WorkerDeliveryTransaction> {
    let mut conn = open(root)?;
    let tx = conn.transaction()?;
    let current = tx.query_row(
        "SELECT id, receipt_id, task_id, state, supervisor_agent_id, verification_id,
                merge_commit_sha, last_error_code, last_error_detail, created_at, updated_at
         FROM worker_delivery_transactions WHERE id = ?1",
        params![transaction_id],
        transaction_from_row,
    )?;
    if current.state == state {
        tx.commit()?;
        return Ok(current);
    }
    if !expected.contains(&current.state) {
        return Err(StoreError::Parse(format!(
            "delivery transition rejected: {} -> {}",
            current.state, state
        )));
    }
    let now = Utc::now();
    let (error_code, error_detail) = error
        .map(|(code, detail)| (Some(code), Some(detail)))
        .unwrap_or((None, None));
    tx.execute(
        "UPDATE worker_delivery_transactions
         SET state = ?2,
             supervisor_agent_id = COALESCE(?3, supervisor_agent_id),
             verification_id = COALESCE(?4, verification_id),
             merge_commit_sha = COALESCE(?5, merge_commit_sha),
             last_error_code = ?6,
             last_error_detail = ?7,
             updated_at = ?8
         WHERE id = ?1",
        params![
            transaction_id,
            state.to_string(),
            supervisor_agent_id,
            verification_id,
            merge_commit_sha,
            error_code,
            error_detail,
            now.to_rfc3339(),
        ],
    )?;
    insert_event(
        &tx,
        transaction_id,
        state,
        actor_agent_id,
        error_detail,
        now,
    )?;
    let updated = tx.query_row(
        "SELECT id, receipt_id, task_id, state, supervisor_agent_id, verification_id,
                merge_commit_sha, last_error_code, last_error_detail, created_at, updated_at
         FROM worker_delivery_transactions WHERE id = ?1",
        params![transaction_id],
        transaction_from_row,
    )?;
    tx.commit()?;
    Ok(updated)
}

pub fn transition_worker_delivery_verification_with_conn(
    conn: &Connection,
    transaction_id: &str,
    verification_id: &str,
    approved: bool,
    actor_agent_id: &str,
) -> Result<Option<WorkerDeliveryTransaction>> {
    conn.execute_batch(DELIVERY_SCHEMA)?;
    let current = conn
        .query_row(
            "SELECT id, receipt_id, task_id, state, supervisor_agent_id, verification_id,
                    merge_commit_sha, last_error_code, last_error_detail, created_at, updated_at
             FROM worker_delivery_transactions
             WHERE id = ?1",
            params![transaction_id],
            transaction_from_row,
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(None);
    };
    if current.state != WorkerDeliveryState::AwaitingVerification {
        return Ok(None);
    }
    let exact_binding: i64 = conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM verifications v
            JOIN verification_dispatches d ON d.id = v.dispatch_id
            JOIN worker_completion_receipts r ON r.id = d.receipt_id
            WHERE v.id = ?1
              AND v.task_id = ?2
              AND v.agent_id = ?3
              AND d.task_id = ?2
              AND d.receipt_id = ?4
              AND d.delivery_transaction_id = ?5
              AND d.state = 'resolved'
              AND r.id = ?4
              AND r.task_id = ?2
              AND ((?6 = 1 AND v.status = 'approved')
                   OR (?6 = 0 AND v.status IN ('rejected', 'error')))
        )",
        params![
            verification_id,
            current.task_id,
            actor_agent_id,
            current.receipt_id,
            current.id,
            approved as i64,
        ],
        |row| row.get(0),
    )?;
    if exact_binding != 1 {
        return Err(StoreError::Parse(
            "delivery verification is not bound to the exact receipt, transaction, task, and verifier"
                .to_string(),
        ));
    }
    let target = if approved {
        WorkerDeliveryState::AwaitingMerge
    } else {
        WorkerDeliveryState::VerificationFailed
    };
    let now = Utc::now();
    let changed = conn.execute(
        "UPDATE worker_delivery_transactions
         SET state = ?2, verification_id = ?3,
             last_error_code = ?4, last_error_detail = ?5, updated_at = ?6
         WHERE id = ?1 AND state = 'awaiting_verification'",
        params![
            current.id,
            target.to_string(),
            verification_id,
            if approved {
                None::<&str>
            } else {
                Some("verification_failed")
            },
            if approved {
                None::<&str>
            } else {
                Some("durable verification rejected")
            },
            now.to_rfc3339(),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::Parse(
            "exact delivery verification transition raced".to_string(),
        ));
    }
    insert_event(
        conn,
        &current.id,
        target,
        actor_agent_id,
        if approved {
            None
        } else {
            Some("durable verification rejected")
        },
        now,
    )?;
    conn.query_row(
        "SELECT id, receipt_id, task_id, state, supervisor_agent_id, verification_id,
                merge_commit_sha, last_error_code, last_error_detail, created_at, updated_at
         FROM worker_delivery_transactions WHERE id = ?1",
        params![current.id],
        transaction_from_row,
    )
    .map(Some)
    .map_err(Into::into)
}

pub fn list_worker_delivery_events(
    root: &Path,
    transaction_id: &str,
) -> Result<Vec<WorkerDeliveryEvent>> {
    let conn = open(root)?;
    let mut stmt = conn.prepare(
        "SELECT id, transaction_id, state, actor_agent_id, detail, created_at
         FROM worker_delivery_events WHERE transaction_id = ?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![transaction_id], |row| {
        Ok(WorkerDeliveryEvent {
            id: row.get(0)?,
            transaction_id: row.get(1)?,
            state: parse_state(row.get(2)?)?,
            actor_agent_id: row.get(3)?,
            detail: row.get(4)?,
            created_at: parse_time(row.get(5)?)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentStore, SqliteAgentStore};
    use cas_types::{Agent, AgentRole, ClaimResult};
    use tempfile::TempDir;

    fn input() -> WorkerCompletionReceiptInput {
        WorkerCompletionReceiptInput {
            task_id: "cas-delivery".into(),
            worker_agent_id: "worker-session".into(),
            repo_selector: "remote:github.com/org/repo".into(),
            source_branch: "factory/worker".into(),
            commit_sha: "a".repeat(40),
            merge_base_sha: "b".repeat(40),
            target_branch: "epic/cas-delivery".into(),
            target_sha: "c".repeat(40),
            proof_reference: "proof:workspace-1".into(),
            scope_summary: "bounded delivery change".into(),
        }
    }

    fn register_worker(store: &SqliteAgentStore, id: &str, name: &str) {
        let mut agent = Agent::new(id.to_string(), name.to_string());
        agent.role = AgentRole::Worker;
        store.register(&agent).unwrap();
    }

    fn delivery_boundary_counts(root: &Path, receipt_id: &str) -> (i64, i64, i64) {
        let conn = open(root).unwrap();
        conn.execute_batch(crate::verification_store::VERIFICATION_SCHEMA)
            .unwrap();
        let receipts = conn
            .query_row(
                "SELECT COUNT(*) FROM worker_completion_receipts WHERE id = ?1",
                params![receipt_id],
                |row| row.get(0),
            )
            .unwrap();
        let deliveries = conn
            .query_row(
                "SELECT COUNT(*) FROM worker_delivery_transactions WHERE receipt_id = ?1",
                params![receipt_id],
                |row| row.get(0),
            )
            .unwrap();
        let dispatches = conn
            .query_row(
                "SELECT COUNT(*) FROM verification_dispatches WHERE task_id = 'cas-delivery'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        (receipts, deliveries, dispatches)
    }

    #[test]
    fn receipt_write_revalidates_expired_and_replaced_lease_generation_atomically() {
        for scenario in ["expired", "replaced", "dead"] {
            let root = TempDir::new().unwrap();
            let store = SqliteAgentStore::open(root.path()).unwrap();
            store.init().unwrap();
            register_worker(&store, "worker-session", "worker");
            register_worker(&store, "replacement-session", "worker");

            let duration = if scenario == "expired" { -1 } else { 600 };
            let ClaimResult::Success(original) = store
                .try_claim("cas-delivery", "worker-session", duration, Some(scenario))
                .unwrap()
            else {
                panic!("fixture lease must be claimed")
            };
            if scenario == "replaced" {
                store
                    .release_lease("cas-delivery", "worker-session")
                    .unwrap();
                assert!(matches!(
                    store
                        .try_claim(
                            "cas-delivery",
                            "replacement-session",
                            600,
                            Some("replacement race"),
                        )
                        .unwrap(),
                    ClaimResult::Success(_)
                ));
            } else if scenario == "dead" {
                store.mark_stale("worker-session").unwrap();
            }

            let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
            let before = delivery_boundary_counts(root.path(), &receipt.id);
            let error = create_worker_delivery_with_dispatch_for_lease(
                root.path(),
                &receipt,
                WorkerDeliveryState::AwaitingVerification,
                "worker-session",
                original.epoch,
                "verifier-owner",
                Utc::now() + chrono::Duration::minutes(10),
            )
            .expect_err("stale lease authority must fail closed");
            assert!(error.to_string().contains("exact active task lease"));
            assert_eq!(
                delivery_boundary_counts(root.path(), &receipt.id),
                before,
                "{scenario} authority persisted receipt, transaction, or dispatch"
            );
        }
    }

    #[test]
    fn receipt_is_immutable_and_retries_are_idempotent() {
        let root = TempDir::new().unwrap();
        let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let first = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingVerification,
            "worker-session",
        )
        .unwrap();
        let retry = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingVerification,
            "worker-session",
        )
        .unwrap();
        assert_eq!(first.id, retry.id);
        let rebuilt = build_worker_completion_receipt(
            &input(),
            "worker",
            Utc::now() + chrono::Duration::seconds(1),
        );
        assert_eq!(rebuilt.id, receipt.id);
        assert_eq!(
            create_worker_delivery(
                root.path(),
                &rebuilt,
                WorkerDeliveryState::AwaitingVerification,
                "worker-session",
            )
            .unwrap()
            .id,
            first.id
        );
        assert_eq!(
            list_worker_delivery_events(root.path(), &first.id)
                .unwrap()
                .len(),
            1
        );
        let before_mismatch = get_worker_delivery_by_receipt(root.path(), &receipt.id)
            .unwrap()
            .expect("persisted delivery boundary");
        let events_before_mismatch = list_worker_delivery_events(root.path(), &first.id).unwrap();
        let mismatch = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::Delivered,
            "worker-session",
        )
        .expect_err("a retry cannot rename the immutable initial state");
        assert!(mismatch.to_string().contains("initial state mismatch"));
        assert_eq!(
            get_worker_delivery_by_receipt(root.path(), &receipt.id)
                .unwrap()
                .expect("delivery survives rejected retry"),
            before_mismatch,
            "different-state retry mutated the receipt or transaction"
        );
        assert_eq!(
            list_worker_delivery_events(root.path(), &first.id).unwrap(),
            events_before_mismatch,
            "different-state retry appended a false event"
        );
        let conn = open(root.path()).unwrap();
        assert!(
            conn.execute(
                "UPDATE worker_completion_receipts SET scope_summary = 'changed' WHERE id = ?1",
                params![receipt.id],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "DELETE FROM worker_completion_receipts WHERE id = ?1",
                params![receipt.id],
            )
            .is_err()
        );
    }

    #[test]
    fn concurrent_retries_are_state_and_event_idempotent() {
        let root = TempDir::new().unwrap();
        let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let transaction = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingVerification,
            "worker-session",
        )
        .unwrap();
        let before = get_worker_delivery_by_receipt(root.path(), &receipt.id)
            .unwrap()
            .expect("persisted delivery boundary");
        let events_before = list_worker_delivery_events(root.path(), &transaction.id).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let outcomes = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for index in 0..8 {
                let root = root.path().to_path_buf();
                let receipt = receipt.clone();
                let barrier = barrier.clone();
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    let requested_state = if index == 7 {
                        WorkerDeliveryState::Delivered
                    } else {
                        WorkerDeliveryState::AwaitingVerification
                    };
                    create_worker_delivery(&root, &receipt, requested_state, "worker-session")
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("retry thread must not panic"))
                .collect::<Vec<_>>()
        });

        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 7);
        let errors = outcomes
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("initial state mismatch"));
        assert_eq!(
            get_worker_delivery_by_receipt(root.path(), &receipt.id)
                .unwrap()
                .expect("delivery survives concurrent retries"),
            before,
            "concurrent retries mutated the receipt or transaction"
        );
        assert_eq!(
            list_worker_delivery_events(root.path(), &transaction.id).unwrap(),
            events_before,
            "concurrent retries appended duplicate or false events"
        );
    }

    #[test]
    fn different_receipt_cannot_persist_under_an_active_proof_cycle() {
        let root = TempDir::new().unwrap();
        let receipt_a = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let (transaction_a, dispatch_a) = create_worker_delivery_with_dispatch(
            root.path(),
            &receipt_a,
            WorkerDeliveryState::AwaitingVerification,
            "worker-session",
            "verifier-owner",
            Utc::now() + chrono::Duration::minutes(10),
        )
        .unwrap();

        let mut input_b = input();
        input_b.commit_sha = "d".repeat(40);
        let receipt_b = build_worker_completion_receipt(&input_b, "worker", Utc::now());
        assert_ne!(receipt_a.id, receipt_b.id);
        assert!(
            create_worker_delivery_with_dispatch(
                root.path(),
                &receipt_b,
                WorkerDeliveryState::AwaitingVerification,
                "worker-session",
                "verifier-owner",
                Utc::now() + chrono::Duration::minutes(10),
            )
            .is_err(),
            "receipt B must not replace active dispatch A"
        );
        assert!(
            get_worker_delivery_by_receipt(root.path(), &receipt_b.id)
                .unwrap()
                .is_none(),
            "receipt B and its transaction must roll back with the rejected dispatch"
        );
        assert_eq!(
            crate::verification_store::get_latest_verification_dispatch(
                root.path(),
                "cas-delivery",
            )
            .unwrap()
            .unwrap()
            .id,
            dispatch_a.id
        );
        assert_eq!(
            get_latest_worker_delivery(root.path(), "cas-delivery")
                .unwrap()
                .unwrap()
                .1
                .id,
            transaction_a.id
        );
    }

    #[test]
    fn transitions_are_strict_and_corrupt_state_fails_loudly() {
        let root = TempDir::new().unwrap();
        let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let transaction = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingVerification,
            "worker-session",
        )
        .unwrap();
        transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::AwaitingVerification],
            WorkerDeliveryState::AwaitingMerge,
            "supervisor",
            Some("supervisor"),
            Some("ver-1"),
            None,
            None,
        )
        .unwrap();
        let merge_sha = "d".repeat(40);
        assert!(
            transition_worker_delivery(
                root.path(),
                &transaction.id,
                &[WorkerDeliveryState::AwaitingVerification],
                WorkerDeliveryState::Merged,
                "supervisor",
                Some("supervisor"),
                None,
                Some(&merge_sha),
                None,
            )
            .is_err()
        );
        let conn = open(root.path()).unwrap();
        conn.execute(
            "UPDATE worker_delivery_transactions SET state = 'mystery' WHERE id = ?1",
            params![transaction.id],
        )
        .unwrap();
        assert!(get_latest_worker_delivery(root.path(), "cas-delivery").is_err());
    }

    #[test]
    fn verification_projection_returns_none_without_an_exact_state_mutation() {
        let root = TempDir::new().unwrap();
        let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let transaction = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingMerge,
            "worker-session",
        )
        .unwrap();
        let conn = open(root.path()).unwrap();

        let projected = transition_worker_delivery_verification_with_conn(
            &conn,
            &transaction.id,
            "ver-noop",
            true,
            "verifier-session",
        )
        .unwrap();

        assert!(
            projected.is_none(),
            "a non-awaiting delivery must not report a verification transition"
        );
        let (_, unchanged) = get_latest_worker_delivery(root.path(), "cas-delivery")
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.state, WorkerDeliveryState::AwaitingMerge);
        assert!(unchanged.verification_id.is_none());
        assert_eq!(
            list_worker_delivery_events(root.path(), &transaction.id)
                .unwrap()
                .len(),
            1,
            "a no-op projection must not append an event"
        );
    }

    #[test]
    fn merge_intent_retry_resume_and_events_are_idempotent_and_portable() {
        let root = TempDir::new().unwrap();
        let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let transaction = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingMerge,
            "worker-session",
        )
        .unwrap();
        let authorized = transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::AwaitingMerge],
            WorkerDeliveryState::MergeAuthorized,
            "supervisor-session",
            Some("supervisor-session"),
            Some("ver-approved"),
            None,
            None,
        )
        .unwrap();
        let retry = transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::AwaitingMerge],
            WorkerDeliveryState::MergeAuthorized,
            "supervisor-session",
            Some("supervisor-session"),
            Some("ver-approved"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(authorized, retry);
        let merge_sha = "d".repeat(40);
        transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::MergeAuthorized],
            WorkerDeliveryState::Merged,
            "supervisor-session",
            Some("supervisor-session"),
            None,
            Some(&merge_sha),
            None,
        )
        .unwrap();
        transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::Merged],
            WorkerDeliveryState::CloseReady,
            "supervisor-session",
            Some("supervisor-session"),
            None,
            Some(&merge_sha),
            None,
        )
        .unwrap();
        transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::CloseReady],
            WorkerDeliveryState::Delivered,
            "supervisor-session",
            Some("supervisor-session"),
            None,
            Some(&merge_sha),
            None,
        )
        .unwrap();
        let events = list_worker_delivery_events(root.path(), &transaction.id).unwrap();
        assert_eq!(events.len(), 5);
        let persisted = serde_json::to_string(&(receipt, events)).unwrap();
        assert!(!persisted.contains("/home/"));
        assert!(!persisted.contains("proof payload"));

        let conn = open(root.path()).unwrap();
        assert!(
            conn.execute(
                "UPDATE worker_delivery_events SET detail = 'changed' WHERE transaction_id = ?1",
                params![transaction.id],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "DELETE FROM worker_delivery_events WHERE transaction_id = ?1",
                params![transaction.id],
            )
            .is_err()
        );
    }

    #[test]
    fn merge_conflict_is_durable_and_requires_a_new_receipt() {
        let root = TempDir::new().unwrap();
        let receipt = build_worker_completion_receipt(&input(), "worker", Utc::now());
        let transaction = create_worker_delivery(
            root.path(),
            &receipt,
            WorkerDeliveryState::AwaitingMerge,
            "worker-session",
        )
        .unwrap();
        transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::AwaitingMerge],
            WorkerDeliveryState::MergeAuthorized,
            "supervisor-session",
            Some("supervisor-session"),
            None,
            None,
            None,
        )
        .unwrap();
        let conflict = transition_worker_delivery(
            root.path(),
            &transaction.id,
            &[WorkerDeliveryState::MergeAuthorized],
            WorkerDeliveryState::Conflict,
            "supervisor-session",
            Some("supervisor-session"),
            None,
            None,
            Some((
                "merge_conflict",
                "Git merge did not complete; explicit conflict/recovery is required.",
            )),
        )
        .unwrap();
        assert_eq!(conflict.state, WorkerDeliveryState::Conflict);
        assert!(
            transition_worker_delivery(
                root.path(),
                &transaction.id,
                &[WorkerDeliveryState::AwaitingMerge],
                WorkerDeliveryState::MergeAuthorized,
                "supervisor-session",
                Some("supervisor-session"),
                None,
                None,
                None,
            )
            .is_err()
        );
    }
}
