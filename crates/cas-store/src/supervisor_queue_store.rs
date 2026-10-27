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

/// Contract severity for a supervisor-facing alarm (communication-contract-v2 P2.7).
///
/// This is separate from queue ordering: priority controls delivery order,
/// while severity controls wake and coalescing policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmClass {
    Blocker,
    Action,
    Info,
}

impl AlarmClass {
    /// Whether this class is allowed to wake an idle supervisor.
    pub const fn wakes_idle(self) -> bool {
        matches!(self, Self::Blocker | Self::Action)
    }
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
    AlreadyExists { id: i64, prompt_delivered: bool },
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

    /// cas-26c8: [`Self::notify_idempotent`] for a relay that replaces other
    /// notifications. In ONE transaction it records this row and stamps and
    /// acknowledges every not-yet-injected `folded_event_type` row whose
    /// transition key starts with one of `folded_key_prefixes` followed by
    /// `:` (one per recipient of a folded death), so no crash, failed open or
    /// changed recipient list can leave both the fold and a member to send.
    /// The default does the two steps one after the other.
    fn notify_idempotent_folding(
        &self,
        supervisor_id: &str,
        event_type: &str,
        payload: &str,
        priority: NotificationPriority,
        transition_key: &str,
        folded_event_type: &str,
        folded_key_prefixes: &[String],
    ) -> Result<NotifyIdempotentResult> {
        let result =
            self.notify_idempotent(supervisor_id, event_type, payload, priority, transition_key)?;
        let _ = (folded_event_type, folded_key_prefixes);
        Ok(result)
    }

    /// Mark prompt_queue delivery complete for a durable notification (cas-17e4 outbox).
    /// Idempotent: safe to call again after success.
    fn mark_prompt_delivered(&self, notification_id: i64) -> Result<()>;

    /// Look up a notification by transition_key (repair / tests).
    fn get_by_transition_key(&self, transition_key: &str)
    -> Result<Option<SupervisorNotification>>;

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

    /// Bounded, event-filtered prompts awaiting adoption by a delivery daemon.
    fn list_pending_prompt_outbox(
        &self,
        supervisor_id: &str,
        event_type: &str,
        limit: usize,
    ) -> Result<Vec<SupervisorNotification>>;

    /// cas-ff42: [`Self::list_pending_prompt_outbox`] for rows created in
    /// `[created_after, created_before)`, so old rows a sweep has given up on
    /// can never crowd recent ones out of the limit. The default filters the
    /// unwindowed list.
    fn list_pending_prompt_outbox_between(
        &self,
        supervisor_id: &str,
        event_type: &str,
        created_after: DateTime<Utc>,
        created_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SupervisorNotification>> {
        Ok(self
            .list_pending_prompt_outbox(supervisor_id, event_type, usize::MAX)?
            .into_iter()
            .filter(|row| row.created_at >= created_after && row.created_at < created_before)
            .take(limit)
            .collect())
    }

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

    /// Delete at most `batch` finished notifications created before
    /// `older_than_secs` ago, in one IMMEDIATE transaction (cas-f207).
    ///
    /// Finished means: an outbox row (`transition_key` set) whose prompt was
    /// delivered, a pull row (no key) that was processed, or a
    /// `reminder_fired` record whose prompt_queue row exists (cas-5871).
    /// Pending and
    /// undelivered rows stay. Attention keys stay while their session has a
    /// registered non-shutdown agent; old delivered keys from ended sessions
    /// age out. Missing registry/session evidence stays conservative.
    /// Integration keys stay to prevent replay of an unchanged integration.
    fn prune_finished_batch(&self, older_than_secs: i64, batch: usize) -> Result<usize>;

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

    /// Newest notifications for one factory session at or before `until`,
    /// newest first (cas-8f88 work events): rows addressed to one of
    /// `supervisor_ids`, or whose payload names the session. Read-only; the
    /// caller drops rows at the `until` boundary it already served.
    pub fn list_for_session_until(
        &self,
        factory_session: &str,
        supervisor_ids: &[String],
        until: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<SupervisorNotification>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let addressed = (0..supervisor_ids.len())
            .map(|index| format!("?{}", index + 4))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at,
                    transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE (supervisor_id IN ({addressed})
                    OR (json_valid(payload) AND json_extract(payload, '$.factory_session') = ?1))
               AND (?2 IS NULL OR julianday(created_at) <= julianday(?2))
             ORDER BY julianday(created_at) DESC, id DESC
             LIMIT ?3"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![
            factory_session.to_owned().into(),
            until.map(|at| at.to_rfc3339()).into(),
            (limit as i64).into(),
        ];
        values.extend(supervisor_ids.iter().map(|id| id.clone().into()));
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values), Self::notification_from_row)?
            .filter_map(|row| row.ok())
            .collect();
        Ok(rows)
    }

    fn notification_from_row(row: &rusqlite::Row) -> rusqlite::Result<SupervisorNotification> {
        let processed_at_str: Option<String> = row.get(6)?;
        let processed_at = processed_at_str.and_then(|s| Self::parse_datetime(&s));
        // Column 7 = transition_key (cas-062d); tolerate pre-migration tables.
        let transition_key: Option<String> = row.get(7).unwrap_or(None);
        // Column 8 = prompt_delivered_at (cas-17e4); tolerate pre-migration.
        let prompt_delivered_at_str: Option<String> = row.get(8).unwrap_or(None);
        let prompt_delivered_at = prompt_delivered_at_str.and_then(|s| Self::parse_datetime(&s));

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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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

    fn notify_idempotent_folding(
        &self,
        supervisor_id: &str,
        event_type: &str,
        payload: &str,
        priority: NotificationPriority,
        transition_key: &str,
        folded_event_type: &str,
        folded_key_prefixes: &[String],
    ) -> Result<NotifyIdempotentResult> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let now = Utc::now().to_rfc3339();
            let changed = tx.execute(
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
            let result = if changed > 0 {
                NotifyIdempotentResult::Created(tx.last_insert_rowid())
            } else {
                let (existing_id, prompt_delivered_at): (i64, Option<String>) = tx.query_row(
                    "SELECT id, prompt_delivered_at FROM supervisor_queue WHERE transition_key = ?",
                    params![transition_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                NotifyIdempotentResult::AlreadyExists {
                    id: existing_id,
                    prompt_delivered: prompt_delivered_at.is_some(),
                }
            };
            // cas-ff42 L2: every other recipient of a folded member (a live
            // Director, say) gets its own copy of the fold, so stamping its
            // member rows never leaves it without the group alert. The copy
            // is unstamped and names its recipient (`fold_copy_for`), so the
            // death sweep injects its prompt like the per-worker prompts it
            // replaces.
            for prefix in folded_key_prefixes {
                let member = format!("{prefix}:");
                let others: Vec<String> = {
                    let mut stmt = tx.prepare(
                        "SELECT DISTINCT supervisor_id FROM supervisor_queue
                          WHERE event_type = ?1
                            AND prompt_delivered_at IS NULL
                            AND substr(transition_key, 1, length(?2)) = ?2
                            AND supervisor_id <> ?3",
                    )?;
                    stmt.query_map(params![folded_event_type, member, supervisor_id], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<std::result::Result<_, _>>()?
                };
                for other in others {
                    tx.execute(
                        "INSERT OR IGNORE INTO supervisor_queue
                            (supervisor_id, event_type, payload, priority, created_at,
                             transition_key)
                         VALUES (?1, ?2, json_set(?3, '$.fold_copy_for', ?1), ?4, ?5, ?6)",
                        params![
                            other,
                            event_type,
                            payload,
                            i32::from(priority),
                            now,
                            format!("{transition_key}:{other}")
                        ],
                    )?;
                }
            }
            for prefix in folded_key_prefixes {
                let member = format!("{prefix}:");
                tx.execute(
                    "UPDATE supervisor_queue
                        SET prompt_delivered_at = COALESCE(prompt_delivered_at, ?1),
                            processed_at = COALESCE(processed_at, ?1)
                      WHERE event_type = ?2
                        AND prompt_delivered_at IS NULL
                        AND substr(transition_key, 1, length(?3)) = ?3",
                    params![now, folded_event_type, member],
                )?;
            }
            tx.commit()?;
            Ok(result)
        })
    }

    fn mark_prompt_delivered(&self, notification_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;
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

    fn list_pending_prompt_outbox(
        &self,
        supervisor_id: &str,
        event_type: &str,
        limit: usize,
    ) -> Result<Vec<SupervisorNotification>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at,
                    transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE supervisor_id = ?1 AND event_type = ?2
               AND processed_at IS NULL AND prompt_delivered_at IS NULL
             ORDER BY priority ASC, created_at ASC, id ASC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(
                params![supervisor_id, event_type, limit as i64],
                Self::notification_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn list_pending_prompt_outbox_between(
        &self,
        supervisor_id: &str,
        event_type: &str,
        created_after: DateTime<Utc>,
        created_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SupervisorNotification>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, supervisor_id, event_type, payload, priority, created_at, processed_at,
                    transition_key, prompt_delivered_at
             FROM supervisor_queue
             WHERE supervisor_id = ?1 AND event_type = ?2
               AND processed_at IS NULL AND prompt_delivered_at IS NULL
               AND julianday(created_at) >= julianday(?3)
               AND julianday(created_at) < julianday(?4)
             ORDER BY priority ASC, created_at ASC, id ASC LIMIT ?5",
        )?;
        let rows = stmt
            .query_map(
                params![
                    supervisor_id,
                    event_type,
                    created_after.to_rfc3339(),
                    created_before.to_rfc3339(),
                    limit as i64
                ],
                Self::notification_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn poll(&self, supervisor_id: &str, limit: usize) -> Result<Vec<SupervisorNotification>> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM supervisor_queue WHERE supervisor_id = ? AND processed_at IS NULL",
            params![supervisor_id],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    fn list_pending(&self, supervisor_id: &str) -> Result<Vec<SupervisorNotification>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
            let conn = crate::shared_db::lock_connection(&self.conn)?;

            let rows = conn.execute(
                "DELETE FROM supervisor_queue WHERE supervisor_id = ?",
                params![supervisor_id],
            )?;

            Ok(rows)
        }) // with_write_retry
    }

    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();

            let rows = conn.execute(
                "DELETE FROM supervisor_queue WHERE processed_at IS NOT NULL AND processed_at < ?",
                params![cutoff],
            )?;

            Ok(rows)
        }) // with_write_retry
    }

    fn prune_finished_batch(&self, older_than_secs: i64, batch: usize) -> Result<usize> {
        if older_than_secs <= 0 {
            return Err(crate::error::StoreError::Other(
                "prune_finished_batch requires a positive window".to_string(),
            ));
        }
        let batch = batch.clamp(1, crate::retention::RETENTION_MAX_BATCH);
        let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();
        crate::shared_db::with_immediate_write_txn_pooled(&self.conn, |tx| {
            // A queue-only legacy database has no session registry yet. Do
            // not treat that lack of evidence as proof that a session ended.
            let has_agents: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agents')",
                [],
                |row| row.get(0),
            )?;
            let has_prompt_queue: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'prompt_queue')",
                [],
                |row| row.get(0),
            )?;
            let reminder_delivered = if has_prompt_queue {
                "EXISTS (
                     SELECT 1 FROM prompt_queue
                     WHERE prompt_queue.dedupe_key = 'reminder-fired:'
                         || json_extract(CASE WHEN json_valid(payload) THEN payload ELSE '{}' END, '$.reminder_id')
                 )"
            } else {
                "0"
            };
            let ended_attention = if has_agents {
                "NULLIF(json_extract(CASE WHEN json_valid(payload) THEN payload ELSE '{}' END, '$.factory_session'), '') IS NOT NULL
                 AND NOT EXISTS (
                     SELECT 1 FROM agents
                     WHERE agents.factory_session = json_extract(CASE WHEN json_valid(payload) THEN payload ELSE '{}' END, '$.factory_session')
                       AND agents.status <> 'shutdown'
                 )"
            } else {
                "0"
            };
            // Outbox rows are finished once their prompt is delivered (the
            // outbox re-relays only prompt_delivered_at IS NULL); pull rows once
            // processed. Preserve same-episode dedupe for any live member of
            // the session, including a replacement supervisor after resume.
            Ok(tx.execute(
                &format!("DELETE FROM supervisor_queue
                 WHERE id IN (
                     SELECT id FROM supervisor_queue
                     WHERE created_at < ?1
                       AND (
                           (transition_key IS NULL AND processed_at IS NOT NULL)
                           -- cas-5871: a reminder is delivered through
                           -- prompt_queue; its record here is finished once
                           -- that prompt row exists (keyed reminder-fired:<id>,
                           -- unique-indexed). A record whose prompt enqueue
                           -- failed may be the only delivery and stays.
                           OR (transition_key IS NULL AND event_type = 'reminder_fired'
                               AND ({reminder_delivered}))
                           OR (transition_key IS NOT NULL
                               AND prompt_delivered_at IS NOT NULL
                               AND (transition_key NOT LIKE 'worker-attention:%'
                                    OR ({ended_attention}))
                               AND transition_key NOT LIKE 'integration:%')
                       )
                     ORDER BY id
                     LIMIT ?2
                 )"),
                params![cutoff, batch as i64],
            )?)
        })
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
    fn cas_5c2c_round2_pending_prompt_query_filters_before_limit() {
        let (_temp, store) = create_test_store();
        for _ in 0..40 {
            store
                .notify("operator", "unrelated", "{}", NotificationPriority::High)
                .unwrap();
        }
        let done = store
            .notify(
                "operator",
                "supervisor_notice",
                "{}",
                NotificationPriority::High,
            )
            .unwrap();
        store.ack(done).unwrap();
        let delivered = store
            .notify(
                "operator",
                "supervisor_notice",
                "{}",
                NotificationPriority::High,
            )
            .unwrap();
        store.mark_prompt_delivered(delivered).unwrap();
        store
            .notify(
                "another-owner",
                "supervisor_notice",
                "{}",
                NotificationPriority::High,
            )
            .unwrap();
        let first = store
            .notify(
                "operator",
                "supervisor_notice",
                "{}",
                NotificationPriority::High,
            )
            .unwrap();
        let second = store
            .notify(
                "operator",
                "supervisor_notice",
                "{}",
                NotificationPriority::High,
            )
            .unwrap();
        let rows = store
            .list_pending_prompt_outbox("operator", "supervisor_notice", 1)
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![first]
        );
        assert!(
            store
                .list_pending_prompt_outbox("operator", "supervisor_notice", 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .list_pending_prompt_outbox("operator", "supervisor_notice", 10)
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        store.ack(first).unwrap();
        assert_eq!(
            store
                .list_pending_prompt_outbox("operator", "supervisor_notice", 1)
                .unwrap()[0]
                .id,
            second
        );
    }

    #[test]
    fn ct2_p2_attention_keys_age_out() {
        use crate::{AgentStore, SqliteAgentStore};
        use cas_types::{Agent, AgentStatus};
        let (temp, store) = create_test_store();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        for (id, session, status) in [
            ("live", "live-session", AgentStatus::Active),
            ("ended", "ended-session", AgentStatus::Shutdown),
            ("reused", "new-session", AgentStatus::Active),
        ] {
            let mut agent = Agent::new(id.into(), id.into());
            agent.factory_session = Some(session.into());
            agent.status = status;
            agents.register(&agent).unwrap();
        }
        let insert = |supervisor: &str, session: &str, suffix: &str, delivered: bool| {
            let payload = serde_json::json!({"factory_session": session}).to_string();
            let key = format!("worker-attention:{session}:worker_idle:worker:{suffix}");
            let NotifyIdempotentResult::Created(id) = store
                .notify_idempotent(
                    supervisor,
                    "worker_idle",
                    &payload,
                    NotificationPriority::High,
                    &key,
                )
                .unwrap()
            else {
                panic!("new episode")
            };
            if delivered {
                store.mark_prompt_delivered(id).unwrap();
            }
            id
        };
        let live = insert("live", "live-session", "old", true);
        let ended = insert("ended", "ended-session", "old", true);
        let reused = insert("reused", "old-session", "old", true);
        let missing = insert("removed", "removed-session", "old", true);
        let pending = insert("ended", "ended-session", "pending", false);
        let fresh = insert("ended", "ended-session", "fresh", true);
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE supervisor_queue SET created_at = ?1 WHERE id <> ?2",
                params![
                    (Utc::now() - chrono::Duration::days(30)).to_rfc3339(),
                    fresh
                ],
            )
            .unwrap();
        }
        // Bounded transactions retain active-session dedupe and undelivered
        // outbox state, while terminal/removed/rebound session keys age out.
        let removed: usize = (0..4)
            .map(|_| store.prune_finished_batch(14 * 86400, 1).unwrap())
            .sum();
        assert_eq!(removed, 3);
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM supervisor_queue ORDER BY id")
            .unwrap();
        let actual: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        let mut expected = vec![live, pending, fresh];
        expected.sort();
        assert_eq!(actual, expected);
        assert!(!actual.contains(&ended));
        assert!(!actual.contains(&reused));
        assert!(!actual.contains(&missing));
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
                // pin: a newly inserted deduplicated notification must not claim prior delivery.
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

    /// cas-f207: finished notifications past the window are deleted in
    /// bounded batches; pending, undelivered and recurring-key rows stay.
    #[test]
    fn cas_f207_prune_finished_batch_keeps_pending_and_recurring_keys() {
        let (_temp, store) = create_test_store();
        let created = |key: Option<&str>| -> i64 {
            match key {
                None => store
                    .notify("sup", "worker_died", "{}", NotificationPriority::Normal)
                    .unwrap(),
                Some(key) => match store
                    .notify_idempotent("sup", "task_lifecycle", "{}", NotificationPriority::Normal, key)
                    .unwrap()
                {
                    NotifyIdempotentResult::Created(id) => id,
                    other => panic!("{other:?}"),
                },
            }
        };
        let delivered_lifecycle: Vec<i64> = (0..3)
            .map(|n| created(Some(&format!("cas-x:open:closed:s:task_closed:occ-{n}"))))
            .collect();
        let processed_pull = created(None);
        let pending_pull = created(None);
        let undelivered_outbox = created(Some("cas-y:open:closed:s:task_closed:occ"));
        let recurring = created(Some("worker-attention:s:worker_idle:w1::t"));
        let integration = created(Some("integration:owner:cas-z:sha:Failed"));
        let fresh = created(Some("cas-f:open:closed:s:task_closed:occ"));
        for id in delivered_lifecycle.iter().chain([&recurring, &integration, &fresh]) {
            store.mark_prompt_delivered(*id).unwrap();
        }
        store.ack(processed_pull).unwrap();
        {
            let aged = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE supervisor_queue SET created_at = ?1 WHERE id <> ?2",
                params![aged, fresh],
            )
            .unwrap();
        }

        let window = 14 * 24 * 60 * 60;
        let batches: Vec<usize> = (0..4)
            .map(|_| store.prune_finished_batch(window, 2).unwrap())
            .collect();
        assert_eq!(batches, vec![2, 2, 0, 0], "bounded by the batch, then drained");

        let remaining: Vec<i64> = {
            let conn = store.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT id FROM supervisor_queue ORDER BY id").unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .map(|id| id.unwrap())
                .collect()
        };
        let mut expected = vec![pending_pull, undelivered_outbox, recurring, integration, fresh];
        expected.sort();
        assert_eq!(remaining, expected);
        assert!(store.prune_finished_batch(0, 10).is_err(), "a zero window is refused");
    }


    /// cas-5871: an outbox row is consumed by its prompt delivery, not by
    /// `processed_at`, which stays the supervisor's acknowledgement (`ack`
    /// refuses an already-processed row). Retention finishes a delivered
    /// task_lifecycle row once it ages out, so it does not stay forever.
    #[test]
    fn a_delivered_lifecycle_row_ages_out_and_keeps_its_ack_slot_cas_5871() {
        let (_temp, store) = create_test_store();
        let id = match store
            .notify_idempotent(
                "supervisor-1",
                "task_lifecycle",
                "{}",
                NotificationPriority::Normal,
                "cas-x:open:closed:s:task_closed:1",
            )
            .unwrap()
        {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("{other:?}"),
        };
        store.mark_prompt_delivered(id).unwrap();
        assert!(store.get(id).unwrap().unwrap().processed_at.is_none(), "not acknowledged yet");
        store.ack(id).unwrap();
        {
            let aged = (Utc::now() - chrono::Duration::days(15)).to_rfc3339();
            let conn = store.conn.lock().unwrap();
            conn.execute("UPDATE supervisor_queue SET created_at = ?1", params![aged]).unwrap();
        }
        assert_eq!(store.prune_finished_batch(14 * 24 * 60 * 60, 10).unwrap(), 1);
        assert!(store.get(id).unwrap().is_none());
    }

    /// cas-5871: 2,942 reminder_fired rows since May were never consumed.
    /// The reminder is delivered through prompt_queue, so an aged record whose
    /// prompt row exists is finished. One whose prompt enqueue failed may be
    /// the only delivery and stays, as does any other unconsumed pull row.
    #[test]
    fn an_aged_reminder_record_is_pruned_only_once_its_prompt_exists_cas_5871() {
        let (temp, store) = create_test_store();
        let prompts = crate::SqlitePromptQueueStore::open(temp.path()).unwrap();
        crate::PromptQueueStore::init(&prompts).unwrap();
        let delivered = store
            .notify("supervisor-1", "reminder_fired", r#"{"reminder_id":7}"#, NotificationPriority::Normal)
            .unwrap();
        crate::PromptQueueStore::enqueue_idempotent(
            &prompts,
            "lead-sup",
            "lead-sup",
            "Reminder #7: check CI",
            Some("s"),
            None,
            None,
            "reminder-fired:7",
            None,
        )
        .unwrap();
        let only_delivery = store
            .notify("supervisor-1", "reminder_fired", r#"{"reminder_id":8}"#, NotificationPriority::Normal)
            .unwrap();
        let pull = store
            .notify("supervisor-1", "worker_died", "{}", NotificationPriority::Normal)
            .unwrap();
        {
            let aged = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
            let conn = store.conn.lock().unwrap();
            conn.execute("UPDATE supervisor_queue SET created_at = ?1", params![aged])
                .unwrap();
        }
        assert_eq!(store.prune_finished_batch(14 * 24 * 60 * 60, 10).unwrap(), 1);
        assert!(store.get(delivered).unwrap().is_none(), "its prompt row exists");
        assert!(store.get(only_delivery).unwrap().is_some(), "it may be the only delivery");
        assert!(store.get(pull).unwrap().is_some(), "a pending pull row is never deleted");
    }
    /// cas-26c8 review M1: the folded relay and the stamping of its member
    /// rows are one transaction. Every recipient's member row is stamped and
    /// acknowledged, a lookalike key is untouched, an already-delivered member
    /// keeps its delivery time, and a replay changes nothing.
    #[test]
    fn folding_relay_stamps_its_members_in_one_transaction_cas_26c8() {
        let (_temp, store) = create_test_store();
        let died = |key: &str, recipient: &str| match store
            .notify_idempotent(recipient, "worker_died", "{}", NotificationPriority::Critical, key)
            .unwrap()
        {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("{other:?}"),
        };
        let a1 = died("worker_died:w1:100:sup-a", "sup-a");
        let b1 = died("worker_died:w1:100:sup-b", "sup-b");
        let a2 = died("worker_died:w2:200:sup-a", "sup-a");
        let lookalike = died("worker_died:w10:100:sup-a", "sup-a");
        store.mark_prompt_delivered(a2).unwrap();
        let delivered_at = store.get(a2).unwrap().unwrap().prompt_delivered_at;

        let members = vec!["worker_died:w1:100".to_string(), "worker_died:w2:200".to_string()];
        let fold = |store: &SqliteSupervisorQueueStore| {
            store
                .notify_idempotent_folding(
                    "sup-a",
                    "workers_died_together",
                    "{}",
                    NotificationPriority::Critical,
                    "fold:w1:100",
                    "worker_died",
                    &members,
                )
                .unwrap()
        };
        let folded = fold(&store);
        assert!(matches!(folded, NotifyIdempotentResult::Created(_)), "{folded:?}");
        for id in [a1, b1] {
            let row = store.get(id).unwrap().unwrap();
            // pin: review M1: a folded member is stamped so it is never injected.
            assert!(row.prompt_delivered_at.is_some(), "member {id} is stamped");
            // pin: review M1: a folded member is acknowledged in the same transaction.
            assert!(row.processed_at.is_some(), "member {id} is acknowledged");
        }
        assert_eq!(store.get(a2).unwrap().unwrap().prompt_delivered_at, delivered_at);
        let untouched = store.get(lookalike).unwrap().unwrap();
        assert!(untouched.prompt_delivered_at.is_none() && untouched.processed_at.is_none());
        // cas-ff42 L2: sup-b's member row was folded away, so sup-b holds its
        // own copy of the fold, awaiting its prompt and naming its recipient.
        let copy = store
            .get_by_transition_key("fold:w1:100:sup-b")
            .unwrap()
            .expect("the other recipient keeps the group alert");
        assert_eq!(copy.supervisor_id, "sup-b");
        assert_eq!(copy.event_type, "workers_died_together");
        assert!(copy.prompt_delivered_at.is_none() && copy.processed_at.is_none());
        let copy_payload: serde_json::Value = serde_json::from_str(&copy.payload).unwrap();
        assert_eq!(copy_payload["fold_copy_for"], "sup-b");
        assert!(
            store.get_by_transition_key("fold:w1:100:sup-a").unwrap().is_none(),
            "the fold's own recipient gets no copy"
        );

        let NotifyIdempotentResult::Created(fold_id) = folded else { unreachable!() };
        assert_eq!(
            fold(&store),
            NotifyIdempotentResult::AlreadyExists { id: fold_id, prompt_delivered: false },
            "a replay re-reads the same fold"
        );
    }

    /// cas-ff42: the windowed outbox query keeps aged-out rows from crowding
    /// recent ones out of its limit.
    #[test]
    fn windowed_prompt_outbox_skips_rows_outside_the_window_cas_ff42() {
        let (_temp, store) = create_test_store();
        let mut ids = Vec::new();
        for key in ["old-1", "old-2", "new-1"] {
            let NotifyIdempotentResult::Created(id) = store
                .notify_idempotent("sup", "worker_died", "{}", NotificationPriority::Critical, key)
                .unwrap()
            else {
                unreachable!()
            };
            ids.push(id);
        }
        let aged = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE supervisor_queue SET created_at = ?1 WHERE id IN (?2, ?3)",
                params![aged, ids[0], ids[1]],
            )
            .unwrap();
        }
        let now = Utc::now();
        let recent = store
            .list_pending_prompt_outbox_between(
                "sup",
                "worker_died",
                now - chrono::Duration::minutes(30),
                now + chrono::Duration::seconds(1),
                1,
            )
            .unwrap();
        assert_eq!(recent.iter().map(|row| row.id).collect::<Vec<_>>(), [ids[2]]);
        let aged_out = store
            .list_pending_prompt_outbox_between(
                "sup",
                "worker_died",
                now - chrono::Duration::hours(3),
                now - chrono::Duration::minutes(30),
                10,
            )
            .unwrap();
        assert_eq!(aged_out.len(), 2);
    }

}
