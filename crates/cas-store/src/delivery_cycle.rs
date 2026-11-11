//! cas-50b2: `DeliveryCycle`, the one recorded identity of a delivery.
//!
//! A cycle is minted the first time a task's `factory_branch_anchor` is
//! written and every later anchor write in that cycle appends its tip. Only
//! request-changes and a reopen end a cycle, so the next anchor write mints
//! the next one. A lease transfer or a re-park is not a new cycle. The task
//! store writes these rows inside the same transaction that persists the
//! anchor; nothing reads them yet.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::Result;

pub const DELIVERY_CYCLE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS delivery_cycles (
    cycle_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    task_id TEXT NOT NULL,
    cycle INTEGER NOT NULL,
    anchors TEXT NOT NULL,
    start_base TEXT,
    authorized_merge_sources TEXT NOT NULL,
    ended_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_delivery_cycles_task
    ON delivery_cycles(task_id, cycle DESC);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryCycle {
    pub cycle_id: String,
    pub project: String,
    pub task_id: String,
    pub cycle: u32,
    /// Append-only pushed tips parked in this cycle; the last is current.
    pub anchors: Vec<String>,
    pub start_base: Option<String>,
    pub ended: bool,
}

pub fn delivery_cycle_id(project: &str, task_id: &str, cycle: u32) -> String {
    let mut hasher = Sha256::new();
    for part in [project, task_id, &cycle.to_string()] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("dc-{}", &format!("{:x}", hasher.finalize())[..24])
}

fn latest(conn: &Connection, task_id: &str) -> Result<Option<(String, u32, Vec<String>, bool)>> {
    conn.execute_batch(DELIVERY_CYCLE_SCHEMA)?;
    let row: Option<(String, u32, String, Option<String>)> = conn
        .query_row(
            "SELECT cycle_id, cycle, anchors, ended_at FROM delivery_cycles
             WHERE task_id = ?1 ORDER BY cycle DESC LIMIT 1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(id, cycle, anchors, ended)| {
        Ok((id, cycle, serde_json::from_str(&anchors)?, ended.is_some()))
    })
    .transpose()
}

/// The cycle the task's next anchor write belongs to.
pub fn current_delivery_cycle_with_conn(conn: &Connection, task_id: &str) -> Result<u32> {
    Ok(match latest(conn, task_id)? {
        Some((_, cycle, _, false)) => cycle,
        Some((_, cycle, _, true)) => cycle + 1,
        None => 1,
    })
}

pub fn current_delivery_cycle(root: &std::path::Path, task_id: &str) -> Result<u32> {
    let conn = Connection::open(root.join("cas.db"))?;
    crate::shared_db::install_busy_handler(&conn)?;
    current_delivery_cycle_with_conn(&conn, task_id)
}

/// Mint the current cycle with `anchor`, or append `anchor` to it. Writing
/// the cycle's current anchor again is a no-op.
pub fn mint_or_append_with_conn(
    conn: &Connection,
    project: &str,
    task_id: &str,
    anchor: &str,
    start_base: Option<&str>,
    authorized_merge_sources: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    match latest(conn, task_id)? {
        Some((cycle_id, _, mut anchors, false)) => {
            if anchors.last().map(String::as_str) != Some(anchor) {
                anchors.push(anchor.to_string());
                conn.execute(
                    "UPDATE delivery_cycles SET anchors = ?2, updated_at = ?3 WHERE cycle_id = ?1",
                    params![cycle_id, serde_json::to_string(&anchors)?, now.to_rfc3339()],
                )?;
            }
        }
        previous => {
            let cycle = previous.map_or(1, |(_, cycle, _, _)| cycle + 1);
            conn.execute(
                "INSERT INTO delivery_cycles (cycle_id, project, task_id, cycle, anchors,
                     start_base, authorized_merge_sources, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    delivery_cycle_id(project, task_id, cycle),
                    project,
                    task_id,
                    cycle,
                    serde_json::to_string(&[anchor])?,
                    start_base,
                    authorized_merge_sources,
                    now.to_rfc3339(),
                ],
            )?;
        }
    }
    Ok(())
}

/// Request-changes and reopen end the open cycle; the next anchor write
/// mints the next one. Ending an already-ended cycle is a no-op.
pub fn end_delivery_cycle_with_conn(conn: &Connection, task_id: &str, now: DateTime<Utc>) -> Result<()> {
    conn.execute_batch(DELIVERY_CYCLE_SCHEMA)?;
    conn.execute(
        "UPDATE delivery_cycles SET ended_at = ?2, updated_at = ?2
         WHERE task_id = ?1 AND ended_at IS NULL",
        params![task_id, now.to_rfc3339()],
    )?;
    Ok(())
}

pub fn delivery_cycles(root: &std::path::Path, task_id: &str) -> Result<Vec<DeliveryCycle>> {
    let conn = Connection::open(root.join("cas.db"))?;
    crate::shared_db::install_busy_handler(&conn)?;
    conn.execute_batch(DELIVERY_CYCLE_SCHEMA)?;
    let mut statement = conn.prepare(
        "SELECT cycle_id, project, task_id, cycle, anchors, start_base, ended_at
         FROM delivery_cycles WHERE task_id = ?1 ORDER BY cycle",
    )?;
    let rows = statement
        .query_map(params![task_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(cycle_id, project, task_id, cycle, anchors, start_base, ended)| {
            Ok(DeliveryCycle {
                cycle_id,
                project,
                task_id,
                cycle,
                anchors: serde_json::from_str(&anchors)?,
                start_base,
                ended: ended.is_some(),
            })
        })
        .collect()
}
