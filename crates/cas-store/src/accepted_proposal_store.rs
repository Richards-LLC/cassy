//! Local record of cross-project proposals accepted into this project
//! (cas-3556).
//!
//! An accepted task keeps the proposer in `origin_project`, yet it belongs to
//! the project that accepted it. Ownership checks read that fact from here,
//! never from the task's free-text notes: only the proposal accept path and
//! the sync pull of a server-attested accepted proposal write this table, and
//! it is never synced.
use crate::Result;
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use std::path::Path;

pub const ACCEPTED_PROPOSAL_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS accepted_proposal_targets (
        task_id TEXT PRIMARY KEY, proposal_id TEXT NOT NULL,
        target_project_canonical_id TEXT NOT NULL, recorded_at TEXT NOT NULL)",
];

fn connection(root: &Path) -> Result<std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>> {
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    {
        let guard = crate::shared_db::lock_connection(&conn)?;
        for sql in ACCEPTED_PROPOSAL_SCHEMA_STATEMENTS {
            guard.execute(sql, [])?;
        }
    }
    Ok(conn)
}

/// Record that `task_id` was accepted into `target_project` by `proposal_id`.
pub fn record_accepted_proposal_target(
    root: &Path,
    task_id: &str,
    proposal_id: &str,
    target_project: &str,
) -> Result<()> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    guard.execute(
        "INSERT INTO accepted_proposal_targets
        (task_id, proposal_id, target_project_canonical_id, recorded_at) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(task_id) DO UPDATE SET proposal_id = excluded.proposal_id,
            target_project_canonical_id = excluded.target_project_canonical_id,
            recorded_at = excluded.recorded_at",
        params![task_id, proposal_id, target_project, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// The project `task_id` was accepted into, if it is an accepted proposal.
pub fn accepted_proposal_target(root: &Path, task_id: &str) -> Result<Option<String>> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    Ok(guard
        .query_row(
            "SELECT target_project_canonical_id FROM accepted_proposal_targets WHERE task_id = ?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Every accepted proposal: `(task_id, target project)` pairs, read once so a
/// board view need not query per task (cas-11ba).
pub fn accepted_proposal_targets(root: &Path) -> Result<Vec<(String, String)>> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let mut stmt =
        guard.prepare("SELECT task_id, target_project_canonical_id FROM accepted_proposal_targets")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
