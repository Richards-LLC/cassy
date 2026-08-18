//! Durable Viktor run watches for daemon-owned inbound delivery.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{Result, StoreError};

/// A day is long enough for an intentionally slow delegated run while still
/// bounding forgotten watches and provider polling cost.
pub const DEFAULT_VIKTOR_WATCH_TTL_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViktorWatchStatus {
    Pending,
    Delivered,
    Expired,
    Quarantined,
    Undeliverable,
}

impl std::fmt::Display for ViktorWatchStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Expired => "expired",
            Self::Quarantined => "quarantined",
            Self::Undeliverable => "undeliverable",
        })
    }
}

impl std::str::FromStr for ViktorWatchStatus {
    type Err = StoreError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "delivered" => Ok(Self::Delivered),
            "expired" => Ok(Self::Expired),
            "quarantined" => Ok(Self::Quarantined),
            "undeliverable" => Ok(Self::Undeliverable),
            other => Err(StoreError::Parse(format!(
                "unknown Viktor watch status: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViktorThreadWatch {
    pub id: i64,
    pub thread_id: String,
    pub run_id: String,
    pub requesting_agent_id: String,
    pub requesting_agent_name: String,
    pub requesting_agent_role: String,
    pub factory_session: Option<String>,
    pub task_id: Option<String>,
    pub watermark: Option<String>,
    pub status: ViktorWatchStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub next_poll_at: DateTime<Utc>,
    pub last_polled_at: Option<DateTime<Utc>>,
    pub poll_count: u32,
    pub delivered_at: Option<DateTime<Utc>>,
    pub notification_id: Option<i64>,
    pub last_error: Option<String>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS viktor_thread_watches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id TEXT NOT NULL,
    run_id TEXT NOT NULL UNIQUE,
    requesting_agent_id TEXT NOT NULL,
    requesting_agent_name TEXT NOT NULL,
    requesting_agent_role TEXT NOT NULL,
    factory_session TEXT,
    task_id TEXT,
    watermark TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    next_poll_at TEXT NOT NULL,
    last_polled_at TEXT,
    poll_count INTEGER NOT NULL DEFAULT 0,
    delivered_at TEXT,
    notification_id INTEGER,
    last_error TEXT
);
CREATE INDEX IF NOT EXISTS idx_viktor_watches_due
    ON viktor_thread_watches(status, next_poll_at, expires_at);
CREATE INDEX IF NOT EXISTS idx_viktor_watches_task
    ON viktor_thread_watches(task_id, status);
"#;

pub struct SqliteViktorWatchStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteViktorWatchStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = crate::shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        thread_id: &str,
        run_id: &str,
        requesting_agent_id: &str,
        requesting_agent_name: &str,
        requesting_agent_role: &str,
        factory_session: Option<&str>,
        task_id: Option<&str>,
        watermark: Option<&str>,
        ttl_secs: i64,
    ) -> Result<i64> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl_secs.max(1));
        conn.execute(
            "INSERT INTO viktor_thread_watches (
                thread_id, run_id, requesting_agent_id, requesting_agent_name,
                requesting_agent_role, factory_session, task_id, watermark,
                status, created_at, expires_at, next_poll_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?9)
             ON CONFLICT(run_id) DO UPDATE SET
                thread_id = excluded.thread_id,
                requesting_agent_id = excluded.requesting_agent_id,
                requesting_agent_name = excluded.requesting_agent_name,
                requesting_agent_role = excluded.requesting_agent_role,
                factory_session = excluded.factory_session,
                task_id = excluded.task_id,
                watermark = COALESCE(excluded.watermark, viktor_thread_watches.watermark)",
            params![
                thread_id,
                run_id,
                requesting_agent_id,
                requesting_agent_name,
                requesting_agent_role,
                factory_session,
                task_id,
                watermark,
                now.to_rfc3339(),
                expires_at.to_rfc3339(),
            ],
        )?;
        conn.query_row(
            "SELECT id FROM viktor_thread_watches WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn get(&self, id: i64) -> Result<Option<ViktorThreadWatch>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            &format!(
                "SELECT {} FROM viktor_thread_watches WHERE id = ?1",
                Self::COLUMNS
            ),
            [id],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Return a fair, bounded due batch. `next_poll_at` is advanced by each
    /// attempted poll, so a busy fleet cannot let one watch monopolize ticks.
    pub fn list_due(&self, limit: usize) -> Result<Vec<ViktorThreadWatch>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut statement = conn.prepare(&format!(
            "SELECT {} FROM viktor_thread_watches
             WHERE status = 'pending'
               AND datetime(expires_at) > datetime('now')
               AND datetime(next_poll_at) <= datetime('now')
             ORDER BY datetime(next_poll_at), id LIMIT ?1",
            Self::COLUMNS
        ))?;
        statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_live(&self) -> Result<Vec<ViktorThreadWatch>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut statement = conn.prepare(&format!(
            "SELECT {} FROM viktor_thread_watches
             WHERE status = 'pending' AND datetime(expires_at) > datetime('now')
             ORDER BY id",
            Self::COLUMNS
        ))?;
        statement
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn record_poll(
        &self,
        id: i64,
        interval_secs: i64,
        watermark: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now();
        let next = now + chrono::Duration::seconds(interval_secs.max(1));
        conn.execute(
            "UPDATE viktor_thread_watches SET
                last_polled_at = ?2, next_poll_at = ?3,
                poll_count = poll_count + 1,
                watermark = COALESCE(?4, watermark), last_error = ?5
             WHERE id = ?1 AND status = 'pending'",
            params![id, now.to_rfc3339(), next.to_rfc3339(), watermark, error],
        )?;
        Ok(())
    }

    pub fn mark_delivered(&self, id: i64, notification_id: i64) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE viktor_thread_watches SET status = 'delivered', delivered_at = ?2,
             notification_id = ?3, last_error = NULL WHERE id = ?1 AND status = 'pending'",
            params![id, Utc::now().to_rfc3339(), notification_id],
        )?;
        Ok(())
    }

    pub fn mark_undeliverable(&self, id: i64, reason: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE viktor_thread_watches SET status = 'undeliverable', last_error = ?2
             WHERE id = ?1 AND status = 'pending'",
            params![id, reason],
        )?;
        Ok(())
    }

    pub fn expire_stale(&self) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE viktor_thread_watches SET status = 'expired',
             last_error = COALESCE(last_error, 'watch TTL elapsed')
             WHERE status = 'pending' AND datetime(expires_at) <= datetime('now')",
            [],
        )
        .map_err(Into::into)
    }

    pub fn quarantine_for_task(&self, task_id: &str) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE viktor_thread_watches SET status = 'quarantined',
             last_error = 'task closed before inbound delivery'
             WHERE status = 'pending' AND task_id = ?1",
            [task_id],
        )
        .map_err(Into::into)
    }

    const COLUMNS: &str = "id, thread_id, run_id, requesting_agent_id,
        requesting_agent_name, requesting_agent_role, factory_session, task_id,
        watermark, status, created_at, expires_at, next_poll_at, last_polled_at,
        poll_count, delivered_at, notification_id, last_error";

    fn parse_datetime(value: String) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&value)
            .map(|value| value.with_timezone(&Utc))
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
                    .map(|value| Utc.from_utc_datetime(&value))
            })
            .unwrap_or_else(|_| Utc::now())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ViktorThreadWatch> {
        let status: String = row.get(9)?;
        let last_polled_at: Option<String> = row.get(13)?;
        let delivered_at: Option<String> = row.get(15)?;
        Ok(ViktorThreadWatch {
            id: row.get(0)?,
            thread_id: row.get(1)?,
            run_id: row.get(2)?,
            requesting_agent_id: row.get(3)?,
            requesting_agent_name: row.get(4)?,
            requesting_agent_role: row.get(5)?,
            factory_session: row.get(6)?,
            task_id: row.get(7)?,
            watermark: row.get(8)?,
            status: status.parse().unwrap_or(ViktorWatchStatus::Undeliverable),
            created_at: Self::parse_datetime(row.get(10)?),
            expires_at: Self::parse_datetime(row.get(11)?),
            next_poll_at: Self::parse_datetime(row.get(12)?),
            last_polled_at: last_polled_at.map(Self::parse_datetime),
            poll_count: row.get::<_, u32>(14)?,
            delivered_at: delivered_at.map(Self::parse_datetime),
            notification_id: row.get(16)?,
            last_error: row.get(17)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_deduplicates_and_quarantines_task_watches() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteViktorWatchStore::open(temp.path()).unwrap();
        let first = store
            .record(
                "thread-1",
                "run-1",
                "agent-1",
                "worker-1",
                "worker",
                Some("factory-1"),
                Some("cas-1"),
                Some("message-1"),
                DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();
        let duplicate = store
            .record(
                "thread-1",
                "run-1",
                "agent-1",
                "worker-1",
                "worker",
                Some("factory-1"),
                Some("cas-1"),
                None,
                DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();

        assert_eq!(first, duplicate);
        assert_eq!(store.list_due(16).unwrap().len(), 1);
        assert_eq!(store.quarantine_for_task("cas-1").unwrap(), 1);
        assert!(store.list_live().unwrap().is_empty());
        assert_eq!(
            store.get(first).unwrap().unwrap().status,
            ViktorWatchStatus::Quarantined
        );
    }

    #[test]
    fn delivery_is_terminal_and_keeps_prompt_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteViktorWatchStore::open(temp.path()).unwrap();
        let id = store
            .record(
                "thread-2",
                "run-2",
                "agent-2",
                "worker-2",
                "worker",
                None,
                None,
                None,
                DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();
        store.mark_delivered(id, 41).unwrap();
        let watch = store.get(id).unwrap().unwrap();
        assert_eq!(watch.status, ViktorWatchStatus::Delivered);
        assert_eq!(watch.notification_id, Some(41));
        assert!(store.list_due(16).unwrap().is_empty());
    }

    #[test]
    fn stale_and_unroutable_watches_reach_terminal_states() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteViktorWatchStore::open(temp.path()).unwrap();
        let expired = store
            .record(
                "thread-expired",
                "run-expired",
                "agent-3",
                "worker-3",
                "worker",
                None,
                None,
                None,
                DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();
        let unroutable = store
            .record(
                "thread-unroutable",
                "run-unroutable",
                "agent-4",
                "worker-4",
                "worker",
                None,
                None,
                None,
                DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();

        {
            let conn = crate::shared_db::lock_connection(&store.conn).unwrap();
            conn.execute(
                "UPDATE viktor_thread_watches SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
                [expired],
            )
            .unwrap();
        }
        assert_eq!(store.expire_stale().unwrap(), 1);
        store
            .mark_undeliverable(unroutable, "requester and supervisor are gone")
            .unwrap();

        let expired = store.get(expired).unwrap().unwrap();
        assert_eq!(expired.status, ViktorWatchStatus::Expired);
        assert_eq!(expired.last_error.as_deref(), Some("watch TTL elapsed"));
        let unroutable = store.get(unroutable).unwrap().unwrap();
        assert_eq!(unroutable.status, ViktorWatchStatus::Undeliverable);
        assert_eq!(
            unroutable.last_error.as_deref(),
            Some("requester and supervisor are gone")
        );
    }
}
