//! Release-gate resource ledger: one row per metered use of a rate or quota
//! resource, keyed by (env, resource). Every take runs in one `BEGIN
//! IMMEDIATE` transaction, so processes sharing `cas.db` share one budget.
use crate::Result;
use rusqlite::{OptionalExtension, params};
use std::path::Path;

pub const QA_RESOURCE_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS qa_resource_use (
        id INTEGER PRIMARY KEY AUTOINCREMENT, env TEXT NOT NULL, resource TEXT NOT NULL,
        amount INTEGER NOT NULL, at_ms INTEGER NOT NULL, held_until_ms INTEGER)",
    "CREATE INDEX IF NOT EXISTS idx_qa_resource_use_bucket
        ON qa_resource_use(env, resource, at_ms)",
];

/// The longest window a ledger row can count toward (a quota's day).
const MAX_WINDOW_MS: i64 = 86_400_000;

/// One attempt to take `amount` from the (env, resource) bucket at `now_ms`.
#[derive(Debug, Clone)]
pub struct ResourceTake<'a> {
    pub env: &'a str,
    pub resource: &'a str,
    pub amount: u32,
    pub now_ms: i64,
    /// At most `limit` units may be taken within any `window_ms`.
    pub window_ms: i64,
    pub limit: u32,
    /// At most this many takes may be held at once.
    pub concurrency: Option<u32>,
    /// When an unreleased hold stops counting (a crashed holder's lease).
    pub hold_until_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceGrant {
    /// The ledger row id; release it when the request ends.
    Granted(i64),
    /// No grant before `earliest` (the window); a held concurrency slot
    /// frees by `latest` at the latest, or sooner if its holder releases.
    RetryAt { earliest: i64, latest: i64 },
    /// `amount` exceeds `limit`, so no wait can satisfy it.
    Never,
}

fn connection(root: &Path) -> Result<std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>> {
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    {
        let guard = crate::shared_db::lock_connection(&conn)?;
        for sql in QA_RESOURCE_SCHEMA_STATEMENTS {
            guard.execute(sql, [])?;
        }
    }
    Ok(conn)
}

/// Takes `amount` atomically across processes, or says when to retry.
pub fn take_resource(root: &Path, take: &ResourceTake<'_>) -> Result<ResourceGrant> {
    if take.amount > take.limit {
        return Ok(ResourceGrant::Never);
    }
    let conn = connection(root)?;
    crate::shared_db::with_immediate_write_txn_pooled(&conn, |tx| {
        let now = take.now_ms;
        tx.execute(
            "DELETE FROM qa_resource_use WHERE at_ms <= ?1
             AND (held_until_ms IS NULL OR held_until_ms <= ?2)",
            params![now - MAX_WINDOW_MS, now],
        )?;
        let mut window_at = None;
        let since = now - take.window_ms;
        let used: Vec<(i64, i64)> = tx
            .prepare(
                "SELECT at_ms, amount FROM qa_resource_use
                 WHERE env=?1 AND resource=?2 AND at_ms > ?3 ORDER BY at_ms",
            )?
            .query_map(params![take.env, take.resource, since], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut excess = used.iter().map(|(_, a)| a).sum::<i64>() + i64::from(take.amount)
            - i64::from(take.limit);
        for (at_ms, amount) in &used {
            if excess <= 0 {
                break;
            }
            excess -= amount;
            window_at = Some(at_ms + take.window_ms);
        }
        let mut held_until = None;
        if let Some(concurrency) = take.concurrency {
            let (held, first_free): (u32, Option<i64>) = tx.query_row(
                "SELECT COUNT(*), MIN(held_until_ms) FROM qa_resource_use
                 WHERE env=?1 AND resource=?2 AND held_until_ms > ?3",
                params![take.env, take.resource, now],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if held >= concurrency {
                held_until = first_free;
            }
        }
        if window_at.is_some() || held_until.is_some() {
            let earliest = window_at.unwrap_or(now);
            let latest = earliest.max(held_until.unwrap_or(earliest));
            return Ok(ResourceGrant::RetryAt { earliest, latest });
        }
        tx.execute(
            "INSERT INTO qa_resource_use (env, resource, amount, at_ms, held_until_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                take.env,
                take.resource,
                take.amount,
                now,
                take.concurrency.map(|_| take.hold_until_ms)
            ],
        )?;
        // AUTOINCREMENT never reuses an id, so a late release cannot clear a
        // later take's hold.
        Ok(ResourceGrant::Granted(tx.last_insert_rowid()))
    })
}

/// Ends a take's hold. With `refund`, the request never ran and its use is
/// removed from the window too.
pub fn release_resource(root: &Path, id: i64, refund: bool) -> Result<()> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let sql = if refund {
        "DELETE FROM qa_resource_use WHERE id=?1"
    } else {
        "UPDATE qa_resource_use SET held_until_ms=NULL WHERE id=?1"
    };
    guard.execute(sql, [id])?;
    Ok(())
}

/// Units of (env, resource) taken after `since_ms`.
pub fn resource_used(root: &Path, env: &str, resource: &str, since_ms: i64) -> Result<u64> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    let used: Option<i64> = guard
        .query_row(
            "SELECT SUM(amount) FROM qa_resource_use WHERE env=?1 AND resource=?2 AND at_ms > ?3",
            params![env, resource, since_ms],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(used.unwrap_or(0).max(0) as u64)
}
