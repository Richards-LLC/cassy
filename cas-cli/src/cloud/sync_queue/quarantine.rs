//! Local quarantine ledger for rows whose home project cannot be established.
//!
//! # Why a ledger and not a status change (GH #701)
//!
//! The `cas-ed15` pull leak left hundreds of rows in each project database that
//! belong to *some* project but carry no attribution and no local-activity
//! evidence anywhere on the host. They surface in `task ready` as this
//! project's outstanding work. They cannot be deleted — the peer databases are
//! not evidence about them — and they must not be closed in the cloud, because
//! closing a row whose owner is unknown is a decision made on somebody else's
//! data.
//!
//! So the remediation is local and reversible: a marker in this ledger hides
//! the row from the board and stops it from being pushed. The row itself is
//! untouched, which is what makes the two hard requirements fall out for free:
//!
//! - **Idempotent across pulls.** The ledger is separate local state that the
//!   pull never writes. A re-pulled row can rewrite the task row's content as
//!   often as it likes; the marker still hides it, so the quarantine does not
//!   have to be re-applied and the counts stay flat.
//! - **Reversible.** Releasing deletes one marker row and the task returns
//!   exactly as it was.
//!
//! Nothing here is ever synced: the table is local, and quarantining also drops
//! any already-queued push for the row so the decision cannot leak outward.

use std::collections::BTreeSet;

use chrono::Utc;
use rusqlite::params;

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

/// DDL for the quarantine ledger. Shared by the queue schema (fresh databases)
/// and migration 250 (existing ones).
pub const QUARANTINED_ROW_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS quarantined_rows (
        entity_type TEXT NOT NULL,
        entity_id TEXT NOT NULL,
        reason TEXT NOT NULL,
        quarantined_at TEXT NOT NULL,
        PRIMARY KEY (entity_type, entity_id)
    )",
    "CREATE INDEX IF NOT EXISTS idx_quarantined_rows_entity_type
        ON quarantined_rows(entity_type)",
];

/// Entity type stored for quarantined task rows.
pub const QUARANTINE_TASK: &str = "task";

/// One quarantined row, for reporting and for release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinedRow {
    pub entity_type: String,
    pub entity_id: String,
    pub reason: String,
    pub quarantined_at: String,
}

impl SyncQueue {
    /// Quarantine one row. Idempotent: re-quarantining an already-marked row
    /// keeps the original reason and timestamp and reports `false`, so a
    /// repeated `doctor --fix-cloud-rows` run is a no-op rather than a
    /// re-stamp that would make the audit trail lie about when the decision
    /// was taken.
    ///
    /// Also drops any pending push for the row: the quarantine decision is
    /// local, and a queued upsert would carry the row outward regardless.
    pub fn quarantine_row(
        &self,
        entity_type: &str,
        entity_id: &str,
        reason: &str,
    ) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO quarantined_rows
                 (entity_type, entity_id, reason, quarantined_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                entity_type,
                entity_id,
                reason,
                Utc::now().to_rfc3339()
            ],
        )?;
        conn.execute(
            "DELETE FROM sync_queue WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )?;
        Ok(inserted > 0)
    }

    /// Release one row from quarantine. Reports whether a marker was removed,
    /// so a release run can honestly say "0 rows released" instead of implying
    /// it undid something.
    pub fn release_quarantined_row(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM quarantined_rows WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )?;
        Ok(removed > 0)
    }

    /// Ids quarantined for one entity type. Read on every board query, so it
    /// returns a set rather than a Vec.
    pub fn quarantined_ids(&self, entity_type: &str) -> Result<BTreeSet<String>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT entity_id FROM quarantined_rows WHERE entity_type = ?1")?;
        let ids = stmt
            .query_map(params![entity_type], |row| row.get::<_, String>(0))?
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(ids)
    }

    /// Every quarantined row of one entity type, for `doctor` reporting.
    pub fn quarantined_rows(&self, entity_type: &str) -> Result<Vec<QuarantinedRow>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT entity_type, entity_id, reason, quarantined_at
             FROM quarantined_rows WHERE entity_type = ?1 ORDER BY entity_id",
        )?;
        let rows = stmt
            .query_map(params![entity_type], |row| {
                Ok(QuarantinedRow {
                    entity_type: row.get(0)?,
                    entity_id: row.get(1)?,
                    reason: row.get(2)?,
                    quarantined_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Drop every queued push for one quarantined row, reporting how many rows
    /// were removed.
    ///
    /// Separate from [`SyncQueue::quarantine_row`] because the two run at
    /// different times: quarantining clears what is queued *now*, and the pull
    /// re-asserts the same invariant afterwards against anything a write path
    /// enqueued in between.
    pub fn drop_queued_pushes_for(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM sync_queue WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
        )?;
        Ok(removed)
    }

    /// How many rows of one entity type are quarantined.
    pub fn quarantined_count(&self, entity_type: &str) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM quarantined_rows WHERE entity_type = ?1",
            params![entity_type],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}
