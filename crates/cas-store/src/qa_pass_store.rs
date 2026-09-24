//! Durable independent QA pass rounds (cas-619f).
//!
//! One row per QA round for a user-facing delivery. The row is both the
//! dispatch (who must review which exact branch tip, by when) and the typed
//! verdict. The no-self-review rule is enforced here, at the storage
//! boundary, so no caller can route around it: the implementer can neither
//! claim nor resolve a round for its own delivery.

use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;
use crate::error::StoreError;
use crate::shared_db::ImmediateTx;
use cas_types::{QA_PASS_WITHDRAWN_PREFIX, QaPass, QaPassState, QaVerdict};

/// DDL shared by store-open repair and the numbered migration.
pub const QA_PASS_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS qa_passes (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL,
        round INTEGER NOT NULL,
        implementer_agent_id TEXT NOT NULL,
        branch TEXT NOT NULL,
        bound_head TEXT NOT NULL,
        qa_task_id TEXT,
        reviewer_agent_id TEXT,
        state TEXT NOT NULL CHECK (state IN
            ('pending', 'claimed', 'passed', 'failed', 'timed_out', 'superseded', 'waived')),
        summary TEXT,
        issues_json TEXT,
        ledger_path TEXT,
        issuer_agent_id TEXT,
        requested_at TEXT NOT NULL,
        deadline_at TEXT NOT NULL,
        resolved_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_qa_passes_task ON qa_passes(task_id, requested_at DESC)",
    "CREATE INDEX IF NOT EXISTS idx_qa_passes_qa_task ON qa_passes(qa_task_id)",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_qa_passes_active_task
        ON qa_passes(task_id) WHERE state IN ('pending', 'claimed')",
];

const COLUMNS: &str = "id, task_id, round, implementer_agent_id, branch, bound_head, qa_task_id,
    reviewer_agent_id, state, summary, issues_json, ledger_path, issuer_agent_id,
    requested_at, deadline_at, resolved_at";

/// Input for opening a round when a delivery parks for merge.
#[derive(Debug, Clone)]
pub struct NewQaPass<'a> {
    pub task_id: &'a str,
    pub implementer_agent_id: &'a str,
    pub branch: &'a str,
    pub bound_head: &'a str,
    pub deadline_at: DateTime<Utc>,
    /// Rejected rounds allowed before escalation.
    pub max_rounds: u32,
}

/// What parking did to the task's QA state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QaPassOpen {
    /// A fresh round was dispatched; the caller must create its QA task and
    /// wake the supervisor.
    Dispatched(QaPass),
    /// The same head already has an open round (re-park is idempotent).
    AlreadyOpen(QaPass),
    /// The same head already passed or was waived; merge may proceed.
    AlreadySatisfied(QaPass),
    /// `max_rounds` rejected rounds exist; escalate instead of reopening.
    Escalate { failed_rounds: u32, latest: QaPass },
}

fn open_conn(cas_dir: &Path) -> Result<std::sync::Arc<std::sync::Mutex<Connection>>> {
    let conn = crate::shared_db::shared_connection(&cas_dir.join("cas.db"))?;
    {
        let guard = conn.lock().map_err(lock_err)?;
        ensure_schema(&guard)?;
    }
    Ok(conn)
}

fn lock_err<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Parse("Failed to acquire lock".to_string())
}

/// Create the table and indexes when missing (idempotent).
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    for statement in QA_PASS_SCHEMA_STATEMENTS {
        conn.execute(statement, [])?;
    }
    Ok(())
}

fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
        })
}

fn parse_row(row: &rusqlite::Row) -> rusqlite::Result<QaPass> {
    let state: String = row.get(8)?;
    let state = QaPassState::from_str(&state).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(QaPass {
        id: row.get(0)?,
        task_id: row.get(1)?,
        round: row.get::<_, i64>(2)?.max(1) as u32,
        implementer_agent_id: row.get(3)?,
        branch: row.get(4)?,
        bound_head: row.get(5)?,
        qa_task_id: row.get(6)?,
        reviewer_agent_id: row.get(7)?,
        state,
        summary: row.get(9)?,
        issues_json: row.get(10)?,
        ledger_path: row.get(11)?,
        issuer_agent_id: row.get(12)?,
        requested_at: parse_time(row.get(13)?)?,
        deadline_at: parse_time(row.get(14)?)?,
        resolved_at: row
            .get::<_, Option<String>>(15)?
            .map(parse_time)
            .transpose()?,
    })
}

fn latest_with_conn(conn: &Connection, task_id: &str) -> Result<Option<QaPass>> {
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM qa_passes WHERE task_id = ?1
             ORDER BY requested_at DESC, round DESC, id DESC LIMIT 1"
        ),
        params![task_id],
        parse_row,
    )
    .optional()
    .map_err(StoreError::Database)
}

fn active_with_conn(conn: &Connection, task_id: &str) -> Result<Option<QaPass>> {
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM qa_passes
             WHERE task_id = ?1 AND state IN ('pending', 'claimed') LIMIT 1"
        ),
        params![task_id],
        parse_row,
    )
    .optional()
    .map_err(StoreError::Database)
}

fn by_id_with_conn(conn: &Connection, pass_id: &str) -> Result<QaPass> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM qa_passes WHERE id = ?1"),
        params![pass_id],
        parse_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("QA pass {pass_id}")))
}

fn failed_rounds_with_conn(conn: &Connection, task_id: &str) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM qa_passes WHERE task_id = ?1 AND state = 'failed'",
        params![task_id],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u32)
}

fn set_state(
    conn: &Connection,
    pass_id: &str,
    state: QaPassState,
    resolved_at: Option<DateTime<Utc>>,
) -> Result<()> {
    conn.execute(
        "UPDATE qa_passes SET state = ?2, resolved_at = ?3 WHERE id = ?1",
        params![pass_id, state.as_str(), resolved_at.map(|t| t.to_rfc3339())],
    )?;
    Ok(())
}

/// Time out every open round whose deadline has passed. Readers call this
/// lazily so an expired round never blocks a redispatch forever.
fn expire_with_conn(conn: &Connection, task_id: &str, now: DateTime<Utc>) -> Result<()> {
    if let Some(active) = active_with_conn(conn, task_id)?
        && active.deadline_at <= now
    {
        set_state(conn, &active.id, QaPassState::TimedOut, Some(now))?;
    }
    Ok(())
}

fn new_pass_id() -> String {
    format!("qapass-{:016x}", rand::random::<u64>())
}

/// Open (or re-find) the QA round for a delivery that parked for merge.
///
/// Idempotent per `bound_head`: re-parking the same tip returns the open or
/// already-satisfied round. A different tip supersedes any open round.
pub fn open_qa_pass(cas_dir: &Path, new: &NewQaPass<'_>, now: DateTime<Utc>) -> Result<QaPassOpen> {
    open_qa_pass_reporting_superseded(cas_dir, new, now).map(|(outcome, _)| outcome)
}

/// [`open_qa_pass`], also returning the open round a new tip superseded
/// (cas-ce39), as it reads after the transition (`state == Superseded`).
///
/// A re-park at a new tip retires whatever round was pending or claimed for
/// the old one. The caller owns telling people: cancel the retired round's
/// QA work item and, when a reviewer had claimed it, message the reviewer
/// with the new round, so nobody keeps reviewing a dead head.
pub fn open_qa_pass_reporting_superseded(
    cas_dir: &Path,
    new: &NewQaPass<'_>,
    now: DateTime<Utc>,
) -> Result<(QaPassOpen, Option<QaPass>)> {
    if new.bound_head.trim().is_empty() || new.implementer_agent_id.trim().is_empty() {
        return Err(StoreError::Parse(
            "a QA pass needs the delivered branch tip and its implementer".to_string(),
        ));
    }
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    expire_with_conn(&tx, new.task_id, now)?;

    let mut superseded = None;
    if let Some(active) = active_with_conn(&tx, new.task_id)? {
        if active.bound_head == new.bound_head {
            tx.commit()?;
            return Ok((QaPassOpen::AlreadyOpen(active), None));
        }
        set_state(&tx, &active.id, QaPassState::Superseded, Some(now))?;
        superseded = Some(by_id_with_conn(&tx, &active.id)?);
    }
    if let Some(latest) = latest_with_conn(&tx, new.task_id)?
        && latest.bound_head == new.bound_head
        && latest.state.satisfies_gate()
    {
        tx.commit()?;
        return Ok((QaPassOpen::AlreadySatisfied(latest), superseded));
    }

    let failed = failed_rounds_with_conn(&tx, new.task_id)?;
    if failed >= new.max_rounds.max(1) {
        let latest = latest_with_conn(&tx, new.task_id)?
            .ok_or_else(|| StoreError::Other("failed QA rounds vanished".to_string()))?;
        tx.commit()?;
        return Ok((
            QaPassOpen::Escalate {
                failed_rounds: failed,
                latest,
            },
            superseded,
        ));
    }

    let id = new_pass_id();
    tx.execute(
        "INSERT INTO qa_passes (id, task_id, round, implementer_agent_id, branch, bound_head,
            state, requested_at, deadline_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8)",
        params![
            id,
            new.task_id,
            i64::from(failed + 1),
            new.implementer_agent_id,
            new.branch,
            new.bound_head,
            now.to_rfc3339(),
            new.deadline_at.to_rfc3339(),
        ],
    )?;
    let pass = by_id_with_conn(&tx, &id)?;
    tx.commit()?;
    Ok((QaPassOpen::Dispatched(pass), superseded))
}

/// Link the QA work item the reviewer will start.
pub fn set_qa_task(cas_dir: &Path, pass_id: &str, qa_task_id: &str) -> Result<QaPass> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    conn.execute(
        "UPDATE qa_passes SET qa_task_id = ?2 WHERE id = ?1",
        params![pass_id, qa_task_id],
    )?;
    by_id_with_conn(&conn, pass_id)
}

fn reject_self_review(pass: &QaPass, agent_id: &str, action: &str) -> Result<()> {
    if agent_id.trim().is_empty() {
        return Err(StoreError::Parse(format!(
            "{action} requires a registered agent identity"
        )));
    }
    if agent_id == pass.implementer_agent_id {
        return Err(StoreError::Parse(format!(
            "no self-review: {agent_id} implemented {task} and cannot {action} its independent QA pass {pass}; \
             the supervisor must assign a different (taste-lane) worker",
            task = pass.task_id,
            pass = pass.id,
        )));
    }
    Ok(())
}

/// The reviewer starts the round. Rejects the implementer; idempotent for
/// the same reviewer; rejects a second, different reviewer.
pub fn claim_qa_pass(
    cas_dir: &Path,
    task_id: &str,
    reviewer_agent_id: &str,
    now: DateTime<Utc>,
) -> Result<QaPass> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    expire_with_conn(&tx, task_id, now)?;
    let active = active_with_conn(&tx, task_id)?.ok_or_else(|| {
        StoreError::NotFound(format!("open independent QA pass for {task_id}"))
    })?;
    reject_self_review(&active, reviewer_agent_id, "claim")?;
    match active.reviewer_agent_id.as_deref() {
        Some(existing) if existing == reviewer_agent_id => {}
        Some(existing) => {
            return Err(StoreError::Parse(format!(
                "QA pass {} is already claimed by {existing}",
                active.id
            )));
        }
        None => {
            tx.execute(
                "UPDATE qa_passes SET reviewer_agent_id = ?2, state = 'claimed' WHERE id = ?1",
                params![active.id, reviewer_agent_id],
            )?;
        }
    }
    let pass = by_id_with_conn(&tx, &active.id)?;
    tx.commit()?;
    Ok(pass)
}

/// Return a claimed round to the reviewer queue when its QA work item is
/// reset. The same round and deadline remain in place for the next reviewer.
/// Other task IDs and resolved rounds are left untouched.
pub fn release_qa_claim_for_task(cas_dir: &Path, qa_task_id: &str) -> Result<bool> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    let changed = tx.execute(
        "UPDATE qa_passes SET reviewer_agent_id = NULL, state = 'pending'
         WHERE qa_task_id = ?1 AND state = 'claimed'",
        params![qa_task_id],
    )?;
    tx.commit()?;
    Ok(changed > 0)
}

/// Guard used by task start/claim of a QA work item: the implementer of the
/// delivery under review may never take it.
pub fn assert_may_review_qa_task(
    cas_dir: &Path,
    qa_task_id: &str,
    agent_id: &str,
) -> Result<Option<QaPass>> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let pass = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM qa_passes WHERE qa_task_id = ?1
                 ORDER BY requested_at DESC LIMIT 1"
            ),
            params![qa_task_id],
            parse_row,
        )
        .optional()?;
    if let Some(pass) = &pass {
        reject_self_review(pass, agent_id, "start")?;
    }
    Ok(pass)
}

/// Record the reviewer's verdict on the open round for `task_id`.
pub fn resolve_qa_pass(
    cas_dir: &Path,
    task_id: &str,
    reviewer_agent_id: &str,
    verdict: QaVerdict,
    summary: &str,
    issues_json: Option<&str>,
    ledger_path: &str,
    now: DateTime<Utc>,
) -> Result<QaPass> {
    if summary.trim().is_empty() {
        return Err(StoreError::Parse("a QA verdict needs a summary".to_string()));
    }
    if ledger_path.trim().is_empty() {
        return Err(StoreError::Parse(
            "a QA verdict must cite its LEDGER.md (ledger_path)".to_string(),
        ));
    }
    if let Some(issues) = issues_json {
        let parsed: serde_json::Value = serde_json::from_str(issues)?;
        if !parsed.is_array() {
            return Err(StoreError::Parse("issues must be a JSON array".to_string()));
        }
    }
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    expire_with_conn(&tx, task_id, now)?;
    let active = active_with_conn(&tx, task_id)?.ok_or_else(|| {
        StoreError::NotFound(format!(
            "open independent QA pass for {task_id} (it may have timed out or been superseded)"
        ))
    })?;
    reject_self_review(&active, reviewer_agent_id, "resolve")?;
    if active.reviewer_agent_id.as_deref() != Some(reviewer_agent_id) {
        return Err(StoreError::Parse(format!(
            "only the reviewer who claimed QA pass {} may record its verdict",
            active.id
        )));
    }
    let state = match verdict {
        QaVerdict::Approved => QaPassState::Passed,
        QaVerdict::Rejected => QaPassState::Failed,
    };
    tx.execute(
        "UPDATE qa_passes SET state = ?2, summary = ?3, issues_json = ?4, ledger_path = ?5,
            resolved_at = ?6
         WHERE id = ?1",
        params![
            active.id,
            state.as_str(),
            summary.trim(),
            issues_json,
            ledger_path.trim(),
            now.to_rfc3339(),
        ],
    )?;
    let pass = by_id_with_conn(&tx, &active.id)?;
    tx.commit()?;
    Ok(pass)
}

/// Supervisor waiver for one exact tip. Supersedes any open round.
pub fn waive_qa_pass(
    cas_dir: &Path,
    task_id: &str,
    supervisor_agent_id: &str,
    implementer_agent_id: &str,
    branch: &str,
    bound_head: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<QaPass> {
    if reason.trim().is_empty() {
        return Err(StoreError::Parse("a QA waiver needs a reason".to_string()));
    }
    if supervisor_agent_id.trim().is_empty() || bound_head.trim().is_empty() {
        return Err(StoreError::Parse(
            "a QA waiver needs the supervisor identity and the waived branch tip".to_string(),
        ));
    }
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    if let Some(active) = active_with_conn(&tx, task_id)? {
        set_state(&tx, &active.id, QaPassState::Superseded, Some(now))?;
    }
    let round = failed_rounds_with_conn(&tx, task_id)? + 1;
    let id = new_pass_id();
    tx.execute(
        "INSERT INTO qa_passes (id, task_id, round, implementer_agent_id, branch, bound_head,
            state, summary, issuer_agent_id, requested_at, deadline_at, resolved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'waived', ?7, ?8, ?9, ?9, ?9)",
        params![
            id,
            task_id,
            i64::from(round),
            implementer_agent_id,
            branch,
            bound_head,
            reason.trim(),
            supervisor_agent_id,
            now.to_rfc3339(),
        ],
    )?;
    let pass = by_id_with_conn(&tx, &id)?;
    tx.commit()?;
    Ok(pass)
}

/// Withdraw the open round for `task_id` because the delivery no longer needs
/// independent QA (cas-5c38). Only an unclaimed (pending) round is withdrawn
/// unless `include_claimed`; a reviewer already at work keeps its round. The
/// row is kept as `superseded` with a [`QA_PASS_WITHDRAWN_PREFIX`] summary so
/// the audit trail survives while the gates stop counting it.
pub fn withdraw_open_qa_pass(
    cas_dir: &Path,
    task_id: &str,
    reason: &str,
    include_claimed: bool,
    now: DateTime<Utc>,
) -> Result<Option<QaPass>> {
    if reason.trim().is_empty() {
        return Err(StoreError::Parse(
            "withdrawing a QA round needs a reason".to_string(),
        ));
    }
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let tx = ImmediateTx::new(&conn)?;
    expire_with_conn(&tx, task_id, now)?;
    let Some(active) = active_with_conn(&tx, task_id)? else {
        tx.commit()?;
        return Ok(None);
    };
    if active.state == QaPassState::Claimed && !include_claimed {
        tx.commit()?;
        return Ok(None);
    }
    tx.execute(
        "UPDATE qa_passes SET state = 'superseded', summary = ?2, resolved_at = ?3 WHERE id = ?1",
        params![
            active.id,
            format!("{QA_PASS_WITHDRAWN_PREFIX}{}", reason.trim()),
            now.to_rfc3339(),
        ],
    )?;
    let pass = by_id_with_conn(&tx, &active.id)?;
    tx.commit()?;
    Ok(Some(pass))
}

/// Latest round for a task, after lazily timing out an expired one.
pub fn latest_qa_pass(cas_dir: &Path, task_id: &str, now: DateTime<Utc>) -> Result<Option<QaPass>> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    expire_with_conn(&conn, task_id, now)?;
    latest_with_conn(&conn, task_id)
}

/// Passed or waived round for exactly `head`, if any.
pub fn satisfying_qa_pass_for_head(
    cas_dir: &Path,
    task_id: &str,
    head: &str,
) -> Result<Option<QaPass>> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM qa_passes
             WHERE task_id = ?1 AND bound_head = ?2 AND state IN ('passed', 'waived')
             ORDER BY requested_at DESC LIMIT 1"
        ),
        params![task_id, head],
        parse_row,
    )
    .optional()
    .map_err(StoreError::Database)
}

/// Every passed or waived round for a task, newest first (used by the close
/// backstop to test ancestry against the merged target).
pub fn satisfying_qa_passes(cas_dir: &Path, task_id: &str) -> Result<Vec<QaPass>> {
    list_filtered(cas_dir, task_id, true)
}

/// All rounds for a task, newest first.
pub fn list_qa_passes(cas_dir: &Path, task_id: &str) -> Result<Vec<QaPass>> {
    list_filtered(cas_dir, task_id, false)
}

fn list_filtered(cas_dir: &Path, task_id: &str, satisfying_only: bool) -> Result<Vec<QaPass>> {
    let conn = open_conn(cas_dir)?;
    let conn = conn.lock().map_err(lock_err)?;
    let filter = if satisfying_only {
        "AND state IN ('passed', 'waived')"
    } else {
        ""
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM qa_passes WHERE task_id = ?1 {filter}
         ORDER BY requested_at DESC, round DESC, id DESC"
    ))?;
    let rows = stmt
        .query_map(params![task_id], parse_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    fn new<'a>(head: &'a str, now: DateTime<Utc>) -> NewQaPass<'a> {
        NewQaPass {
            task_id: "cas-ui1",
            implementer_agent_id: "impl-worker",
            branch: "factory/impl-worker",
            bound_head: head,
            deadline_at: now + Duration::minutes(45),
            max_rounds: 3,
        }
    }

    fn dispatched(outcome: QaPassOpen) -> QaPass {
        match outcome {
            QaPassOpen::Dispatched(pass) => pass,
            other => panic!("expected a dispatch, got {other:?}"),
        }
    }

    #[test]
    fn park_dispatches_once_per_head_and_supersedes_on_drift() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let first = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        assert_eq!(first.round, 1);
        assert_eq!(first.state, QaPassState::Pending);

        match open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap() {
            QaPassOpen::AlreadyOpen(pass) => assert_eq!(pass.id, first.id),
            other => panic!("re-park must be idempotent, got {other:?}"),
        }

        let second = dispatched(open_qa_pass(dir.path(), &new("bbbb2222", now), now).unwrap());
        assert_ne!(second.id, first.id);
        assert_eq!(second.round, 1, "drift is not a rejected round");
        let passes = list_qa_passes(dir.path(), "cas-ui1").unwrap();
        assert_eq!(passes.len(), 2);
        assert_eq!(
            passes.iter().find(|p| p.id == first.id).unwrap().state,
            QaPassState::Superseded
        );
    }

    /// cas-ce39: a re-park at a new tip reports the round it retired, pending
    /// or claimed, so the caller can cancel its work item and tell the
    /// reviewer. The same tip retires nothing.
    #[test]
    fn a_new_tip_reports_the_pending_round_it_supersedes_cas_ce39() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let (first, retired) =
            open_qa_pass_reporting_superseded(dir.path(), &new("aaaa1111", now), now).unwrap();
        let first = dispatched(first);
        assert!(retired.is_none(), "the first round supersedes nothing");

        let (same, retired) =
            open_qa_pass_reporting_superseded(dir.path(), &new("aaaa1111", now), now).unwrap();
        assert!(matches!(same, QaPassOpen::AlreadyOpen(ref pass) if pass.id == first.id));
        assert!(retired.is_none(), "re-parking the same tip retires nothing");

        let (second, retired) =
            open_qa_pass_reporting_superseded(dir.path(), &new("bbbb2222", now), now).unwrap();
        let second = dispatched(second);
        let retired = retired.expect("the pending round is reported");
        assert_eq!(retired.id, first.id);
        assert_eq!(retired.state, QaPassState::Superseded);
        assert_eq!(retired.bound_head, "aaaa1111");
        assert!(retired.reviewer_agent_id.is_none(), "pending: nobody to tell");
        assert_eq!(second.bound_head, "bbbb2222");
    }

    #[test]
    fn a_new_tip_reports_the_claimed_round_and_its_reviewer_cas_ce39() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let first = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        let claimed = claim_qa_pass(dir.path(), "cas-ui1", "reviewer-worker", now).unwrap();
        assert_eq!(claimed.state, QaPassState::Claimed);

        let (second, retired) =
            open_qa_pass_reporting_superseded(dir.path(), &new("bbbb2222", now), now).unwrap();
        let second = dispatched(second);
        let retired = retired.expect("the claimed round is reported");
        assert_eq!(retired.id, first.id);
        assert_eq!(retired.state, QaPassState::Superseded);
        assert_eq!(retired.reviewer_agent_id.as_deref(), Some("reviewer-worker"));
        assert_ne!(second.id, first.id);
        // The retired round can no longer take the old reviewer's verdict.
        let active = list_qa_passes(dir.path(), "cas-ui1")
            .unwrap()
            .into_iter()
            .filter(|pass| pass.state.is_active())
            .collect::<Vec<_>>();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, second.id);
    }

    #[test]
    fn reset_qa_task_releases_claim_for_a_replacement_reviewer_cas_1aef3() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let opened = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        set_qa_task(dir.path(), &opened.id, "cas-qa1").unwrap();
        claim_qa_pass(dir.path(), "cas-ui1", "dead-reviewer", now).unwrap();

        let second = claim_qa_pass(dir.path(), "cas-ui1", "replacement", now).unwrap_err();
        assert!(second.to_string().contains("already claimed"), "{second}");

        let released = release_qa_claim_for_task(dir.path(), "cas-qa1").unwrap();
        assert!(released, "reset releases the QA work item's active claim");
        let pending = latest_qa_pass(dir.path(), "cas-ui1", now).unwrap().unwrap();
        assert_eq!(pending.id, opened.id, "reset keeps the same round");
        assert_eq!(pending.state, QaPassState::Pending);
        assert!(pending.reviewer_agent_id.is_none());
        assert_eq!(pending.deadline_at, opened.deadline_at);

        let reclaimed = claim_qa_pass(dir.path(), "cas-ui1", "replacement", now).unwrap();
        assert_eq!(reclaimed.id, opened.id);
        assert_eq!(reclaimed.state, QaPassState::Claimed);
        assert_eq!(reclaimed.reviewer_agent_id.as_deref(), Some("replacement"));
    }

    #[test]
    fn withdrawal_takes_only_an_unclaimed_round_unless_asked_cas_5c38() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        assert!(withdraw_open_qa_pass(dir.path(), "cas-ui1", "cleared", false, now)
            .unwrap()
            .is_none());
        assert!(withdraw_open_qa_pass(dir.path(), "cas-ui1", "  ", false, now).is_err());

        let first = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        let withdrawn = withdraw_open_qa_pass(dir.path(), "cas-ui1", "demo cleared", false, now)
            .unwrap()
            .expect("a pending round is withdrawn");
        assert_eq!(withdrawn.id, first.id);
        assert!(withdrawn.is_withdrawn());
        assert_eq!(withdrawn.summary.as_deref(), Some("withdrawn: demo cleared"));

        // A reviewer already at work keeps its round unless asked.
        let second = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        claim_qa_pass(dir.path(), "cas-ui1", "qa-worker", now).unwrap();
        assert!(withdraw_open_qa_pass(dir.path(), "cas-ui1", "demo cleared", false, now)
            .unwrap()
            .is_none());
        let taken = withdraw_open_qa_pass(dir.path(), "cas-ui1", "no-code", true, now)
            .unwrap()
            .expect("an explicit withdrawal takes a claimed round too");
        assert_eq!(taken.id, second.id);
        assert!(taken.is_withdrawn());
    }

    #[test]
    fn implementer_can_neither_claim_nor_resolve() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());

        let claim = claim_qa_pass(dir.path(), "cas-ui1", "impl-worker", now).unwrap_err();
        assert!(claim.to_string().contains("no self-review"), "{claim}");

        let resolve = resolve_qa_pass(
            dir.path(),
            "cas-ui1",
            "impl-worker",
            QaVerdict::Approved,
            "looks fine",
            None,
            "/tmp/LEDGER.md",
            now,
        )
        .unwrap_err();
        assert!(resolve.to_string().contains("no self-review"), "{resolve}");

        let claimed = claim_qa_pass(dir.path(), "cas-ui1", "qa-worker", now).unwrap();
        assert_eq!(claimed.state, QaPassState::Claimed);
        let other = claim_qa_pass(dir.path(), "cas-ui1", "someone-else", now).unwrap_err();
        assert!(other.to_string().contains("already claimed"), "{other}");
        let unclaimed = resolve_qa_pass(
            dir.path(),
            "cas-ui1",
            "someone-else",
            QaVerdict::Approved,
            "ok",
            None,
            "/tmp/LEDGER.md",
            now,
        )
        .unwrap_err();
        assert!(unclaimed.to_string().contains("only the reviewer"), "{unclaimed}");
    }

    #[test]
    fn verdicts_bind_to_the_head_and_count_rounds() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        claim_qa_pass(dir.path(), "cas-ui1", "qa-worker", now).unwrap();
        let failed = resolve_qa_pass(
            dir.path(),
            "cas-ui1",
            "qa-worker",
            QaVerdict::Rejected,
            "empty state crashes",
            Some(r#"[{"severity":"blocking","problem":"crash"}]"#),
            "/tmp/qa/round-1/LEDGER.md",
            now,
        )
        .unwrap();
        assert_eq!(failed.state, QaPassState::Failed);
        assert!(
            satisfying_qa_pass_for_head(dir.path(), "cas-ui1", "aaaa1111")
                .unwrap()
                .is_none()
        );

        let round2 = dispatched(open_qa_pass(dir.path(), &new("cccc3333", now), now).unwrap());
        assert_eq!(round2.round, 2);
        claim_qa_pass(dir.path(), "cas-ui1", "qa-worker", now).unwrap();
        resolve_qa_pass(
            dir.path(),
            "cas-ui1",
            "qa-worker",
            QaVerdict::Approved,
            "clean",
            None,
            "/tmp/qa/round-2/LEDGER.md",
            now,
        )
        .unwrap();
        let passing = satisfying_qa_pass_for_head(dir.path(), "cas-ui1", "cccc3333")
            .unwrap()
            .expect("approved head satisfies the gate");
        assert_eq!(passing.state, QaPassState::Passed);
        match open_qa_pass(dir.path(), &new("cccc3333", now), now).unwrap() {
            QaPassOpen::AlreadySatisfied(pass) => assert_eq!(pass.id, passing.id),
            other => panic!("a passed head must not redispatch, got {other:?}"),
        }
    }

    #[test]
    fn rounds_escalate_after_max_rejections() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        for (round, head) in ["h1", "h2", "h3"].into_iter().enumerate() {
            let pass = dispatched(open_qa_pass(dir.path(), &new(head, now), now).unwrap());
            assert_eq!(pass.round as usize, round + 1);
            claim_qa_pass(dir.path(), "cas-ui1", "qa-worker", now).unwrap();
            resolve_qa_pass(
                dir.path(),
                "cas-ui1",
                "qa-worker",
                QaVerdict::Rejected,
                "still broken",
                None,
                "/tmp/LEDGER.md",
                now,
            )
            .unwrap();
        }
        match open_qa_pass(dir.path(), &new("h4", now), now).unwrap() {
            QaPassOpen::Escalate { failed_rounds, .. } => assert_eq!(failed_rounds, 3),
            other => panic!("fourth park must escalate, got {other:?}"),
        }
    }

    #[test]
    fn expired_rounds_time_out_and_waivers_satisfy_their_head() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        let later = now + Duration::minutes(46);
        let latest = latest_qa_pass(dir.path(), "cas-ui1", later).unwrap().unwrap();
        assert_eq!(latest.state, QaPassState::TimedOut);

        let err = waive_qa_pass(
            dir.path(),
            "cas-ui1",
            "supervisor-1",
            "impl-worker",
            "factory/impl-worker",
            "aaaa1111",
            " ",
            later,
        )
        .unwrap_err();
        assert!(err.to_string().contains("reason"));
        let waived = waive_qa_pass(
            dir.path(),
            "cas-ui1",
            "supervisor-1",
            "impl-worker",
            "factory/impl-worker",
            "aaaa1111",
            "copy-only change reviewed in person",
            later,
        )
        .unwrap();
        assert_eq!(waived.state, QaPassState::Waived);
        assert_eq!(waived.issuer_agent_id.as_deref(), Some("supervisor-1"));
        assert!(
            satisfying_qa_pass_for_head(dir.path(), "cas-ui1", "aaaa1111")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn qa_task_start_guard_rejects_the_implementer() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let pass = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        set_qa_task(dir.path(), &pass.id, "cas-qa01").unwrap();
        let err = assert_may_review_qa_task(dir.path(), "cas-qa01", "impl-worker").unwrap_err();
        assert!(err.to_string().contains("no self-review"), "{err}");
        let ok = assert_may_review_qa_task(dir.path(), "cas-qa01", "qa-worker").unwrap();
        assert_eq!(ok.unwrap().id, pass.id);
        assert!(
            assert_may_review_qa_task(dir.path(), "cas-unrelated", "impl-worker")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn verdict_requires_summary_ledger_and_array_issues() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        claim_qa_pass(dir.path(), "cas-ui1", "qa-worker", now).unwrap();
        let resolve = |summary: &str, issues: Option<&str>, ledger: &str| {
            resolve_qa_pass(
                dir.path(),
                "cas-ui1",
                "qa-worker",
                QaVerdict::Approved,
                summary,
                issues,
                ledger,
                now,
            )
        };
        assert!(resolve("", None, "/tmp/LEDGER.md").is_err());
        assert!(resolve("ok", None, "").is_err());
        assert!(resolve("ok", Some(r#"{"not":"array"}"#), "/tmp/LEDGER.md").is_err());
        assert!(resolve("ok", Some("[]"), "/tmp/LEDGER.md").is_ok());
    }
}
