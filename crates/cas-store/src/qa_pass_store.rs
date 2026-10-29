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
        let guard = crate::shared_db::lock_connection(&conn)?;
        ensure_schema(&guard)?;
    }
    Ok(conn)
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
    let conn = crate::shared_db::lock_connection(&conn)?;
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
    let conn = crate::shared_db::lock_connection(&conn)?;
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
/// the same reviewer; rejects a second reviewer while the claimant is live.
pub fn claim_qa_pass(
    cas_dir: &Path,
    task_id: &str,
    reviewer_agent_id: &str,
    now: DateTime<Utc>,
) -> Result<QaPass> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let tx = ImmediateTx::new(&conn)?;
    expire_with_conn(&tx, task_id, now)?;
    let active = active_with_conn(&tx, task_id)?
        .ok_or_else(|| StoreError::NotFound(format!("open independent QA pass for {task_id}")))?;
    reject_self_review(&active, reviewer_agent_id, "claim")?;
    if let Some(existing) = active.reviewer_agent_id.as_deref()
        && existing != reviewer_agent_id
        && reviewer_is_live_with_conn(&tx, existing)?
    {
        return Err(StoreError::Other(format!(
            "QA pass {} is already claimed by {existing}",
            active.id
        )));
    }
    tx.execute(
        "UPDATE qa_passes SET reviewer_agent_id = ?2, state = 'claimed' WHERE id = ?1",
        params![active.id, reviewer_agent_id],
    )?;
    let pass = by_id_with_conn(&tx, &active.id)?;
    tx.commit()?;
    Ok(pass)
}

// Resolve the same two identity forms accepted by task start. A registry-free
// QA store cannot disprove ownership; with a registry, absent/retired owners
// have no authority to keep a round. Read under the claim's write transaction.
fn reviewer_is_live_with_conn(conn: &Connection, reviewer: &str) -> Result<bool> {
    if !conn.table_exists(None, "agents")? {
        return Ok(true);
    }
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM agents WHERE id = ?1 OR name = ?1
         ORDER BY (id = ?1) DESC, registered_at DESC LIMIT 1",
            params![reviewer],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(status.as_deref(), Some("active" | "idle")))
}

/// Release all claims owned by a retiring registration, including claims whose
/// task lease was already released. Call before deleting its registry row and
/// within the retirement transaction. A live replacement with the same display
/// name owns name-bound claims; retiring the old ID must not release those.
pub(crate) fn release_qa_claims_for_agent_with_conn(
    conn: &Connection,
    agent_id: &str,
) -> Result<()> {
    if conn.table_exists(None, "qa_passes")? {
        conn.execute(
            "UPDATE qa_passes SET reviewer_agent_id = NULL, state = 'pending'
             WHERE state = 'claimed' AND (reviewer_agent_id = ?1 OR
                 reviewer_agent_id IN (SELECT retiring.name FROM agents retiring
                     WHERE retiring.id = ?1 AND NOT EXISTS (
                         SELECT 1 FROM agents replacement
                         WHERE replacement.name = retiring.name AND replacement.id != ?1
                           AND replacement.status IN ('active', 'idle'))))",
            params![agent_id],
        )?;
    }
    Ok(())
}

/// Return a claimed round to the reviewer queue when its QA work item is
/// reset. The same round and deadline remain in place for the next reviewer.
/// Other task IDs and resolved rounds are left untouched.
pub fn release_qa_claim_for_task(cas_dir: &Path, qa_task_id: &str) -> Result<bool> {
    release_qa_claim(cas_dir, qa_task_id, None)
}

/// Recovery may release only the reviewer whose task binding it is clearing.
/// Refuse a different claimant instead of dropping a replacement's review.
pub fn release_qa_claim_for_reviewer(
    cas_dir: &Path,
    qa_task_id: &str,
    reviewer: &str,
) -> Result<bool> {
    release_qa_claim(cas_dir, qa_task_id, Some(reviewer))
}

fn release_qa_claim(cas_dir: &Path, qa_task_id: &str, reviewer: Option<&str>) -> Result<bool> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let tx = ImmediateTx::new(&conn)?;
    if let Some(reviewer) = reviewer {
        let conflicting: Option<String> = tx
            .query_row(
                "SELECT reviewer_agent_id FROM qa_passes
             WHERE qa_task_id = ?1 AND state = 'claimed' AND reviewer_agent_id != ?2",
                params![qa_task_id, reviewer],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = conflicting {
            return Err(StoreError::Other(format!(
                "QA task {qa_task_id} claim changed from '{reviewer}' to '{existing}'; recovery refused"
            )));
        }
    }
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
    let conn = crate::shared_db::lock_connection(&conn)?;
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
    let conn = crate::shared_db::lock_connection(&conn)?;
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
    waive_qa_pass_reporting_superseded(cas_dir, task_id, supervisor_agent_id,
        implementer_agent_id, branch, bound_head, reason, now).map(|(pass, _)| pass)
}

/// Record the waiver and report the retired round so lifecycle callers can
/// cancel its work item, just as they do after a head-advance supersession.
pub fn waive_qa_pass_reporting_superseded(
    cas_dir: &Path,
    task_id: &str,
    supervisor_agent_id: &str,
    implementer_agent_id: &str,
    branch: &str,
    bound_head: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(QaPass, Option<QaPass>)> {
    if reason.trim().is_empty() {
        return Err(StoreError::Parse("a QA waiver needs a reason".to_string()));
    }
    if supervisor_agent_id.trim().is_empty() || bound_head.trim().is_empty() {
        return Err(StoreError::Parse(
            "a QA waiver needs the supervisor identity and the waived branch tip".to_string(),
        ));
    }
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let tx = ImmediateTx::new(&conn)?;
    let mut retired = None;
    if let Some(active) = active_with_conn(&tx, task_id)? {
        set_state(&tx, &active.id, QaPassState::Superseded, Some(now))?;
        retired = Some(by_id_with_conn(&tx, &active.id)?);
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
    Ok((pass, retired))
}

/// Keep a shared work item if any round still needs it or records a verdict.
/// Deadline expiry is deliberately not applied by this read-only predicate.
pub fn qa_task_has_current_round(cas_dir: &Path, qa_task_id: &str) -> Result<bool> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM qa_passes WHERE qa_task_id = ?1
         AND state NOT IN ('superseded', 'waived'))",
        params![qa_task_id], |row| row.get(0),
    )?)
}

/// Read a bounded page of obsolete work items for exactly one delivery. Each
/// child appears once; references from any non-obsolete round protect it.
pub fn obsolete_qa_task_passes(cas_dir: &Path, task_id: &str, limit: usize) -> Result<Vec<QaPass>> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let columns = COLUMNS.split(',').map(|column| format!("p.{}", column.trim()))
        .collect::<Vec<_>>().join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT {columns} FROM qa_passes p JOIN tasks t ON t.id = p.qa_task_id
         WHERE p.task_id = ?1 AND p.state IN ('superseded', 'waived')
           AND t.status NOT IN ('closed', 'cancelled')
           AND EXISTS(SELECT 1 FROM json_each(t.labels) WHERE value = 'qa-pass')
           AND NOT EXISTS(SELECT 1 FROM qa_passes current
             WHERE current.qa_task_id = p.qa_task_id
               AND current.state NOT IN ('superseded', 'waived'))
           AND p.rowid = (SELECT newest.rowid FROM qa_passes newest
             WHERE newest.qa_task_id = p.qa_task_id
             ORDER BY newest.requested_at DESC, newest.rowid DESC LIMIT 1)
         ORDER BY p.requested_at, p.rowid LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![task_id, limit.clamp(1, 100) as i64], parse_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(StoreError::Database)
}

/// cas-0c988: carry a recorded verdict to a rebased tip of the same delivery.
///
/// The caller has proved that `bound_head` carries the reviewed source patch
/// byte for byte (only generated build output differs). A new row records the
/// verdict at `bound_head` with the original state, reviewer, QA task, ledger
/// and findings, and a summary naming the round it came from, so the audit
/// trail shows the carry rather than a fresh review. Only a passed or waived
/// round carries.
pub fn carry_qa_pass(
    cas_dir: &Path,
    from: &QaPass,
    bound_head: &str,
    branch: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<QaPass> {
    if !from.state.satisfies_gate() {
        return Err(StoreError::Parse(format!(
            "only a passed or waived QA round carries to a new tip; pass {} is {}",
            from.id, from.state
        )));
    }
    if bound_head.trim().is_empty() || reason.trim().is_empty() {
        return Err(StoreError::Parse(
            "a carried QA verdict needs the new tip and the reason it is the same delivery"
                .to_string(),
        ));
    }
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let tx = ImmediateTx::new(&conn)?;
    if let Some(active) = active_with_conn(&tx, &from.task_id)? {
        set_state(&tx, &active.id, QaPassState::Superseded, Some(now))?;
    }
    let id = new_pass_id();
    let head8 = &from.bound_head[..from.bound_head.len().min(8)];
    let summary = format!(
        "carried over from @{head8} (pass {}, round {}, {}): {}. Original verdict: {}",
        from.id,
        from.round,
        from.state,
        reason.trim(),
        from.summary.as_deref().unwrap_or("(none recorded)"),
    );
    tx.execute(
        "INSERT INTO qa_passes (id, task_id, round, implementer_agent_id, branch, bound_head,
            qa_task_id, reviewer_agent_id, state, summary, issues_json, ledger_path,
            issuer_agent_id, requested_at, deadline_at, resolved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?14)",
        params![
            id,
            from.task_id,
            i64::from(from.round),
            from.implementer_agent_id,
            branch,
            bound_head,
            from.qa_task_id,
            from.reviewer_agent_id,
            from.state.as_str(),
            summary,
            from.issues_json,
            from.ledger_path,
            from.issuer_agent_id,
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
    let conn = crate::shared_db::lock_connection(&conn)?;
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

/// Withdraw the open round whose QA work item is `qa_task_id` (cas-7877).
///
/// Cancelling the work item is the supervisor saying this review will not
/// happen. Leaving its pass pending kept the delivery gated and made the next
/// park open a "re-review" of a round nobody ran. Only a pending or claimed
/// round is withdrawn; a recorded verdict stands.
pub fn withdraw_qa_pass_for_qa_task(
    cas_dir: &Path,
    qa_task_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<Option<QaPass>> {
    if reason.trim().is_empty() {
        return Err(StoreError::Parse(
            "withdrawing a QA round needs a reason".to_string(),
        ));
    }
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let tx = ImmediateTx::new(&conn)?;
    let active = tx
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM qa_passes
                 WHERE qa_task_id = ?1 AND state IN ('pending', 'claimed') LIMIT 1"
            ),
            params![qa_task_id],
            parse_row,
        )
        .optional()?;
    let Some(active) = active else {
        tx.commit()?;
        return Ok(None);
    };
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
    let conn = crate::shared_db::lock_connection(&conn)?;
    expire_with_conn(&conn, task_id, now)?;
    latest_with_conn(&conn, task_id)
}

/// Reviewer identities and round numbers for tasks whose latest QA round failed.
/// Callers apply task-lifecycle and configured round-limit policy.
pub fn latest_failed_qa_reviewers(
    cas_dir: &Path,
    task_ids: &[String],
) -> Result<std::collections::HashMap<String, (String, u32)>> {
    if task_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let placeholders = (1..=task_ids.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT current.task_id, current.reviewer_agent_id, current.round
         FROM qa_passes current
         WHERE current.task_id IN ({placeholders})
           AND current.state = 'failed'
           AND current.reviewer_agent_id IS NOT NULL
           AND NOT EXISTS (
             SELECT 1 FROM qa_passes newer
             WHERE newer.task_id = current.task_id
               AND (newer.requested_at > current.requested_at
                 OR (newer.requested_at = current.requested_at AND newer.round > current.round)
                 OR (newer.requested_at = current.requested_at AND newer.round = current.round
                     AND newer.id > current.id))
           )"
    ))?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(task_ids.iter()),
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, u32>(2)?),
            ))
        },
    )?;
    rows.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
        .map_err(StoreError::Database)
}

/// Passed or waived round for exactly `head`, if any.
pub fn satisfying_qa_pass_for_head(
    cas_dir: &Path,
    task_id: &str,
    head: &str,
) -> Result<Option<QaPass>> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
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
    let conn = crate::shared_db::lock_connection(&conn)?;
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

/// Import a trusted owner-scoped QA snapshot. This is deliberately separate
/// from task verifications and from the ordinary task-note write interface.
/// Immutable delivery identity and no-self-review also apply on a peer host.
pub fn import_qa_pass(cas_dir: &Path, pass: &QaPass, changed_at: DateTime<Utc>) -> Result<()> {
    if pass.id.is_empty() || pass.task_id.is_empty() || pass.bound_head.is_empty()
        || pass.implementer_agent_id.is_empty() || pass.branch.is_empty() || pass.round == 0
        || pass.reviewer_agent_id.as_deref() == Some(pass.implementer_agent_id.as_str())
        || (matches!(pass.state, QaPassState::Passed | QaPassState::Failed)
            && (pass.reviewer_agent_id.as_deref().is_none_or(str::is_empty)
                || pass.ledger_path.as_deref().is_none_or(str::is_empty)
                || pass.resolved_at.is_none()))
        || (pass.state == QaPassState::Waived
            && (pass.issuer_agent_id.as_deref().is_none_or(str::is_empty) || pass.resolved_at.is_none()))
    {
        return Err(StoreError::Parse("invalid independent QA snapshot".into()));
    }
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let tx = ImmediateTx::new(&conn)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS qa_pass_sync_versions (
        id TEXT PRIMARY KEY, changed_at TEXT NOT NULL)")?;
    let previous = match by_id_with_conn(&tx, &pass.id) {
        Ok(previous) => Some(previous),
        Err(StoreError::NotFound(_)) => None,
        Err(error) => return Err(error),
    };
    if let Some(previous) = &previous {
        if previous.task_id != pass.task_id || previous.bound_head != pass.bound_head
            || previous.branch != pass.branch || previous.implementer_agent_id != pass.implementer_agent_id
            || previous.requested_at != pass.requested_at || previous.round != pass.round
            || previous.deadline_at != pass.deadline_at
            || (previous.qa_task_id.is_some() && previous.qa_task_id != pass.qa_task_id)
        {
            return Err(StoreError::Parse("QA snapshot changes immutable delivery identity".into()));
        }
        let version: Option<String> = tx.query_row(
            "SELECT changed_at FROM qa_pass_sync_versions WHERE id = ?1", [&pass.id],
            |row| row.get(0)).optional()?;
        let old_time = version.map(parse_time).transpose()?.unwrap_or(
            previous.resolved_at.unwrap_or(previous.requested_at));
        // Terminal decisions never regress to an open round, including a
        // stale source snapshot delivered after a local deadline transition.
        if (!previous.state.is_active() && pass.state.is_active()) || changed_at < old_time
            || previous.resolved_at > pass.resolved_at && !previous.state.is_active()
        {
            return Ok(());
        }
        if changed_at == old_time && previous != pass {
            return Err(StoreError::Parse("conflicting independent QA snapshots".into()));
        }
    }
    if let Some(active) = active_with_conn(&tx, &pass.task_id)?
        && active.id != pass.id
    {
        // A source verdict replaces a duplicate peer dispatch for the same
        // delivery. A stale active round cannot displace a newer dispatch.
        if pass.state.is_active() && active.requested_at >= pass.requested_at {
            return Ok(());
        }
        if pass.state.is_active() || active.bound_head == pass.bound_head {
            set_state(&tx, &active.id, QaPassState::Superseded, Some(changed_at))?;
        }
    }
    tx.execute(
        "INSERT INTO qa_passes (id, task_id, round, implementer_agent_id, branch, bound_head,
         qa_task_id, reviewer_agent_id, state, summary, issues_json, ledger_path, issuer_agent_id,
         requested_at, deadline_at, resolved_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(id) DO UPDATE SET qa_task_id=excluded.qa_task_id,
         reviewer_agent_id=excluded.reviewer_agent_id, state=excluded.state,
         summary=excluded.summary, issues_json=excluded.issues_json, ledger_path=excluded.ledger_path,
         issuer_agent_id=excluded.issuer_agent_id, resolved_at=excluded.resolved_at",
        params![pass.id, pass.task_id, pass.round, pass.implementer_agent_id, pass.branch,
            pass.bound_head, pass.qa_task_id, pass.reviewer_agent_id, pass.state.as_str(),
            pass.summary, pass.issues_json, pass.ledger_path, pass.issuer_agent_id,
            pass.requested_at.to_rfc3339(), pass.deadline_at.to_rfc3339(),
            pass.resolved_at.map(|time| time.to_rfc3339())],
    )?;
    tx.execute("INSERT INTO qa_pass_sync_versions VALUES (?1,?2)
        ON CONFLICT(id) DO UPDATE SET changed_at=excluded.changed_at",
        params![pass.id, changed_at.to_rfc3339()])?;
    tx.commit()?;
    Ok(())
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
    fn peer_snapshots_preserve_verdict_identity_and_do_not_revive_open_rounds() {
        let source = TempDir::new().unwrap();
        let peer = TempDir::new().unwrap();
        let now = Utc::now();
        let pending = dispatched(open_qa_pass(source.path(), &new("aaaa1111", now), now).unwrap());
        set_qa_task(source.path(), &pending.id, "cas-review").unwrap();
        let claimed = claim_qa_pass(source.path(), "cas-ui1", "reviewer", now).unwrap();
        let passed = resolve_qa_pass(source.path(), "cas-ui1", "reviewer", QaVerdict::Approved,
            "PASS", None, "/other/LEDGER.md", now + Duration::seconds(1)).unwrap();
        for (index, pass) in [&pending, &claimed, &passed].into_iter().enumerate() {
            import_qa_pass(peer.path(), pass, now + Duration::milliseconds(index as i64 * 500)).unwrap();
        }
        import_qa_pass(peer.path(), &claimed, now + Duration::hours(1)).unwrap();
        assert_eq!(list_qa_passes(peer.path(), "cas-ui1").unwrap(), vec![passed.clone()]);
        for kind in ["self-review", "head", "implementer", "task", "deadline"] {
            let mut forged = passed.clone();
            match kind {
                "self-review" => forged.reviewer_agent_id = Some(forged.implementer_agent_id.clone()),
                "head" => forged.bound_head = "other".into(),
                "implementer" => forged.implementer_agent_id = "other".into(),
                "task" => forged.task_id = "cas-other".into(),
                _ => forged.deadline_at += Duration::hours(1),
            }
            assert!(import_qa_pass(peer.path(), &forged, now + Duration::hours(2)).is_err(), "{kind}");
        }
        assert_eq!(list_qa_passes(peer.path(), "cas-ui1").unwrap(), vec![passed]);
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
        assert!(!second.to_string().contains("parse error:"), "{second}");

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
    fn retiring_reviewer_releases_claims_by_id_and_name_cas_8881() {
        use crate::{AgentStore, SqliteAgentStore};
        use cas_types::Agent;
        for unregister in [false, true] {
            for by_name in [false, true] {
                let dir = TempDir::new().unwrap();
                let agents = SqliteAgentStore::open(dir.path()).unwrap();
                agents.init().unwrap();
                let agent = Agent::new("reviewer-id".into(), "reviewer-name".into());
                agents.register(&agent).unwrap();
                let now = Utc::now();
                let opened =
                    dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
                set_qa_task(dir.path(), &opened.id, "cas-qa1").unwrap();
                let identity = if by_name { &agent.name } else { &agent.id };
                claim_qa_pass(dir.path(), "cas-ui1", identity, now).unwrap();
                // No active task lease: shutdown must release the pass itself.
                if unregister {
                    agents.unregister(&agent.id).unwrap();
                } else {
                    agents.graceful_shutdown(&agent.id).unwrap();
                }
                let pending = latest_qa_pass(dir.path(), "cas-ui1", now).unwrap().unwrap();
                assert_eq!(
                    pending.state,
                    QaPassState::Pending,
                    "unregister={unregister}, by_name={by_name}"
                );
                assert!(pending.reviewer_agent_id.is_none());
                assert_eq!(pending.id, opened.id);
                assert_eq!(pending.deadline_at, opened.deadline_at);
                assert!(
                    matches!(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap(),
                    QaPassOpen::AlreadyOpen(pass) if pass == pending)
                );
            }
        }
    }

    #[test]
    fn claim_reclaims_only_inactive_reviewers_cas_8881() {
        use crate::{AgentStore, SqliteAgentStore};
        use cas_types::{Agent, AgentStatus};
        for status in [
            Some(AgentStatus::Active),
            Some(AgentStatus::Idle),
            Some(AgentStatus::Stale),
            Some(AgentStatus::Shutdown),
            None,
        ] {
            for by_name in [false, true] {
                let dir = TempDir::new().unwrap();
                let agents = SqliteAgentStore::open(dir.path()).unwrap();
                agents.init().unwrap();
                let agent = Agent::new("reviewer-id".into(), "reviewer-name".into());
                agents.register(&agent).unwrap();
                let now = Utc::now();
                let opened =
                    dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
                let identity = if by_name { &agent.name } else { &agent.id };
                claim_qa_pass(dir.path(), "cas-ui1", identity, now).unwrap();
                // Simulate a legacy stale claim that escaped retirement cleanup.
                let conn = open_conn(dir.path()).unwrap();
                let conn = crate::shared_db::lock_connection(&conn).unwrap();
                if let Some(status) = status {
                    conn.execute(
                        "UPDATE agents SET status = ?1 WHERE id = ?2",
                        params![status.to_string(), agent.id],
                    )
                    .unwrap();
                } else {
                    conn.execute("DELETE FROM agents WHERE id = ?1", params![agent.id])
                        .unwrap();
                }
                drop(conn);
                let result = claim_qa_pass(dir.path(), "cas-ui1", "replacement", now);
                if matches!(status, Some(AgentStatus::Active | AgentStatus::Idle)) {
                    assert!(result.unwrap_err().to_string().contains("already claimed"));
                } else {
                    let reclaimed = result.expect("inactive claimant cannot block replacement");
                    assert_eq!(reclaimed.id, opened.id);
                    assert_eq!(reclaimed.deadline_at, opened.deadline_at);
                    assert_eq!(reclaimed.reviewer_agent_id.as_deref(), Some("replacement"));
                }
            }
        }
    }

    #[test]
    fn recovery_only_releases_expected_reviewer_cas_3172() {
        let dir = TempDir::new().unwrap();
        let now = Utc::now();
        let opened = dispatched(open_qa_pass(dir.path(), &new("aaaa1111", now), now).unwrap());
        set_qa_task(dir.path(), &opened.id, "cas-qa1").unwrap();
        let claimed = claim_qa_pass(dir.path(), "cas-ui1", "replacement", now).unwrap();
        assert!(release_qa_claim_for_reviewer(dir.path(), "cas-qa1", "dead-reviewer").is_err());
        assert_eq!(latest_qa_pass(dir.path(), "cas-ui1", now).unwrap().unwrap(), claimed);
        assert!(release_qa_claim_for_reviewer(dir.path(), "cas-qa1", "replacement").unwrap());
        let pending = latest_qa_pass(dir.path(), "cas-ui1", now).unwrap().unwrap();
        assert_eq!(pending.id, opened.id);
        assert_eq!(pending.deadline_at, opened.deadline_at);
        assert_eq!(pending.state, QaPassState::Pending);
        assert!(!release_qa_claim_for_reviewer(dir.path(), "cas-qa1", "replacement").unwrap());
        assert!(!release_qa_claim_for_reviewer(dir.path(), "ordinary-task", "replacement").unwrap());
        claim_qa_pass(dir.path(), "cas-ui1", "replacement", now).unwrap();
        let resolved = resolve_qa_pass(dir.path(), "cas-ui1", "replacement", QaVerdict::Approved,
            "passed", None, "/qa/LEDGER.md", now).unwrap();
        assert!(!release_qa_claim_for_reviewer(dir.path(), "cas-qa1", "replacement").unwrap());
        assert_eq!(latest_qa_pass(dir.path(), "cas-ui1", now).unwrap().unwrap(), resolved);
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
        assert_eq!(
            latest_failed_qa_reviewers(dir.path(), &["cas-ui1".into()])
                .unwrap()
                .get("cas-ui1"),
            Some(&("qa-worker".into(), 1)),
            "the caller needs the failed round to apply its review limit"
        );
        assert!(
            satisfying_qa_pass_for_head(dir.path(), "cas-ui1", "aaaa1111")
                .unwrap()
                .is_none()
        );

        let round2 = dispatched(open_qa_pass(dir.path(), &new("cccc3333", now), now).unwrap());
        assert_eq!(round2.round, 2);
        assert!(latest_failed_qa_reviewers(dir.path(), &["cas-ui1".into()]).unwrap().is_empty());
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
        assert!(latest_failed_qa_reviewers(dir.path(), &["cas-ui1".into()]).unwrap().is_empty());
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
