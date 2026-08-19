//! Durable provider-originated Viktor messages awaiting a Cassy supervisor.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViktorInboundMessage {
    pub message_id: String,
    pub thread_id: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub factory_session: Option<String>,
    pub notification_id: Option<i64>,
    pub last_error: Option<String>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS viktor_inbound_messages (
    message_id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    factory_session TEXT,
    notification_id INTEGER,
    last_error TEXT
);
CREATE INDEX IF NOT EXISTS idx_viktor_inbound_pending
    ON viktor_inbound_messages(delivered_at, created_at);
"#;

pub struct SqliteViktorInboundStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteViktorInboundStore {
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

    /// Persist a provider message before attempting delivery. Returns `true`
    /// only for the first observation of this provider message ID.
    pub fn record(&self, thread_id: &str, message_id: &str, body: &str) -> Result<bool> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        Ok(conn.execute(
            "INSERT OR IGNORE INTO viktor_inbound_messages
                (message_id, thread_id, body, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![message_id, thread_id, body, Utc::now().to_rfc3339()],
        )? > 0)
    }

    pub fn list_pending(&self, limit: usize) -> Result<Vec<ViktorInboundMessage>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut statement = conn.prepare(&format!(
            "SELECT {} FROM viktor_inbound_messages
             WHERE delivered_at IS NULL ORDER BY datetime(created_at), message_id LIMIT ?1",
            Self::COLUMNS
        ))?;
        statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn contains_thread(&self, thread_id: &str) -> Result<bool> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM viktor_inbound_messages WHERE thread_id = ?1)",
            [thread_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn mark_delivery_error(&self, message_id: &str, error: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE viktor_inbound_messages SET last_error = ?2
             WHERE message_id = ?1 AND delivered_at IS NULL",
            params![message_id, error],
        )?;
        Ok(())
    }

    pub fn mark_delivered(
        &self,
        message_id: &str,
        factory_session: Option<&str>,
        notification_id: Option<i64>,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "UPDATE viktor_inbound_messages SET delivered_at = ?2,
                factory_session = ?3, notification_id = ?4, last_error = NULL
             WHERE message_id = ?1 AND delivered_at IS NULL",
            params![
                message_id,
                Utc::now().to_rfc3339(),
                factory_session,
                notification_id
            ],
        )?;
        Ok(())
    }

    /// Atomically claim pending messages for direct SessionStart surfacing.
    /// Every returned row is already receipted, so callers must render all of
    /// them in the hook output.
    pub fn surface_pending(
        &self,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ViktorInboundMessage>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let mut statement = tx.prepare(&format!(
                "SELECT {} FROM viktor_inbound_messages
                 WHERE delivered_at IS NULL ORDER BY datetime(created_at), message_id LIMIT ?1",
                Self::COLUMNS
            ))?;
            let messages = statement
                .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], Self::from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(statement);
            let delivered_at = Utc::now().to_rfc3339();
            for message in &messages {
                tx.execute(
                    "UPDATE viktor_inbound_messages SET delivered_at = ?2,
                        factory_session = ?3, last_error = NULL
                     WHERE message_id = ?1 AND delivered_at IS NULL",
                    params![message.message_id, delivered_at, factory_session],
                )?;
            }
            tx.commit()?;
            Ok(messages)
        })
    }

    pub fn get(&self, message_id: &str) -> Result<Option<ViktorInboundMessage>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            &format!(
                "SELECT {} FROM viktor_inbound_messages WHERE message_id = ?1",
                Self::COLUMNS
            ),
            [message_id],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    const COLUMNS: &str = "message_id, thread_id, body, created_at, delivered_at,
        factory_session, notification_id, last_error";

    fn parse_datetime(value: String) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&value)
            .map(|value| value.with_timezone(&Utc))
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
                    .map(|value| Utc.from_utc_datetime(&value))
            })
            .unwrap_or_else(|_| Utc::now())
    }

    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ViktorInboundMessage> {
        let delivered_at: Option<String> = row.get(4)?;
        Ok(ViktorInboundMessage {
            message_id: row.get(0)?,
            thread_id: row.get(1)?,
            body: row.get(2)?,
            created_at: Self::parse_datetime(row.get(3)?),
            delivered_at: delivered_at.map(Self::parse_datetime),
            factory_session: row.get(5)?,
            notification_id: row.get(6)?,
            last_error: row.get(7)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_messages_are_idempotent_and_surface_once() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteViktorInboundStore::open(temp.path()).unwrap();
        assert!(store.record("thread-1", "message-1", "question").unwrap());
        assert!(!store.record("thread-1", "message-1", "duplicate").unwrap());
        assert!(store.contains_thread("thread-1").unwrap());
        assert!(!store.contains_thread("thread-missing").unwrap());
        store
            .mark_delivery_error("message-1", "no live factory supervisor")
            .unwrap();

        let surfaced = store.surface_pending(Some("factory-1"), 8).unwrap();
        assert_eq!(surfaced.len(), 1);
        assert_eq!(surfaced[0].body, "question");
        assert!(
            store
                .surface_pending(Some("factory-1"), 8)
                .unwrap()
                .is_empty()
        );
        let persisted = store.get("message-1").unwrap().unwrap();
        assert!(persisted.delivered_at.is_some());
        assert_eq!(persisted.factory_session.as_deref(), Some("factory-1"));
        assert!(persisted.last_error.is_none());
    }
}
