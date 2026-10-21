//! Crash-persistent release fixture cleanup. Failed teardown keeps the row.
use crate::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const QA_FIXTURE_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS qa_fixture_cleanup (
        id TEXT PRIMARY KEY, handle TEXT NOT NULL, env TEXT NOT NULL,
        sha TEXT NOT NULL, recipe TEXT NOT NULL, expires_at INTEGER NOT NULL,
        orphan_error TEXT)",
    "CREATE INDEX IF NOT EXISTS idx_qa_fixture_cleanup_expiry
        ON qa_fixture_cleanup(expires_at)",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureCleanup {
    pub id: String,
    pub handle: String,
    pub env: String,
    pub sha: String,
    pub recipe: String,
    pub expires_at: i64,
    pub orphan_error: Option<String>,
}

fn connection(root: &Path) -> Result<std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>> {
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    {
        let guard = crate::shared_db::lock_connection(&conn)?;
        for sql in QA_FIXTURE_SCHEMA_STATEMENTS {
            guard.execute(sql, [])?;
        }
    }
    Ok(conn)
}

pub fn record_fixture_cleanup(root: &Path, row: &FixtureCleanup) -> Result<()> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    guard.execute(
        "INSERT INTO qa_fixture_cleanup (id, handle, env, sha, recipe, expires_at, orphan_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.id,
            row.handle,
            row.env,
            row.sha,
            row.recipe,
            row.expires_at,
            row.orphan_error
        ],
    )?;
    Ok(())
}

/// `None` lists all outstanding rows; a timestamp selects expired rows only.
pub fn fixture_cleanups(root: &Path, expired_at: Option<i64>) -> Result<Vec<FixtureCleanup>> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let mut stmt = guard.prepare(
        "SELECT id, handle, env, sha, recipe, expires_at, orphan_error FROM qa_fixture_cleanup
         WHERE (?1 IS NULL OR expires_at <= ?1) ORDER BY expires_at, id",
    )?;
    let rows = stmt
        .query_map([expired_at], |row| {
            Ok(FixtureCleanup {
                id: row.get(0)?,
                handle: row.get(1)?,
                env: row.get(2)?,
                sha: row.get(3)?,
                recipe: row.get(4)?,
                expires_at: row.get(5)?,
                orphan_error: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// A successful teardown deletes; an error becomes durable orphan debt.
pub fn settle_fixture_cleanup(root: &Path, id: &str, error: Option<&str>) -> Result<()> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    if let Some(error) = error {
        guard.execute(
            "UPDATE qa_fixture_cleanup SET orphan_error=?2 WHERE id=?1",
            params![id, error],
        )?;
    } else {
        guard.execute("DELETE FROM qa_fixture_cleanup WHERE id=?1", [id])?;
    }
    Ok(())
}
