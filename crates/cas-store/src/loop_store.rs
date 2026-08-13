//! Loop storage for iteration loops
//!
//! Stores loop state in SQLite for persistence across hook invocations.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{Loop, LoopStatus};

// Helper to convert lock errors
fn lock_err<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Parse("Failed to acquire lock".to_string())
}

/// SQLite DDL for the `loops` table and its indexes.
///
/// Re-exported via `cas_store::LOOP_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base table before applying ALTER migrations.
/// See cas-bdb9 / EPIC cas-9fdb.
pub const LOOP_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS loops (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    completion_promise TEXT,
    iteration INTEGER NOT NULL DEFAULT 1,
    max_iterations INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active',
    task_id TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    end_reason TEXT,
    cwd TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_loops_session ON loops(session_id);
CREATE INDEX IF NOT EXISTS idx_loops_status ON loops(status);
CREATE INDEX IF NOT EXISTS idx_loops_started ON loops(started_at DESC);
"#;

/// Trait for loop storage operations
pub trait LoopStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Generate a new unique loop ID (e.g., loop-a1b2)
    fn generate_id(&self) -> Result<String>;

    /// Add a new loop
    fn add(&self, loop_state: &Loop) -> Result<()>;

    /// Get a loop by ID
    fn get(&self, id: &str) -> Result<Loop>;

    /// Update an existing loop
    fn update(&self, loop_state: &Loop) -> Result<()>;

    /// Delete a loop
    fn delete(&self, id: &str) -> Result<()>;

    /// Get active loop for a session (if any)
    fn get_active_for_session(&self, session_id: &str) -> Result<Option<Loop>>;

    /// List recent loops
    fn list_recent(&self, limit: usize) -> Result<Vec<Loop>>;

    /// Delete loops older than the given number of days
    fn prune(&self, older_than_days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based loop store
pub struct SqliteLoopStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteLoopStore {
    /// Open or create a SQLite loop store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };

        store.init()?;
        Ok(store)
    }

    fn parse_loop(row: &rusqlite::Row) -> rusqlite::Result<Loop> {
        let status_str: String = row.get(6)?;
        let status = LoopStatus::from_str(&status_str).unwrap_or_default();

        let started_at_str: String = row.get(8)?;
        let started_at = DateTime::parse_from_rfc3339(&started_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let ended_at: Option<DateTime<Utc>> = row
            .get::<_, Option<String>>(9)?
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(Loop {
            id: row.get(0)?,
            session_id: row.get(1)?,
            prompt: row.get(2)?,
            completion_promise: row.get(3)?,
            iteration: row.get::<_, i32>(4)? as u32,
            max_iterations: row.get::<_, i32>(5)? as u32,
            status,
            task_id: row.get(7)?,
            started_at,
            ended_at,
            end_reason: row.get(10)?,
            cwd: row.get(11)?,
        })
    }
}

impl LoopStore for SqliteLoopStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute_batch(LOOP_SCHEMA)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                StoreError::Other(format!("system clock predates Unix epoch: {error}"))
            })?
            .as_millis();

        // Generate a short hash from timestamp
        let hash = format!("{timestamp:x}");
        let short_hash = &hash[hash.len().saturating_sub(4)..];

        Ok(format!("loop-{short_hash}"))
    }

    fn add(&self, loop_state: &Loop) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        conn.execute(
            "INSERT INTO loops (id, session_id, prompt, completion_promise, iteration,
             max_iterations, status, task_id, started_at, ended_at, end_reason, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                loop_state.id,
                loop_state.session_id,
                loop_state.prompt,
                loop_state.completion_promise,
                loop_state.iteration as i32,
                loop_state.max_iterations as i32,
                loop_state.status.to_string(),
                loop_state.task_id,
                loop_state.started_at.to_rfc3339(),
                loop_state.ended_at.map(|dt| dt.to_rfc3339()),
                loop_state.end_reason,
                loop_state.cwd,
            ],
        )?;

        Ok(())
    }

    fn get(&self, id: &str) -> Result<Loop> {
        let conn = self.conn.lock().map_err(lock_err)?;

        conn.query_row(
            "SELECT id, session_id, prompt, completion_promise, iteration, max_iterations,
             status, task_id, started_at, ended_at, end_reason, cwd
             FROM loops WHERE id = ?1",
            params![id],
            Self::parse_loop,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(id.to_string()),
            _ => StoreError::Database(e),
        })
    }

    fn update(&self, loop_state: &Loop) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let rows = conn.execute(
            "UPDATE loops SET session_id = ?2, prompt = ?3, completion_promise = ?4,
             iteration = ?5, max_iterations = ?6, status = ?7, task_id = ?8,
             started_at = ?9, ended_at = ?10, end_reason = ?11, cwd = ?12
             WHERE id = ?1",
            params![
                loop_state.id,
                loop_state.session_id,
                loop_state.prompt,
                loop_state.completion_promise,
                loop_state.iteration as i32,
                loop_state.max_iterations as i32,
                loop_state.status.to_string(),
                loop_state.task_id,
                loop_state.started_at.to_rfc3339(),
                loop_state.ended_at.map(|dt| dt.to_rfc3339()),
                loop_state.end_reason,
                loop_state.cwd,
            ],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(loop_state.id.clone()));
        }

        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let rows = conn.execute("DELETE FROM loops WHERE id = ?1", params![id])?;

        if rows == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }

        Ok(())
    }

    fn get_active_for_session(&self, session_id: &str) -> Result<Option<Loop>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        conn.query_row(
            "SELECT id, session_id, prompt, completion_promise, iteration, max_iterations,
             status, task_id, started_at, ended_at, end_reason, cwd
             FROM loops WHERE session_id = ?1 AND status = 'active'
             ORDER BY started_at DESC LIMIT 1",
            params![session_id],
            Self::parse_loop,
        )
        .optional()
        .map_err(StoreError::Database)
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Loop>> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, prompt, completion_promise, iteration, max_iterations,
             status, task_id, started_at, ended_at, end_reason, cwd
             FROM loops ORDER BY started_at DESC LIMIT ?1",
        )?;

        let loops = stmt
            .query_map(params![limit as i32], Self::parse_loop)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(loops)
    }

    fn prune(&self, older_than_days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        let rows = conn.execute("DELETE FROM loops WHERE started_at < ?", params![cutoff])?;

        Ok(rows)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

// Import FromStr for LoopStatus
use std::str::FromStr;

#[cfg(test)]
mod tests {
    use crate::loop_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (SqliteLoopStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = SqliteLoopStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn test_add_and_get_loop() {
        let (store, _dir) = create_test_store();

        let loop_state = Loop::new(
            "loop-test".to_string(),
            "session-123".to_string(),
            "Build something".to_string(),
            "/project".to_string(),
        );

        store.add(&loop_state).unwrap();

        let retrieved = store.get("loop-test").unwrap();
        assert_eq!(retrieved.id, "loop-test");
        assert_eq!(retrieved.session_id, "session-123");
        assert_eq!(retrieved.prompt, "Build something");
        assert_eq!(retrieved.iteration, 1);
        assert!(retrieved.is_active());
    }

    #[test]
    fn test_update_loop() {
        let (store, _dir) = create_test_store();

        let mut loop_state = Loop::new(
            "loop-test".to_string(),
            "session-123".to_string(),
            "Build something".to_string(),
            "/project".to_string(),
        );

        store.add(&loop_state).unwrap();

        loop_state.increment();
        loop_state.increment();
        store.update(&loop_state).unwrap();

        let retrieved = store.get("loop-test").unwrap();
        assert_eq!(retrieved.iteration, 3);
    }

    #[test]
    fn test_get_active_for_session() {
        let (store, _dir) = create_test_store();

        // No active loop initially
        let result = store.get_active_for_session("session-123").unwrap();
        assert!(result.is_none());

        // Add an active loop
        let loop_state = Loop::new(
            "loop-test".to_string(),
            "session-123".to_string(),
            "Build something".to_string(),
            "/project".to_string(),
        );
        store.add(&loop_state).unwrap();

        let result = store.get_active_for_session("session-123").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "loop-test");

        // Complete the loop
        let mut completed = store.get("loop-test").unwrap();
        completed.complete("Done");
        store.update(&completed).unwrap();

        // No active loop anymore
        let result = store.get_active_for_session("session-123").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_recent() {
        let (store, _dir) = create_test_store();

        for i in 0..5 {
            let loop_state = Loop::new(
                format!("loop-{i}"),
                "session-123".to_string(),
                format!("Task {i}"),
                "/project".to_string(),
            );
            store.add(&loop_state).unwrap();
        }

        let recent = store.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_delete_loop() {
        let (store, _dir) = create_test_store();

        let loop_state = Loop::new(
            "loop-test".to_string(),
            "session-123".to_string(),
            "Build something".to_string(),
            "/project".to_string(),
        );

        store.add(&loop_state).unwrap();
        store.delete("loop-test").unwrap();

        let result = store.get("loop-test");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_id() {
        let (store, _dir) = create_test_store();

        let id1 = store.generate_id().unwrap();
        let id2 = store.generate_id().unwrap();

        assert!(id1.starts_with("loop-"));
        assert!(id2.starts_with("loop-"));
        // IDs should be unique (though in rapid succession might be same)
    }

    #[test]
    fn test_loop_with_options() {
        let (store, _dir) = create_test_store();

        let loop_state = Loop::with_options(
            "loop-test".to_string(),
            "session-123".to_string(),
            "Build something".to_string(),
            "/project".to_string(),
            Some("DONE".to_string()),
            10,
            Some("cas-task-1".to_string()),
        );

        store.add(&loop_state).unwrap();

        let retrieved = store.get("loop-test").unwrap();
        assert_eq!(retrieved.completion_promise, Some("DONE".to_string()));
        assert_eq!(retrieved.max_iterations, 10);
        assert_eq!(retrieved.task_id, Some("cas-task-1".to_string()));
    }
}
