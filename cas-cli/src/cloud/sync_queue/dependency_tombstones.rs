//! Durable ledger of dependency-edge deletions received from the cloud.
//!
//! The edge row is intentionally gone from `dependencies` after a delete, so
//! this is a separate ledger rather than a column on that table. It exists so a
//! later push from *this* machine cannot resurrect an edge another machine
//! removed: an incremental pull delivers the cloud tombstone exactly once, and
//! without a local record the next reconciliation would see a local-only edge
//! and re-push it.
//!
//! Ordering is settled by timestamp, not by arrival: a tombstone only suppresses
//! a local edge whose `created_at` is at or before `deleted_at`. An edge
//! recreated locally *after* the delete is newer state and must win.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

/// DDL for the tombstone ledger. Shared by the queue schema (fresh databases)
/// and migration 249 (existing ones).
pub const TASK_DEPENDENCY_TOMBSTONE_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS task_dependency_tombstones (
        id TEXT PRIMARY KEY,
        from_id TEXT NOT NULL,
        to_id TEXT NOT NULL,
        dep_type TEXT NOT NULL,
        deleted_at TEXT NOT NULL,
        recorded_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_task_dependency_tombstones_deleted_at
        ON task_dependency_tombstones(deleted_at)",
];

/// The cloud prunes dependency tombstones after 90 days
/// (`TASK_DEPENDENCY_TOMBSTONE_RETENTION_DAYS` in petra-stella-cloud). Keeping
/// the local ledger on the same horizon means it never suppresses an edge the
/// cloud has already forgotten about.
pub const TASK_DEPENDENCY_TOMBSTONE_RETENTION_DAYS: i64 = 90;

impl SyncQueue {
    /// Record (or advance) a tombstone for one dependency edge.
    ///
    /// Keeps the newest `deleted_at`: two clients can report the same delete,
    /// and a later re-delete must not be rolled back by a replayed older row.
    pub fn record_dependency_tombstone(
        &self,
        entity_id: &str,
        from_id: &str,
        to_id: &str,
        dep_type: &str,
        deleted_at: DateTime<Utc>,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO task_dependency_tombstones
                (id, from_id, to_id, dep_type, deleted_at, recorded_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                deleted_at = MAX(task_dependency_tombstones.deleted_at, excluded.deleted_at),
                recorded_at = excluded.recorded_at
            "#,
            params![
                entity_id,
                from_id,
                to_id,
                dep_type,
                deleted_at.to_rfc3339(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Read one edge's tombstone timestamp, if any.
    pub fn dependency_tombstone(
        &self,
        entity_id: &str,
    ) -> Result<Option<DateTime<Utc>>, CasError> {
        let conn = self.conn.lock().unwrap();
        let raw: Option<String> = conn
            .query_row(
                "SELECT deleted_at FROM task_dependency_tombstones WHERE id = ?1",
                params![entity_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.as_deref().and_then(parse_timestamp))
    }

    /// Read the whole ledger keyed by `{from_id}:{to_id}:{dep_type}`.
    pub fn dependency_tombstones(&self) -> Result<BTreeMap<String, DateTime<Utc>>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut statement =
            conn.prepare("SELECT id, deleted_at FROM task_dependency_tombstones")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut tombstones = BTreeMap::new();
        for row in rows {
            let (id, deleted_at) = row?;
            if let Some(deleted_at) = parse_timestamp(&deleted_at) {
                tombstones.insert(id, deleted_at);
            }
        }
        Ok(tombstones)
    }

    /// Forget one tombstone. Called when the edge is legitimately recreated
    /// after the delete, so the ledger cannot suppress the newer edge forever.
    pub fn clear_dependency_tombstone(&self, entity_id: &str) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM task_dependency_tombstones WHERE id = ?1",
            params![entity_id],
        )?;
        Ok(())
    }

    /// Drop tombstones older than the cloud's retention horizon.
    pub fn prune_dependency_tombstones(&self, now: DateTime<Utc>) -> Result<usize, CasError> {
        let cutoff = now - chrono::Duration::days(TASK_DEPENDENCY_TOMBSTONE_RETENTION_DAYS);
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM task_dependency_tombstones WHERE deleted_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(removed)
    }

    /// Drop any queued upsert for an edge the cloud has tombstoned.
    ///
    /// The server refuses a stale resurrection anyway (last-write-wins on
    /// `updated_at`), but leaving the row queued means every push retries a
    /// request that can never succeed. A queued *delete* is left alone: it
    /// agrees with the tombstone.
    pub fn drop_queued_dependency_upsert(&self, entity_id: &str) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM sync_queue
             WHERE entity_type = 'task_dependency' AND entity_id = ?1 AND operation = 'upsert'",
            params![entity_id],
        )?;
        Ok(removed)
    }
}

fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}
