//! Spawn queue for worker lifecycle commands in factory sessions
//!
//! Allows CLI commands and supervisor agents to request worker spawn/shutdown.
//! Factory TUI polls this queue and processes the requests.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;

/// Action type for spawn queue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpawnAction {
    /// Spawn new workers
    Spawn,
    /// Shutdown existing workers
    Shutdown,
    /// Respawn crashed workers (reuse existing clone)
    Respawn,
}

impl SpawnAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Shutdown => "shutdown",
            Self::Respawn => "respawn",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "spawn" => Some(Self::Spawn),
            "shutdown" => Some(Self::Shutdown),
            "respawn" => Some(Self::Respawn),
            _ => None,
        }
    }
}

/// A request in the spawn queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    /// Unique request ID
    pub id: i64,
    /// Action type (spawn or shutdown)
    pub action: SpawnAction,
    /// Number of workers (for spawn: how many to create; for shutdown: how many to remove, 0 = all)
    pub count: Option<i32>,
    /// Specific worker names (comma-separated in DB, Vec here)
    pub worker_names: Vec<String>,
    /// Force operation even with dirty worktree (for shutdown)
    pub force: bool,
    /// Whether spawned workers should be isolated in their own git worktrees
    pub isolate: bool,
    /// Per-worker spec override serialized as JSON (cas-2992).
    ///
    /// `Some(json)` carries a `WorkerSpec`-compatible JSON object.  Callers
    /// in `cas-cli` (which depend on `cas-mux`) deserialise this into a
    /// `WorkerSpec` at consumption time.  `None` means "use session default".
    pub worker_spec: Option<String>,
    /// Factory session that owns this request.
    ///
    /// `None` preserves legacy/non-factory behavior: any daemon may process it.
    pub factory_session: Option<String>,
    /// Task to pre-assign to the spawned worker (cas-6913).
    ///
    /// `Some(task_id)` only makes sense for single-worker spawn requests —
    /// callers must validate cardinality before enqueueing (see
    /// `factory_spawn_workers`). `None` preserves the pre-cas-6913 behavior
    /// of no auto-assignment.
    pub task_id: Option<String>,
    /// Requesting supervisor's Claude account directory, captured at enqueue
    /// time so the daemon does not substitute its own environment.
    pub requester_config_dir: Option<String>,
    /// When the request was queued
    pub created_at: DateTime<Utc>,
    /// When the request was processed (None if pending)
    pub processed_at: Option<DateTime<Utc>>,
}

/// Schema for spawn queue table.
///
/// `pub` (re-exported from `cas-store`'s root, matching `AGENT_SCHEMA` /
/// `TASK_SCHEMA`) so migration-side class-guard tests in `cas-cli` can
/// assert the baseline (fresh-DB) shape and the post-migration (upgraded-DB)
/// shape agree column-for-column — see m205's
/// `baseline_agent_schema_applies_over_pre_m204_table` for the pattern this
/// guards against: a column that exists on only one of the two paths is
/// invisible to any test that only ever creates fresh DBs (hotfix 4efed95).
pub const SPAWN_QUEUE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS spawn_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    action TEXT NOT NULL,
    count INTEGER,
    worker_names TEXT,
    force INTEGER NOT NULL DEFAULT 0,
    isolate INTEGER NOT NULL DEFAULT 0,
    worker_spec TEXT,
    factory_session TEXT,
    task_id TEXT,
    requester_config_dir TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    processed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_spawn_queue_pending ON spawn_queue(action) WHERE processed_at IS NULL;
"#;

/// Trait for spawn queue operations
pub trait SpawnQueueStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Queue a spawn request.
    ///
    /// `spec_json` is an optional JSON-serialised `WorkerSpec` that callers in
    /// `cas-cli` (which depend on `cas-mux`) produce from `cli`/`model`/`effort`
    /// overrides.  `None` means "use the session default".  This field is stored
    /// in the `worker_spec` column added by migration m201.
    ///
    /// `task_id` (cas-6913) pre-assigns a task to the spawned worker once the
    /// spawn completes. Only meaningful for single-worker requests — callers
    /// must validate cardinality before calling this (the store does not
    /// enforce it, since the store doesn't know how many workers `count`
    /// will actually produce). Stored in the `task_id` column added by
    /// migration m206.
    fn enqueue_spawn(
        &self,
        count: i32,
        worker_names: &[String],
        isolate: bool,
        spec_json: Option<&str>,
        factory_session: Option<&str>,
        task_id: Option<&str>,
    ) -> Result<i64>;

    /// Queue a spawn request while preserving the requesting supervisor's
    /// effective Claude account directory for daemon-side spawning.
    fn enqueue_spawn_with_requester_config_dir(
        &self,
        count: i32,
        worker_names: &[String],
        isolate: bool,
        spec_json: Option<&str>,
        factory_session: Option<&str>,
        task_id: Option<&str>,
        requester_config_dir: Option<&str>,
    ) -> Result<i64> {
        let _ = requester_config_dir;
        self.enqueue_spawn(
            count,
            worker_names,
            isolate,
            spec_json,
            factory_session,
            task_id,
        )
    }

    /// Queue a shutdown request
    fn enqueue_shutdown(
        &self,
        count: Option<i32>,
        worker_names: &[String],
        force: bool,
        factory_session: Option<&str>,
    ) -> Result<i64>;

    /// Queue a respawn request (for crashed workers)
    fn enqueue_respawn(
        &self,
        worker_names: &[String],
        factory_session: Option<&str>,
    ) -> Result<i64>;

    /// Poll for pending requests owned by this session, plus legacy unscoped rows.
    fn poll(&self, factory_session: &str, limit: usize) -> Result<Vec<SpawnRequest>>;

    /// Peek at pending requests without marking as processed
    fn peek(&self, limit: usize) -> Result<Vec<SpawnRequest>>;

    /// Mark a request as processed
    fn mark_processed(&self, request_id: i64) -> Result<()>;

    /// Get count of pending requests
    fn pending_count(&self) -> Result<usize>;

    /// Clear all requests (for cleanup)
    fn clear(&self) -> Result<usize>;

    /// Clear old processed requests (cleanup)
    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based spawn queue store
pub struct SqliteSpawnQueueStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSpawnQueueStore {
    /// Open or create a SQLite spawn queue store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        Ok(Self { conn })
    }

    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    fn parse_worker_names(s: Option<String>) -> Vec<String> {
        s.map(|names| {
            names
                .split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
    }

    fn request_from_row(row: &rusqlite::Row) -> rusqlite::Result<SpawnRequest> {
        let action_str: String = row.get(1)?;
        let action = SpawnAction::from_str(&action_str).unwrap_or(SpawnAction::Spawn);
        let worker_names_str: Option<String> = row.get(3)?;
        let force: i32 = row.get(4).unwrap_or(0);
        let isolate: i32 = row.get(5).unwrap_or(0);
        let worker_spec: Option<String> = row.get(6).unwrap_or_default();
        let factory_session: Option<String> = row.get(7).unwrap_or_default();
        let task_id: Option<String> = row.get(8).unwrap_or_default();
        let requester_config_dir: Option<String> = row.get(9).unwrap_or_default();
        let processed_at_str: Option<String> = row.get(11)?;

        Ok(SpawnRequest {
            id: row.get(0)?,
            action,
            count: row.get(2)?,
            worker_names: Self::parse_worker_names(worker_names_str),
            force: force != 0,
            isolate: isolate != 0,
            worker_spec,
            factory_session,
            task_id,
            requester_config_dir,
            created_at: Self::parse_datetime(&row.get::<_, String>(10)?).unwrap_or_else(Utc::now),
            processed_at: processed_at_str.and_then(|s| Self::parse_datetime(&s)),
        })
    }

    fn enqueue(
        &self,
        action: SpawnAction,
        count: Option<i32>,
        worker_names: &[String],
        force: bool,
        isolate: bool,
        spec_json: Option<&str>,
        factory_session: Option<&str>,
        task_id: Option<&str>,
        requester_config_dir: Option<&str>,
    ) -> Result<i64> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            let names = if worker_names.is_empty() {
                None
            } else {
                Some(worker_names.join(","))
            };

            conn.execute(
                "INSERT INTO spawn_queue (action, count, worker_names, force, isolate, worker_spec, factory_session, task_id, requester_config_dir, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    action.as_str(),
                    count,
                    names,
                    force as i32,
                    isolate as i32,
                    spec_json,
                    factory_session,
                    task_id,
                    requester_config_dir,
                    now
                ],
            )?;

            let id = conn.last_insert_rowid();
            Ok(id)
        }) // with_write_retry
    }
}

impl SpawnQueueStore for SqliteSpawnQueueStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(SPAWN_QUEUE_SCHEMA)?;
        // Note: force/isolate columns are now in SPAWN_QUEUE_SCHEMA inline.
        // Old DBs are upgraded via migration m193_spawn_queue_force_isolate.
        Ok(())
    }

    fn enqueue_spawn(
        &self,
        count: i32,
        worker_names: &[String],
        isolate: bool,
        spec_json: Option<&str>,
        factory_session: Option<&str>,
        task_id: Option<&str>,
    ) -> Result<i64> {
        self.enqueue(
            SpawnAction::Spawn,
            Some(count),
            worker_names,
            false,
            isolate,
            spec_json,
            factory_session,
            task_id,
            None,
        )
    }

    fn enqueue_spawn_with_requester_config_dir(
        &self,
        count: i32,
        worker_names: &[String],
        isolate: bool,
        spec_json: Option<&str>,
        factory_session: Option<&str>,
        task_id: Option<&str>,
        requester_config_dir: Option<&str>,
    ) -> Result<i64> {
        self.enqueue(
            SpawnAction::Spawn,
            Some(count),
            worker_names,
            false,
            isolate,
            spec_json,
            factory_session,
            task_id,
            requester_config_dir,
        )
    }

    fn enqueue_shutdown(
        &self,
        count: Option<i32>,
        worker_names: &[String],
        force: bool,
        factory_session: Option<&str>,
    ) -> Result<i64> {
        self.enqueue(
            SpawnAction::Shutdown,
            count,
            worker_names,
            force,
            false,
            None,
            factory_session,
            None,
            None,
        )
    }

    fn enqueue_respawn(
        &self,
        worker_names: &[String],
        factory_session: Option<&str>,
    ) -> Result<i64> {
        self.enqueue(
            SpawnAction::Respawn,
            None,
            worker_names,
            false,
            false,
            None,
            factory_session,
            None,
            None,
        )
    }

    fn poll(&self, factory_session: &str, limit: usize) -> Result<Vec<SpawnRequest>> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        let mut stmt = conn.prepare_cached(
            "SELECT id, action, count, worker_names, force, isolate, worker_spec, factory_session, task_id, requester_config_dir, created_at, processed_at
             FROM spawn_queue
             WHERE processed_at IS NULL
               AND (factory_session = ? OR factory_session IS NULL)
             ORDER BY created_at ASC
             LIMIT ?",
        )?;

        let requests: Vec<SpawnRequest> = stmt
            .query_map(
                params![factory_session, limit as i64],
                Self::request_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Mark them as processed
        if !requests.is_empty() {
            let ids: Vec<i64> = requests.iter().map(|r| r.id).collect();
            let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
            let sql = format!(
                "UPDATE spawn_queue SET processed_at = ? WHERE id IN ({})",
                placeholders.join(", ")
            );

            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now)];
            for id in ids {
                params.push(Box::new(id));
            }

            conn.execute(
                &sql,
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            )?;
        }

        Ok(requests)
    }

    fn peek(&self, limit: usize) -> Result<Vec<SpawnRequest>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare_cached(
            "SELECT id, action, count, worker_names, force, isolate, worker_spec, factory_session, task_id, requester_config_dir, created_at, processed_at
             FROM spawn_queue
             WHERE processed_at IS NULL
             ORDER BY created_at ASC
             LIMIT ?",
        )?;

        let requests = stmt
            .query_map(params![limit as i64], Self::request_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(requests)
    }

    fn mark_processed(&self, request_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE spawn_queue SET processed_at = ? WHERE id = ?",
            params![now, request_id],
        )?;

        Ok(())
    }

    fn pending_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM spawn_queue WHERE processed_at IS NULL",
            [],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    fn clear(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM spawn_queue", [])?;
        Ok(rows)
    }

    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();

        let rows = conn.execute(
            "DELETE FROM spawn_queue WHERE processed_at IS NOT NULL AND processed_at < ?",
            params![cutoff],
        )?;

        Ok(rows)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::spawn_queue_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqliteSpawnQueueStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteSpawnQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    #[test]
    fn test_enqueue_spawn_and_poll() {
        let (_temp, store) = create_test_store();

        // Queue a spawn request
        let id = store
            .enqueue_spawn(2, &[], false, None, Some("session-a"), None)
            .unwrap();
        assert!(id > 0);

        // Poll should return it
        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, SpawnAction::Spawn);
        assert_eq!(requests[0].count, Some(2));
        assert!(requests[0].worker_names.is_empty());
        assert_eq!(requests[0].factory_session.as_deref(), Some("session-a"));

        // Polling again should return empty (already processed)
        let requests = store.poll("session-a", 10).unwrap();
        assert!(requests.is_empty());
    }

    #[test]
    fn test_enqueue_shutdown_with_names() {
        let (_temp, store) = create_test_store();

        // Queue a shutdown request with specific workers
        let names = vec!["swift-fox".to_string(), "calm-owl".to_string()];
        store
            .enqueue_shutdown(None, &names, false, Some("session-a"))
            .unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, SpawnAction::Shutdown);
        assert_eq!(requests[0].count, None);
        assert_eq!(requests[0].worker_names, names);
        assert!(!requests[0].force);
        assert_eq!(requests[0].factory_session.as_deref(), Some("session-a"));
    }

    #[test]
    fn test_enqueue_shutdown_with_force() {
        let (_temp, store) = create_test_store();

        // Queue a shutdown request with force=true
        store
            .enqueue_shutdown(Some(1), &[], true, Some("session-a"))
            .unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, SpawnAction::Shutdown);
        assert!(requests[0].force);
    }

    #[test]
    fn test_peek_does_not_process() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(3, &[], false, None, Some("session-a"), None)
            .unwrap();

        // Peek should return request
        let requests = store.peek(10).unwrap();
        assert_eq!(requests.len(), 1);

        // Peek again should still return it
        let requests = store.peek(10).unwrap();
        assert_eq!(requests.len(), 1);

        // Pending count should be 1
        assert_eq!(store.pending_count().unwrap(), 1);
    }

    #[test]
    fn test_fifo_ordering() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
            .unwrap();
        store
            .enqueue_spawn(2, &[], false, None, Some("session-a"), None)
            .unwrap();
        store
            .enqueue_shutdown(None, &["worker-1".to_string()], false, Some("session-a"))
            .unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].action, SpawnAction::Spawn);
        assert_eq!(requests[0].count, Some(1));
        assert_eq!(requests[1].action, SpawnAction::Spawn);
        assert_eq!(requests[1].count, Some(2));
        assert_eq!(requests[2].action, SpawnAction::Shutdown);
    }

    #[test]
    fn test_spawn_action_serialization() {
        assert_eq!(SpawnAction::Spawn.as_str(), "spawn");
        assert_eq!(SpawnAction::Shutdown.as_str(), "shutdown");
        assert_eq!(SpawnAction::Respawn.as_str(), "respawn");
        assert_eq!(SpawnAction::from_str("spawn"), Some(SpawnAction::Spawn));
        assert_eq!(
            SpawnAction::from_str("SHUTDOWN"),
            Some(SpawnAction::Shutdown)
        );
        assert_eq!(SpawnAction::from_str("respawn"), Some(SpawnAction::Respawn));
        assert_eq!(SpawnAction::from_str("invalid"), None);
    }

    #[test]
    fn test_enqueue_respawn() {
        let (_temp, store) = create_test_store();

        // Queue a respawn request
        let names = vec!["crashed-worker".to_string()];
        store.enqueue_respawn(&names, Some("session-a")).unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, SpawnAction::Respawn);
        assert_eq!(requests[0].count, None);
        assert_eq!(requests[0].worker_names, names);
    }

    #[test]
    fn test_enqueue_spawn_with_isolate() {
        let (_temp, store) = create_test_store();

        // Queue a spawn request with isolate=true
        store
            .enqueue_spawn(2, &[], true, None, Some("session-a"), None)
            .unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, SpawnAction::Spawn);
        assert!(requests[0].isolate);

        // Queue a spawn request with isolate=false
        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
            .unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].isolate);
    }

    #[test]
    fn test_enqueue_spawn_with_task_id_persists_and_dequeues() {
        // cas-6913: verify task_id round-trips through the queue (poll and peek).
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), Some("cas-abc1"))
            .unwrap();

        let peeked = store.peek(10).unwrap();
        assert_eq!(peeked.len(), 1);
        assert_eq!(peeked[0].task_id.as_deref(), Some("cas-abc1"));

        let polled = store.poll("session-a", 10).unwrap();
        assert_eq!(polled.len(), 1);
        assert_eq!(polled[0].task_id.as_deref(), Some("cas-abc1"));
    }

    #[test]
    fn test_enqueue_spawn_without_task_id_is_none() {
        // Backwards compat: enqueue without task_id -> task_id is None.
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
            .unwrap();

        let requests = store.peek(10).unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].task_id.is_none(),
            "task_id should be None when not supplied"
        );
    }

    #[test]
    fn test_enqueue_spawn_with_spec_json_persists_and_dequeues() {
        // cas-2992: verify worker_spec JSON round-trips through the queue.
        let (_temp, store) = create_test_store();

        let spec_json = r#"{"name":null,"cli":"codex","model":null,"effort":"high"}"#;
        store
            .enqueue_spawn(1, &[], false, Some(spec_json), Some("session-a"), None)
            .unwrap();

        let requests = store.peek(10).unwrap();
        assert_eq!(requests.len(), 1);
        let stored = requests[0]
            .worker_spec
            .as_deref()
            .expect("worker_spec should be set");
        assert!(
            stored.contains("codex"),
            "spec should contain 'codex': {stored}"
        );
    }

    #[test]
    fn test_enqueue_spawn_preserves_requester_config_dir() {
        let (_temp, store) = create_test_store();
        let spec_json = r#"{"name":null,"cli":"claude","model":null,"effort":"high","config_dir":"~/.claude-explicit"}"#;

        store
            .enqueue_spawn_with_requester_config_dir(
                1,
                &[],
                false,
                Some(spec_json),
                Some("session-a"),
                None,
                Some("~/.claude-supervisor"),
            )
            .unwrap();

        let requests = store.peek(10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].requester_config_dir.as_deref(),
            Some("~/.claude-supervisor")
        );
        assert!(
            requests[0]
                .worker_spec
                .as_deref()
                .is_some_and(|spec| spec.contains(".claude-explicit")),
            "explicit config_dir must remain in worker_spec"
        );
    }

    #[test]
    fn test_enqueue_spawn_without_spec_is_none() {
        // Backwards compat: enqueue without spec → worker_spec is None.
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
            .unwrap();

        let requests = store.peek(10).unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].worker_spec.is_none(),
            "worker_spec should be None when no spec supplied"
        );
    }

    #[test]
    fn test_poll_filters_to_session_and_legacy_null_rows() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
            .unwrap();
        store
            .enqueue_shutdown(
                None,
                &["session-b-worker".to_string()],
                false,
                Some("session-b"),
            )
            .unwrap();
        store
            .enqueue_respawn(&["legacy-worker".to_string()], None)
            .unwrap();

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].action, SpawnAction::Spawn);
        assert_eq!(requests[0].factory_session.as_deref(), Some("session-a"));
        assert_eq!(requests[1].action, SpawnAction::Respawn);
        assert_eq!(requests[1].factory_session, None);

        let requests = store.poll("session-b", 10).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].action, SpawnAction::Shutdown);
        assert_eq!(requests[0].factory_session.as_deref(), Some("session-b"));
        assert_eq!(requests[0].worker_names, vec!["session-b-worker"]);
    }

    #[test]
    fn test_poll_does_not_process_other_session_rows() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
            .unwrap();

        let requests = store.poll("session-b", 10).unwrap();
        assert!(requests.is_empty());
        assert_eq!(store.pending_count().unwrap(), 1);

        let requests = store.poll("session-a", 10).unwrap();
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn test_legacy_null_session_rows_keep_single_session_behavior() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_spawn(2, &[], false, None, None, None)
            .unwrap();
        store.enqueue_shutdown(Some(1), &[], true, None).unwrap();

        let requests = store.poll("any-session", 10).unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|request| request.factory_session.is_none())
        );
    }
}
