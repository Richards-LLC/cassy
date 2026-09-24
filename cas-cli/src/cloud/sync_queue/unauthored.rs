//! Local ledger of pulled rows this project did not author (cas-3a90, GH #909).
//!
//! # Why
//!
//! Entries, rules and skills never carried an origin, so a pull admitted any row
//! that came back in this project's scope. The cloud stamps that scope from the
//! request, so a teammate's row from another project arrived looking native.
//! The store then treated it as local. The next local write to it (a decay pass,
//! an access bump, a `helpful` mark) enqueued it, and the push sent it back
//! under *this* project. 2,391 memory ids were echoed into up to 12 project
//! buckets this way.
//!
//! Pushes now stamp `origin_project` on these rows, so new rows arrive with
//! their owner and a foreign one is refused at pull. A legacy row without an
//! origin cannot be attributed. When a pull *creates* such a row locally, the
//! row is recorded here, and every push drops queued writes for it. The row
//! stays readable locally, but it is never re-published under this project.
//!
//! The ledger is local, never synced, and only ever written by the pull. A
//! later pull of the same row carrying this project's origin clears the
//! marker.

use std::collections::BTreeMap;

use chrono::Utc;
use rusqlite::params;

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

/// DDL for the unauthored-pull ledger. Shared by the queue schema (fresh
/// databases) and migration 258 (existing ones).
pub const UNAUTHORED_PULL_STATEMENTS: &[&str] =
    &["CREATE TABLE IF NOT EXISTS unauthored_pulled_rows (
        entity_type TEXT NOT NULL,
        entity_id TEXT NOT NULL,
        reason TEXT NOT NULL,
        recorded_at TEXT NOT NULL,
        PRIMARY KEY (entity_type, entity_id)
    )"];

impl SyncQueue {
    /// Record that a pull created a row this project cannot prove it authored.
    ///
    /// Idempotent: the first reason and timestamp are kept. Any already-queued
    /// write for the row is dropped at the same time, so nothing enqueued
    /// before the pull can carry it outward either.
    pub fn record_unauthored_pull(
        &self,
        entity_type: &str,
        entity_id: &str,
        reason: &str,
    ) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO unauthored_pulled_rows
                 (entity_type, entity_id, reason, recorded_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![entity_type, entity_id, reason, Utc::now().to_rfc3339()],
        )?;
        conn.execute(
            "DELETE FROM sync_queue WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )?;
        Ok(inserted > 0)
    }

    /// Clear the marker when the row is later pulled with this project's own
    /// `origin_project`: its owner is now established as this project.
    pub fn forget_unauthored_pull(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM unauthored_pulled_rows WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )?;
        Ok(removed > 0)
    }

    /// Whether one row is recorded as pulled but not authored here.
    pub fn is_unauthored_pull(&self, entity_type: &str, entity_id: &str) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM unauthored_pulled_rows
                            WHERE entity_type = ?1 AND entity_id = ?2)",
            params![entity_type, entity_id],
            |row| row.get(0),
        )?)
    }

    /// Recorded rows per entity type, for `cas doctor`.
    pub fn unauthored_pull_counts(&self) -> Result<BTreeMap<String, usize>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT entity_type, COUNT(*) FROM unauthored_pulled_rows GROUP BY entity_type",
        )?;
        let counts = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(counts)
    }

    /// Drop every queued write for a recorded row, returning how many queue
    /// rows were removed. Every personal and team push calls this first, so a
    /// local write made after the pull cannot publish the row either.
    pub fn drop_queued_pushes_for_unauthored_pulls(&self) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM sync_queue WHERE EXISTS (
                 SELECT 1 FROM unauthored_pulled_rows u
                 WHERE u.entity_type = sync_queue.entity_type
                   AND u.entity_id = sync_queue.entity_id
             )",
            [],
        )?;
        Ok(removed)
    }
}
