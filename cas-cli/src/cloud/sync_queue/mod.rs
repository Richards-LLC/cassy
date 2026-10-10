//! Persistent sync queue for cloud synchronization
//!
//! Queues local changes for eventual sync to cloud. Provides offline resilience
//! by persisting the queue to SQLite.
//!
//! # Integration Status
//! Queue infrastructure ready for cloud sync feature.

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

use crate::error::CasError;

mod dependency_repair;
mod dependency_tombstones;
mod maintenance;
mod metadata;
mod quarantine;
mod queue_ops;
mod revisions;
mod schema;
mod stats;
mod task_intents;
#[cfg(test)]
mod tests;
mod types;
mod unauthored;

pub(crate) use task_intents::{
    TaskSyncFulfillMode, TaskSyncFulfillResult, TaskSyncIntent, TaskSyncPayload,
};

pub use dependency_tombstones::{
    TASK_DEPENDENCY_TOMBSTONE_RETENTION_DAYS, TASK_DEPENDENCY_TOMBSTONE_STATEMENTS,
};
pub use quarantine::{
    PULL_ID_COLLISION, QUARANTINE_TASK, QUARANTINED_ROW_STATEMENTS, QuarantinedRow,
};
pub use revisions::{SYNC_REVISION_STATEMENTS, parse_wire_revision, wire_revision};
pub use types::{
    EntityType, PendingByType, QueueHealth, QueueStats, QueuedSync, SyncConflictRecord,
    SyncOperation,
};
pub use unauthored::UNAUTHORED_PULL_STATEMENTS;

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
        // cas-0e57: without a busy handler every statement on this connection
        // fails the instant another connection holds the write lock. The task
        // store opens this queue on every logged-in open, so shutdown_workers'
        // safety check and spawn_workers failed with "database is locked"
        // while the database was accepting writes.
        // GH #1165: the pool's budget-aware handler, so a thread under a
        // store wait budget (the factory daemon loop) stops at its deadline.
        cas_store::shared_db::install_busy_handler(&conn)?;

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

    /// Recover an origin from the persisted row for a queue payload written
    /// before `origin_project` was part of entry/rule/task JSON. A present row
    /// with NULL origin is returned as `unknown` so a stale payload cannot
    /// override its missing provenance.
    pub fn stored_origin_project(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<Option<String>, CasError> {
        let table = match entity_type {
            EntityType::Entry => "entries",
            EntityType::Rule => "rules",
            EntityType::Task => "tasks",
            _ => return Ok(None),
        };
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                &format!("SELECT origin_project FROM {table} WHERE id = ?1"),
                params![entity_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .map(|origin| origin.unwrap_or_else(|| "unknown".to_string())))
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
        for statement in QUARANTINED_ROW_STATEMENTS
            .iter()
            .chain(UNAUTHORED_PULL_STATEMENTS.iter())
        {
            conn.execute_batch(statement)?;
        }

        self.migrate_task_sync_intent_bindings(&conn)?;

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
