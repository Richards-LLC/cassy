//! Persistent sync queue for cloud synchronization
//!
//! Queues local changes for eventual sync to cloud. Provides offline resilience
//! by persisting the queue to SQLite.
//!
//! # Integration Status
//! Queue infrastructure ready for cloud sync feature.

use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::Mutex;

use crate::error::CasError;

mod dependency_tombstones;
mod maintenance;
mod metadata;
mod quarantine;
mod queue_ops;
mod revisions;
mod schema;
mod stats;
#[cfg(test)]
mod tests;
mod types;

pub use dependency_tombstones::{
    TASK_DEPENDENCY_TOMBSTONE_RETENTION_DAYS, TASK_DEPENDENCY_TOMBSTONE_STATEMENTS,
};
pub use quarantine::{QUARANTINE_TASK, QUARANTINED_ROW_STATEMENTS, QuarantinedRow};
pub use revisions::{SYNC_REVISION_STATEMENTS, parse_wire_revision, wire_revision};
pub use types::{
    EntityType, PendingByType, QueueHealth, QueueStats, QueuedSync, SyncConflictRecord,
    SyncOperation,
};

/// Persistent sync queue backed by SQLite
pub struct SyncQueue {
    conn: Mutex<Connection>,
    /// The `.cas` directory this queue lives in — the project root the push
    /// guard classifies when a syncer is built without an explicit root.
    cas_dir: std::path::PathBuf,
}

impl SyncQueue {
    /// Open or create a sync queue using the cas.db database
    pub fn open(cas_dir: &Path) -> Result<Self, CasError> {
        let db_path = cas_dir.join("cas.db");
        let conn = Connection::open(&db_path)?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        Ok(Self {
            conn: Mutex::new(conn),
            cas_dir: cas_dir.to_path_buf(),
        })
    }

    /// Open an existing queue without creating or mutating its SQLite file.
    ///
    /// Factory preflight uses this path: readiness checks must not turn a
    /// missing database into a newly-created one just by looking at it.
    pub fn open_read_only(cas_dir: &Path) -> Result<Self, CasError> {
        let db_path = cas_dir.join("cas.db");
        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cas_dir: cas_dir.to_path_buf(),
        })
    }

    /// The `.cas` directory this queue was opened in.
    pub fn cas_dir(&self) -> &Path {
        &self.cas_dir
    }

    /// Initialize the sync queue tables
    pub fn init(&self) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(schema::SCHEMA)?;
        for statement in TASK_DEPENDENCY_TOMBSTONE_STATEMENTS
            .iter()
            .chain(SYNC_REVISION_STATEMENTS.iter())
        {
            conn.execute_batch(statement)?;
        }
        for statement in QUARANTINED_ROW_STATEMENTS {
            conn.execute_batch(statement)?;
        }

        // Migration: add team_id column if missing (for existing databases)
        self.migrate_team_id(&conn)?;
        self.migrate_conflict_revisions(&conn)?;

        // Migration: add the per-row cloud verdict columns. This runs after the
        // team_id migration because that path can rebuild sync_queue from an
        // explicit legacy column list.
        self.migrate_row_outcomes(&conn)?;

        Ok(())
    }
}
