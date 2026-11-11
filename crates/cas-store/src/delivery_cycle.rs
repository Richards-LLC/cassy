//! cas-50b2: `DeliveryCycle`, the one recorded identity of a delivery.
//!
//! A cycle is minted the first time a task's `factory_branch_anchor` is
//! written and every later anchor write in that cycle appends its tip. Only
//! request-changes and a reopen start the next cycle; they advance
//! `TaskDeliverables::delivery_cycle`, which syncs with the task row, so every
//! machine derives the same cycle number and therefore the same `cycle_id`.
//! A lease transfer or a re-park is not a new cycle. The task store writes
//! these rows inside the transaction that persists the anchor; nothing reads
//! them yet.

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

/// Deterministic across machines: the cycle number comes from the synced
/// task row, never from local row counts.
pub fn delivery_cycle_id(project: &str, task_id: &str, cycle: u32) -> String {
    let mut hasher = Sha256::new();
    for part in [project, task_id, &cycle.to_string()] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("dc-{}", &format!("{:x}", hasher.finalize())[..24])
}

struct Latest {
    cycle_id: String,
    cycle: u32,
    anchors: Vec<String>,
    ended: bool,
}

fn latest(conn: &Connection, task_id: &str) -> Result<Option<Latest>> {
    let row: Option<(String, u32, String, Option<String>)> = conn
        .query_row(
            "SELECT cycle_id, cycle, anchors, ended_at FROM delivery_cycles
             WHERE task_id = ?1 ORDER BY cycle DESC LIMIT 1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(cycle_id, cycle, anchors, ended)| {
        Ok(Latest { cycle_id, cycle, anchors: serde_json::from_str(&anchors)?, ended: ended.is_some() })
    })
    .transpose()
}

fn effective(counter: u32, latest: Option<&Latest>) -> u32 {
    let recorded = latest.map_or(1, |row| row.cycle + u32::from(row.ended));
    counter.max(1).max(recorded)
}

/// The cycle the task's next anchor write belongs to. `counter` is the
/// task row's `delivery_cycle`.
pub fn current_delivery_cycle_with_conn(conn: &Connection, task_id: &str, counter: u32) -> Result<u32> {
    Ok(effective(counter, latest(conn, task_id)?.as_ref()))
}

pub fn current_delivery_cycle(root: &std::path::Path, task_id: &str, counter: u32) -> Result<u32> {
    let conn = Connection::open(root.join("cas.db"))?;
    crate::shared_db::install_busy_handler(&conn)?;
    current_delivery_cycle_with_conn(&conn, task_id, counter)
}

pub(crate) struct AnchorWrite<'a> {
    pub project: &'a str,
    pub task_id: &'a str,
    pub anchor: &'a str,
    pub counter: u32,
    pub start_base: Option<&'a str>,
    /// A sync apply replays another machine's writes; it never re-appends a
    /// tip this cycle already holds, so a stale echo cannot reorder anchors.
    pub replicated: bool,
}

/// Mint the current cycle with the anchor, or append the anchor to it, and
/// return the cycle id. Writing the cycle's current anchor again is a no-op.
pub(crate) fn mint_or_append_with_conn(conn: &Connection, write: &AnchorWrite<'_>, now: DateTime<Utc>) -> Result<String> {
    let latest = latest(conn, write.task_id)?;
    let cycle = effective(write.counter, latest.as_ref());
    let now = now.to_rfc3339();
    conn.execute(
        "UPDATE delivery_cycles SET ended_at = ?3, updated_at = ?3
         WHERE task_id = ?1 AND cycle < ?2 AND ended_at IS NULL",
        params![write.task_id, cycle, now],
    )?;
    if let Some(mut row) = latest.filter(|row| row.cycle == cycle) {
        let held = if write.replicated {
            row.anchors.iter().any(|anchor| anchor == write.anchor)
        } else {
            row.anchors.last().map(String::as_str) == Some(write.anchor)
        };
        if !held {
            row.anchors.push(write.anchor.to_string());
            conn.execute(
                "UPDATE delivery_cycles SET anchors = ?2, updated_at = ?3 WHERE cycle_id = ?1",
                params![row.cycle_id, serde_json::to_string(&row.anchors)?, now],
            )?;
        }
        return Ok(row.cycle_id);
    }
    let cycle_id = delivery_cycle_id(write.project, write.task_id, cycle);
    insert(conn, &cycle_id, write, cycle, &[write.anchor.to_string()], None, &now)?;
    Ok(cycle_id)
}

fn insert(
    conn: &Connection,
    cycle_id: &str,
    write: &AnchorWrite<'_>,
    cycle: u32,
    anchors: &[String],
    ended_at: Option<&str>,
    now: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO delivery_cycles (cycle_id, project, task_id, cycle, anchors,
             start_base, ended_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            cycle_id,
            write.project,
            write.task_id,
            cycle,
            serde_json::to_string(anchors)?,
            write.start_base,
            ended_at,
            now,
        ],
    )?;
    Ok(())
}

/// Request-changes and reopen end the current cycle. Returns the task row's
/// next `delivery_cycle`; the next anchor write mints that cycle.
pub(crate) fn end_delivery_cycle_with_conn(
    conn: &Connection,
    task_id: &str,
    counter: u32,
    now: DateTime<Utc>,
) -> Result<u32> {
    let current = current_delivery_cycle_with_conn(conn, task_id, counter)?;
    conn.execute(
        "UPDATE delivery_cycles SET ended_at = ?3, updated_at = ?3
         WHERE task_id = ?1 AND cycle <= ?2 AND ended_at IS NULL",
        params![task_id, current, now.to_rfc3339()],
    )?;
    Ok(current + 1)
}

/// One-shot backfill for open tasks anchored before cycles existed. Retired
/// anchors fill earlier cycles in order, each declined anchor (request-changes)
/// closing one; the current anchor lands in the open newest cycle, numbered
/// by the row's replicated `delivery_cycle` when it is set.
pub(crate) fn backfill_with_conn(conn: &Connection, default_project: Option<&str>) -> Result<()> {
    let candidates = "FROM tasks t
         WHERE t.status NOT IN ('closed', 'cancelled')
           AND NOT EXISTS (SELECT 1 FROM delivery_cycles c WHERE c.task_id = t.id)
           AND json_valid(t.deliverables) AND json_type(t.deliverables) = 'object'
           AND (json_extract(t.deliverables, '$.factory_branch_anchor') IS NOT NULL
                OR json_array_length(t.deliverables, '$.historical_factory_branch_anchors') > 0)";
    let pending: bool = conn.query_row(&format!("SELECT EXISTS (SELECT 1 {candidates})"), [], |row| row.get(0))?;
    if !pending {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    let rows: Vec<(String, Option<String>, String)> = tx
        .prepare(&format!("SELECT t.id, t.origin_project, t.deliverables {candidates}"))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let now = Utc::now().to_rfc3339();
    for (task_id, project, deliverables) in rows {
        let Ok(deliverables) = serde_json::from_str::<cas_types::TaskDeliverables>(&deliverables) else {
            continue;
        };
        let project = project.as_deref().or(default_project).unwrap_or_default();
        let declined = deliverables
            .declined_factory_branch_anchors
            .as_ref()
            .unwrap_or(&deliverables.historical_factory_branch_anchors);
        let mut cycles: Vec<(Vec<String>, bool)> = vec![(Vec::new(), false)];
        for anchor in &deliverables.historical_factory_branch_anchors {
            let open = cycles.last_mut().expect("one cycle");
            open.0.push(anchor.clone());
            if declined.iter().any(|known| known.eq_ignore_ascii_case(anchor)) {
                open.1 = true;
                cycles.push((Vec::new(), false));
            }
        }
        if let Some(anchor) = deliverables.factory_branch_anchor.as_deref().filter(|tip| !tip.trim().is_empty()) {
            cycles.last_mut().expect("one cycle").0.push(anchor.to_string());
        }
        let write = AnchorWrite {
            project,
            task_id: &task_id,
            anchor: "",
            counter: 0,
            start_base: deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str()),
            replicated: false,
        };
        // A replicated nonzero counter names the open cycle; earlier cycles
        // count back from it, and any surplus history folds into cycle 1.
        let open = match deliverables.delivery_cycle {
            0 => cycles.len() as u32,
            counter => counter,
        };
        let total = cycles.len();
        let mut numbered: Vec<(u32, Vec<String>, bool)> = Vec::new();
        for (index, (anchors, ended)) in cycles.into_iter().enumerate() {
            let cycle = (open as usize).saturating_sub(total - 1 - index).max(1) as u32;
            match numbered.last_mut().filter(|(number, ..)| *number == cycle) {
                Some(merged) => {
                    merged.1.extend(anchors);
                    merged.2 = ended;
                }
                None => numbered.push((cycle, anchors, ended)),
            }
        }
        for (cycle, anchors, ended) in numbered.iter().filter(|(_, anchors, _)| !anchors.is_empty()) {
            let cycle_id = delivery_cycle_id(project, &task_id, *cycle);
            insert(&tx, &cycle_id, &write, *cycle, anchors, ended.then_some(now.as_str()), &now)?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delivery_cycles(root: &std::path::Path, task_id: &str) -> Result<Vec<DeliveryCycle>> {
    let conn = Connection::open(root.join("cas.db"))?;
    crate::shared_db::install_busy_handler(&conn)?;
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
