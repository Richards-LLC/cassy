//! Supervisor queue storage for factory session Director → Supervisor communication
//!
//! The supervisor queue allows the Director (TUI) to batch notifications and
//! send them to the Supervisor agent. This enables asynchronous, prioritized
//! event delivery in multi-agent factory sessions.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;

/// Priority levels for supervisor queue notifications
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum NotificationPriority {
    /// Critical - agent died, blocking error
    Critical = 0,
    /// High - task completed, needs review
    High = 1,
    /// Normal - status update, informational
    Normal = 2,
}

impl From<u8> for NotificationPriority {
    fn from(value: u8) -> Self {
        match value {
            0 => NotificationPriority::Critical,
            1 => NotificationPriority::High,
            _ => NotificationPriority::Normal,
        }
    }
}

impl From<NotificationPriority> for i32 {
    fn from(value: NotificationPriority) -> Self {
        value as i32
    }
}

/// A notification in the supervisor queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorNotification {
    /// Unique notification ID
    pub id: i64,
    /// Target supervisor agent ID
    pub supervisor_id: String,
    /// Type of event (e.g., "task_completed", "worker_died", "task_blocked")
    pub event_type: String,
    /// JSON payload with event details
    pub payload: String,
    /// Notification priority
    pub priority: NotificationPriority,
    /// When the notification was created
    pub created_at: DateTime<Utc>,
    /// When the notification was processed (None if pending)
    pub processed_at: Option<DateTime<Utc>>,
    /// Optional idempotency key (cas-062d / cas-17e4 lifecycle transitions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_key: Option<String>,
    /// When prompt_queue delivery for this row succeeded (cas-17e4 outbox).
    /// None means durable event may exist without real-time prompt delivery yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_delivered_at: Option<DateTime<Utc>>,
}

/// Result of an idempotent notify (cas-062d / cas-17e4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyIdempotentResult {
    /// New row inserted (prompt not yet delivered).
    Created(i64),
    /// Existing row for the same transition_key (no duplicate insert).
    /// `prompt_delivered` is true when outbox prompt handoff already completed.
    AlreadyExists {
        id: i64,
        prompt_delivered: bool,
    },
}

/// Schema for supervisor queue table
const SUPERVISOR_QUEUE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS supervisor_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    supervisor_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 2,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    processed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_supervisor_queue_supervisor ON supervisor_queue(supervisor_id);
CREATE INDEX IF NOT EXISTS idx_supervisor_queue_pending ON supervisor_queue(supervisor_id, priority) WHERE processed_at IS NULL;
"#;

/// Idempotency key for lifecycle transition events (cas-062d).
const SUPERVISOR_QUEUE_TRANSITION_KEY_MIGRATION: &str = r#"
ALTER TABLE supervisor_queue ADD COLUMN transition_key TEXT;
"#;
const SUPERVISOR_QUEUE_TRANSITION_KEY_INDEX: &str = r#"
CREATE UNIQUE INDEX IF NOT EXISTS idx_supervisor_queue_transition_key
    ON supervisor_queue(transition_key)
    WHERE transition_key IS NOT NULL;
"#;
/// Outbox marker: real-time prompt delivery completed (cas-17e4).
const SUPERVISOR_QUEUE_PROMPT_DELIVERED_MIGRATION: &str = r#"
ALTER TABLE supervisor_queue ADD COLUMN prompt_delivered_at TEXT;
"#;

/// Trait for supervisor queue operations
pub trait SupervisorQueueStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Queue a notification for a supervisor
    fn notify(
        &self,
        supervisor_id: &str,
        event_type: &str,
        payload: &str,
        priority: NotificationPriority,
    ) -> Result<i64>;

    /// Idempotent notify keyed by `transition_key` (cas-062d / cas-17e4).
    ///
    /// Replaying the same transition identity returns [`NotifyIdempotentResult::AlreadyExists`]
    /// without inserting a second row. Callers must still attempt prompt delivery when
    /// `prompt_delivered` is false (outbox recovery).
    fn notify_idempotent(
        &self,
        supervisor_id: &str,
        event_type: &str,
        payload: &str,
        priority: NotificationPriority,
        transition_key: &str,
    ) -> Result<NotifyIdempotentResult>;

    /// Mark prompt_queue delivery complete for a durable notification (cas-17e4 outbox).
    /// Idempotent: safe to call again after success.
    fn mark_prompt_delivered(&self, notification_id: i64) -> Result<()>;

    /// Look up a notification by transition_key (repair / tests).
    fn get_by_transition_key(&self, transition_key: &str) -> Result<Option<SupervisorNotification>>;

    /// Look up one durable notification by its public notification ID.
    ///
    /// Lifecycle prompt envelopes expose this ID (not the linked
    /// `prompt_queue.id`), so acknowledgement surfaces use this lookup before
    /// cascading confirmation to the prompt outbox row.
    fn get(&self, notification_id: i64) -> Result<Option<SupervisorNotification>>;

    /// List lifecycle outbox rows awaiting prompt delivery (cas-ecff).
    ///
    /// Returns `task_lifecycle` notifications with a transition_key and
    /// `prompt_delivered_at IS NULL`, oldest first. Used by
    /// `drain_lifecycle_outbox` after process restart.
    fn list_pending_lifecycle_outbox(&self, limit: usize) -> Result<Vec<SupervisorNotification>>;

    /// Poll for pending notifications (ordered by priority, then created_at)
    /// Returns up to `limit` notifications and marks them as processed
    fn poll(&self, supervisor_id: &str, limit: usize) -> Result<Vec<SupervisorNotification>>;

    /// Peek at pending notifications without marking them as processed
    fn peek(&self, supervisor_id: &str, limit: usize) -> Result<Vec<SupervisorNotification>>;

    /// Acknowledge a specific notification (mark as processed)
    fn ack(&self, notification_id: i64) -> Result<()>;

    /// Get count of pending notifications for a supervisor
    fn pending_count(&self, supervisor_id: &str) -> Result<usize>;

    /// Get all pending notifications for a supervisor (without processing)
    fn list_pending(&self, supervisor_id: &str) -> Result<Vec<SupervisorNotification>>;

    /// Clear all notifications for a supervisor (processed and pending)
    fn clear(&self, supervisor_id: &str) -> Result<usize>;

    /// Clear old processed notifications (cleanup)
    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based supervisor queue store
pub struct SqliteSupervisorQueueStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSupervisorQueueStore {
    /// Open or create a SQLite supervisor queue store
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

    fn notification_from_row(row: &rusqlite::Row) -> rusqlite::Result<SupervisorNotification> {
        let processed_at_str: Option<String> = row.get(6)?;
        let processed_at = processed_at_str.and_then(|s| Self::parse_datetime(&s));
        // Column 7 = transition_key (cas-062d); tolerate pre-migration tables.
        let transition_key: Option<String> = row.get(7).unwrap_or(None);
        // Column 8 = prompt_delivered_at (cas-17e4); tolerate pre-migration.
        let prompt_delivered_at_str: Option<String> = row.get(8).unwrap_or(None);
        let prompt_delivered_at =
            prompt_delivered_at_str.and_then(|s| Self::parse_datetime(&s));

        Ok(SupervisorNotification {
            id: row.get(0)?,
            supervisor_id: row.get(1)?,
            event_type: row.get(2)?,
            payload: row.get(3)?,
            priority: NotificationPriority::from(row.get::<_, u8>(4)?),
            created_at: Self::parse_datetime(&row.get::<_, String>(5)?).unwrap_or_else(Utc::now),
            processed_at,
            transition_key,
            prompt_delivered_at,
        })
    }
}

impl SupervisorQueueStore for SqliteSupervisorQueueStore {
    fn init(&self) -> Result<()> {
        // cas-88d8: concurrent openers (daemon + MCP) can race on check-then-ALTER.
        // SQLite auto-commits DDL, so we do NOT wrap ADD COLUMN in ImmediateTx
        // (that leaves the transaction state inconsistent). Instead:
        // ensure_column + with_write_retry, and CREATE INDEX only after columns exist.
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            conn.execute_batch(SUPERVISOR_QUEUE_SCHEMA)?;
            crate::shared_db::ensure_column(
                &conn,
                "supervisor_queue",
                "transition_key",
                SUPERVISOR_QUEUE_TRANSITION_KEY_MIGRATION,
            )?;
            conn.execute_batch(SUPERVISOR_QUEUE_TRANSITION_KEY_INDEX)?;
            crate::shared_db::ensure_column(
                &conn,
                "supervisor_queue",
                "prompt_delivered_at",
                SUPERVISOR_QUEUE_PROMPT_DELIVERED_MIGRATION,
            )?;
            Ok(())
        })
    }

    fn notify(
        &self,
        supervisor_id: &str,
        event_type: &str,
        payload: &str,
        priority: NotificationPriority,
    ) -> Result<i64> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();

            conn.execute(
                "INSERT INTO supervisor_queue (supervisor_id, event_type, payload, priority, created_at)
             VALUES (?, ?, ?, ?, ?)",
                params![supervisor_id, event_type, payload, i32::from(priority), now],
            )?;

            let id = conn.last_insert_rowid();
            Ok(id)
        }) // with_write_retry
    }

    fn notify_idempotent(
        &self,
        supervisor_id: &str,
        event_type: &str,
        payload: &str,
        priority: NotificationPriority,
        transition_key: &str,
    ) -> Result<NotifyIdempotentResult> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();

            // Atomic insert-or-ignore under unique transition_key.
            let changed = conn.execute(
                "INSERT OR IGNORE INTO supervisor_queue
                    (supervisor_id, event_type, payload, priority, created_at, transition_key)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    supervisor_id,
                    event_type,
                    payload,
                    i32::from(priority),
                    now,
                    transition_key
                ],
            )?;

            if changed > 0 {
                return Ok(NotifyIdempotentResult::Created(conn.last_insert_rowid()));
            }

            let (existing_id, prompt_delivered_at): (i64, Option<String>) = conn.query_row(
                "SELECT id, prompt_delivered_at FROM supervisor_queue WHERE transition_key = ?",
                params![transition_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(NotifyIdempotentResult::AlreadyExists {
                id: existing_id,
                prompt_delivered: prompt_delivered_at.is_some(),
            })
        })
    }

    fn mark_prompt_delivered(&self, notification_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            // Only stamp once — preserve original delivery time on replay.
            conn.execute(
                "UPDATE supervisor_queue
                 SET prompt_delivered_at = ?
                 WHERE id = ? AND prompt_delivered_at IS NULL",
                params![now, notification_id],
            )?;
            Ok(())
        })
    }

    fn get_by_transition_key(
        &self,
        transition_key: &str,
    ) -> Result<Option<SupervisorNotification>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at,
                    transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE transition_key = ?
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![transition_key])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::notification_from_row(row)?)),
            None => Ok(None),
        }
    }

    fn get(&self, notification_id: i64) -> Result<Option<SupervisorNotification>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at,
                    transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE id = ?
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![notification_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::notification_from_row(row)?)),
            None => Ok(None),
        }
    }

    fn list_pending_lifecycle_outbox(&self, limit: usize) -> Result<Vec<SupervisorNotification>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at,
                    transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE event_type = 'task_lifecycle'
               AND transition_key IS NOT NULL
               AND prompt_delivered_at IS NULL
             ORDER BY id ASC
             LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], Self::notification_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn poll(&self, supervisor_id: &str, limit: usize) -> Result<Vec<SupervisorNotification>> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();

            // Get pending notifications ordered by priority (ascending = critical first), then created_at
            let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at, transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE supervisor_id = ? AND processed_at IS NULL
             ORDER BY priority ASC, created_at ASC
             LIMIT ?",
        )?;

            let notifications: Vec<SupervisorNotification> = stmt
                .query_map(
                    params![supervisor_id, limit as i64],
                    Self::notification_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            // Mark them as processed
            if !notifications.is_empty() {
                let ids: Vec<i64> = notifications.iter().map(|n| n.id).collect();
                let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
                let sql = format!(
                    "UPDATE supervisor_queue SET processed_at = ? WHERE id IN ({})",
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

            Ok(notifications)
        }) // with_write_retry
    }

    fn peek(&self, supervisor_id: &str, limit: usize) -> Result<Vec<SupervisorNotification>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at, transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE supervisor_id = ? AND processed_at IS NULL
             ORDER BY priority ASC, created_at ASC
             LIMIT ?",
        )?;

        let notifications = stmt
            .query_map(
                params![supervisor_id, limit as i64],
                Self::notification_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(notifications)
    }

    fn ack(&self, notification_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();

            let rows = conn.execute(
            "UPDATE supervisor_queue SET processed_at = ? WHERE id = ? AND processed_at IS NULL",
            params![now, notification_id],
        )?;

            if rows == 0 {
                return Err(StoreError::NotFound(format!(
                    "Notification not found or already processed: {notification_id}"
                )));
            }

            Ok(())
        }) // with_write_retry
    }

    fn pending_count(&self, supervisor_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM supervisor_queue WHERE supervisor_id = ? AND processed_at IS NULL",
            params![supervisor_id],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    fn list_pending(&self, supervisor_id: &str) -> Result<Vec<SupervisorNotification>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at, transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE supervisor_id = ? AND processed_at IS NULL
             ORDER BY priority ASC, created_at ASC",
        )?;

        let notifications = stmt
            .query_map(params![supervisor_id], Self::notification_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(notifications)
    }

    fn clear(&self, supervisor_id: &str) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();

            let rows = conn.execute(
                "DELETE FROM supervisor_queue WHERE supervisor_id = ?",
                params![supervisor_id],
            )?;

            Ok(rows)
        }) // with_write_retry
    }

    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();

            let rows = conn.execute(
                "DELETE FROM supervisor_queue WHERE processed_at IS NOT NULL AND processed_at < ?",
                params![cutoff],
            )?;

            Ok(rows)
        }) // with_write_retry
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::supervisor_queue_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqliteSupervisorQueueStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    #[test]
    fn test_notify_idempotent_no_duplicate() {
        let (_temp, store) = create_test_store();
        let key = "cas-x:in_progress:closed:sess-a:task_closed";
        let r1 = store
            .notify_idempotent(
                "sup-a",
                "task_lifecycle",
                r#"{"task_id":"cas-x"}"#,
                NotificationPriority::High,
                key,
            )
            .unwrap();
        let r2 = store
            .notify_idempotent(
                "sup-a",
                "task_lifecycle",
                r#"{"task_id":"cas-x"}"#,
                NotificationPriority::High,
                key,
            )
            .unwrap();
        let id = match (r1, r2) {
            (
                NotifyIdempotentResult::Created(id1),
                NotifyIdempotentResult::AlreadyExists {
                    id: id2,
                    prompt_delivered,
                },
            ) => {
                assert_eq!(id1, id2);
                assert!(!prompt_delivered, "fresh insert has no prompt delivery yet");
                id1
            }
            other => panic!("expected Created then AlreadyExists, got {other:?}"),
        };
        store.mark_prompt_delivered(id).unwrap();
        let r3 = store
            .notify_idempotent(
                "sup-a",
                "task_lifecycle",
                r#"{"task_id":"cas-x"}"#,
                NotificationPriority::High,
                key,
            )
            .unwrap();
        match r3 {
            NotifyIdempotentResult::AlreadyExists {
                prompt_delivered: true,
                ..
            } => {}
            other => panic!("expected AlreadyExists prompt_delivered=true, got {other:?}"),
        }
        assert_eq!(store.pending_count("sup-a").unwrap(), 1);
        // Different session key is a separate event (isolation).
        store
            .notify_idempotent(
                "sup-b",
                "task_lifecycle",
                r#"{"task_id":"cas-x"}"#,
                NotificationPriority::High,
                "cas-x:in_progress:closed:sess-b:task_closed",
            )
            .unwrap();
        assert_eq!(store.pending_count("sup-b").unwrap(), 1);
        assert_eq!(store.pending_count("sup-a").unwrap(), 1);
    }

    #[test]
    fn test_notify_and_poll() {
        let (_temp, store) = create_test_store();

        // Queue some notifications
        let id1 = store
            .notify(
                "supervisor-1",
                "task_completed",
                r#"{"task_id": "task-1"}"#,
                NotificationPriority::Normal,
            )
            .unwrap();
        let id2 = store
            .notify(
                "supervisor-1",
                "worker_died",
                r#"{"worker_id": "worker-1"}"#,
                NotificationPriority::Critical,
            )
            .unwrap();

        assert!(id1 > 0);
        assert!(id2 > id1);

        // Poll should return critical first
        let notifications = store.poll("supervisor-1", 10).unwrap();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].event_type, "worker_died"); // Critical
        assert_eq!(notifications[1].event_type, "task_completed"); // Normal

        // Polling again should return empty (already processed)
        let notifications = store.poll("supervisor-1", 10).unwrap();
        assert!(notifications.is_empty());
    }

    #[test]
    fn test_peek_does_not_process() {
        let (_temp, store) = create_test_store();

        store
            .notify(
                "supervisor-1",
                "task_completed",
                "{}",
                NotificationPriority::Normal,
            )
            .unwrap();

        // Peek should return notification
        let notifications = store.peek("supervisor-1", 10).unwrap();
        assert_eq!(notifications.len(), 1);

        // Peek again should still return it (not processed)
        let notifications = store.peek("supervisor-1", 10).unwrap();
        assert_eq!(notifications.len(), 1);

        // Pending count should be 1
        let count = store.pending_count("supervisor-1").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_ack() {
        let (_temp, store) = create_test_store();

        let id = store
            .notify(
                "supervisor-1",
                "task_completed",
                "{}",
                NotificationPriority::Normal,
            )
            .unwrap();

        // Ack the notification
        store.ack(id).unwrap();

        // Should now be processed
        let count = store.pending_count("supervisor-1").unwrap();
        assert_eq!(count, 0);

        // Ack again should fail
        assert!(store.ack(id).is_err());
    }

    #[test]
    fn test_clear() {
        let (_temp, store) = create_test_store();

        store
            .notify("supervisor-1", "event1", "{}", NotificationPriority::Normal)
            .unwrap();
        store
            .notify("supervisor-1", "event2", "{}", NotificationPriority::Normal)
            .unwrap();
        store
            .notify("supervisor-2", "event3", "{}", NotificationPriority::Normal)
            .unwrap();

        // Clear supervisor-1's notifications
        let cleared = store.clear("supervisor-1").unwrap();
        assert_eq!(cleared, 2);

        // supervisor-1 should have no pending
        let count = store.pending_count("supervisor-1").unwrap();
        assert_eq!(count, 0);

        // supervisor-2 should still have 1
        let count = store.pending_count("supervisor-2").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_priority_ordering() {
        let (_temp, store) = create_test_store();

        // Queue in reverse priority order
        store
            .notify("supervisor-1", "normal", "{}", NotificationPriority::Normal)
            .unwrap();
        store
            .notify("supervisor-1", "high", "{}", NotificationPriority::High)
            .unwrap();
        store
            .notify(
                "supervisor-1",
                "critical",
                "{}",
                NotificationPriority::Critical,
            )
            .unwrap();

        // Poll should return in priority order
        let notifications = store.poll("supervisor-1", 10).unwrap();
        assert_eq!(notifications.len(), 3);
        assert_eq!(notifications[0].event_type, "critical");
        assert_eq!(notifications[1].event_type, "high");
        assert_eq!(notifications[2].event_type, "normal");
    }

    #[test]
    fn test_different_supervisors() {
        let (_temp, store) = create_test_store();

        store
            .notify("supervisor-1", "event1", "{}", NotificationPriority::Normal)
            .unwrap();
        store
            .notify("supervisor-2", "event2", "{}", NotificationPriority::Normal)
            .unwrap();

        // Each supervisor should only see their own notifications
        let notifications = store.poll("supervisor-1", 10).unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].event_type, "event1");

        let notifications = store.poll("supervisor-2", 10).unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].event_type, "event2");
    }

    /// cas-88d8: concurrent SqliteSupervisorQueueStore::init on a legacy DB all succeed.
    #[test]
    fn test_concurrent_init_on_legacy_supervisor_queue() {
        use std::sync::{Arc, Barrier};
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().to_path_buf();
        let db_path = cas_dir.join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                PRAGMA journal_mode=WAL;
                CREATE TABLE supervisor_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    supervisor_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    priority INTEGER NOT NULL DEFAULT 2,
                    created_at TEXT NOT NULL,
                    processed_at TEXT
                );
                INSERT INTO supervisor_queue (supervisor_id, event_type, payload, priority, created_at)
                VALUES ('sup', 'worker_died', '{}', 0, '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let barrier = Arc::new(Barrier::new(6));
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let cas_dir = cas_dir.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let store = SqliteSupervisorQueueStore::open(&cas_dir).unwrap();
                    store.init()
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().expect("concurrent init must succeed");
        }
        let store = SqliteSupervisorQueueStore::open(&cas_dir).unwrap();
        store.init().unwrap();
        assert_eq!(store.list_pending("sup").unwrap().len(), 1);
        // New columns usable.
        store
            .notify_idempotent(
                "sup",
                "task_lifecycle",
                "{}",
                NotificationPriority::Normal,
                "k-concurrent",
            )
            .unwrap();
        assert_eq!(store.list_pending_lifecycle_outbox(10).unwrap().len(), 1);
    }

    /// cas-3a47: open a pre-transition_key / pre-prompt_delivered schema, upgrade, keep data.
    #[test]
    fn test_upgrade_from_legacy_supervisor_queue_schema() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            // Minimal pre-cas-062d schema (no transition_key, no prompt_delivered_at).
            conn.execute_batch(
                r#"
                CREATE TABLE supervisor_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    supervisor_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    priority INTEGER NOT NULL DEFAULT 2,
                    created_at TEXT NOT NULL,
                    processed_at TEXT
                );
                INSERT INTO supervisor_queue (supervisor_id, event_type, payload, priority, created_at)
                VALUES ('sup-legacy', 'worker_died', '{"worker":"w1"}', 0, '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let store = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        // Idempotent second init.
        store.init().unwrap();

        let pending = store.list_pending("sup-legacy").unwrap();
        assert_eq!(pending.len(), 1, "legacy row preserved");
        assert_eq!(pending[0].event_type, "worker_died");
        assert!(pending[0].transition_key.is_none());
        assert!(pending[0].prompt_delivered_at.is_none());

        // New lifecycle outbox APIs work on upgraded schema.
        let key = "cas-u:open:in_progress:s:task_started:occ";
        store
            .notify_idempotent(
                "sup-legacy",
                "task_lifecycle",
                r#"{"task_id":"cas-u"}"#,
                NotificationPriority::Normal,
                key,
            )
            .unwrap();
        let outbox = store.list_pending_lifecycle_outbox(10).unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].transition_key.as_deref(), Some(key));
    }
}
