//! SQLite-based event storage for activity tracking
//!
//! This module provides storage for events that track significant actions in CAS,
//! powering the sidecar activity feed.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use crate::write_jsonl_archive;
use cas_types::{Event, EventEntityType, EventType};

/// Helper to convert mutex poison error to StoreError
fn lock_error<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Other("lock poisoned".to_string())
}

/// Schema for events table
pub const EVENT_SCHEMA: &str = r#"
-- Events table: activity log for sidecar feed
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    session_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_entity ON events(entity_type, entity_id);
"#;

/// Trait for event storage operations
pub trait EventStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Record a new event
    fn record(&self, event: &Event) -> Result<i64>;

    /// Get recent events (most recent first)
    fn list_recent(&self, limit: usize) -> Result<Vec<Event>>;

    /// Get events for a specific entity
    fn list_for_entity(
        &self,
        entity_type: EventEntityType,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<Event>>;

    /// Get events by type
    fn list_by_type(&self, event_type: EventType, limit: usize) -> Result<Vec<Event>>;

    /// Get events since a specific timestamp
    fn list_since(&self, since: DateTime<Utc>, limit: usize) -> Result<Vec<Event>>;

    /// Get events for a specific session
    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<Event>>;

    /// Get event count by type (for stats)
    fn count_by_type(&self) -> Result<Vec<(EventType, i64)>>;

    /// Prune old events (keep last N days)
    fn prune(&self, days: i64) -> Result<usize>;

    /// Archive old events to immutable compressed JSONL, then remove them from
    /// the live table.
    fn archive_old(&self, archive_dir: &Path, days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite implementation of EventStore
pub struct SqliteEventStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEventStore {
    /// Open or create an event store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Parse a row into an Event
    fn row_to_event(row: &rusqlite::Row) -> rusqlite::Result<Event> {
        let event_type_str: String = row.get("event_type")?;
        let entity_type_str: String = row.get("entity_type")?;
        let created_str: String = row.get("created_at")?;
        let metadata_str: Option<String> = row.get("metadata")?;

        Ok(Event {
            id: row.get("id")?,
            event_type: event_type_str.parse().unwrap_or(EventType::MemoryStored),
            entity_type: entity_type_str.parse().unwrap_or(EventEntityType::Entry),
            entity_id: row.get("entity_id")?,
            summary: row.get("summary")?,
            metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
            created_at: parse_datetime(&created_str),
            session_id: row.get("session_id")?,
        })
    }
}

impl EventStore for SqliteEventStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;
        conn.execute_batch(EVENT_SCHEMA)?;
        Ok(())
    }

    fn record(&self, event: &Event) -> Result<i64> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let metadata_json = event.metadata.as_ref().map(|m| m.to_string());

        conn.execute(
            "INSERT INTO events (event_type, entity_type, entity_id, summary, metadata, created_at, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.event_type.to_string(),
                event.entity_type.to_string(),
                event.entity_id,
                event.summary,
                metadata_json,
                event.created_at.to_rfc3339(),
                event.session_id,
            ],
        )
        ?;

        Ok(conn.last_insert_rowid())
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Event>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                 FROM events
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )
            ?;

        let events = stmt
            .query_map(params![limit as i64], Self::row_to_event)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    fn list_for_entity(
        &self,
        entity_type: EventEntityType,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                 FROM events
                 WHERE entity_type = ?1 AND entity_id = ?2
                 ORDER BY created_at DESC
                 LIMIT ?3",
            )
            ?;

        let events = stmt
            .query_map(
                params![entity_type.to_string(), entity_id, limit as i64],
                Self::row_to_event,
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    fn list_by_type(&self, event_type: EventType, limit: usize) -> Result<Vec<Event>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                 FROM events
                 WHERE event_type = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            ?;

        let events = stmt
            .query_map(
                params![event_type.to_string(), limit as i64],
                Self::row_to_event,
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    fn list_since(&self, since: DateTime<Utc>, limit: usize) -> Result<Vec<Event>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                 FROM events
                 WHERE created_at >= ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            ?;

        let events = stmt
            .query_map(
                params![since.to_rfc3339(), limit as i64],
                Self::row_to_event,
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<Event>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                 FROM events
                 WHERE session_id = ?1
                 ORDER BY created_at ASC
                 LIMIT ?2",
            )
            ?;

        let events = stmt
            .query_map(params![session_id, limit as i64], Self::row_to_event)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    fn count_by_type(&self) -> Result<Vec<(EventType, i64)>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT event_type, COUNT(*) as count
                 FROM events
                 GROUP BY event_type
                 ORDER BY count DESC",
        )?;

        let counts = stmt
            .query_map([], |row| {
                let type_str: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((type_str, count))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(type_str, count)| type_str.parse::<EventType>().ok().map(|t| (t, count)))
            .collect();

        Ok(counts)
    }

    fn prune(&self, days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let cutoff = Utc::now() - chrono::Duration::days(days);

        let deleted = conn.execute(
            "DELETE FROM events WHERE created_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;

        Ok(deleted)
    }

    fn archive_old(&self, archive_dir: &Path, days: i64) -> Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        let events = {
            let conn = self.conn.lock().map_err(lock_error)?;
            let mut stmt = conn.prepare_cached(
                "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                 FROM events WHERE created_at < ?1 ORDER BY id",
            )?;
            stmt.query_map(params![cutoff.to_rfc3339()], Self::row_to_event)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        if events.is_empty() {
            return Ok(0);
        }

        let ids: Vec<i64> = events.iter().map(|event| event.id).collect();
        write_jsonl_archive(archive_dir, "events", &events)?;

        let conn = self.conn.lock().map_err(lock_error)?;
        let tx = conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM events WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(events.len())
    }

    fn close(&self) -> Result<()> {
        // Connection will be closed when dropped
        Ok(())
    }
}

/// Parse a datetime string, with fallback to current time
fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            // Try ISO format without timezone
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|dt| Utc.from_utc_datetime(&dt))
        })
        .unwrap_or_else(|_| Utc::now())
}

/// Helper to record an event using the connection from another store
pub fn record_event_with_conn(
    conn: &Connection,
    event: &Event,
) -> std::result::Result<i64, rusqlite::Error> {
    let metadata_json = event.metadata.as_ref().map(|m| m.to_string());

    conn.execute(
        "INSERT INTO events (event_type, entity_type, entity_id, summary, metadata, created_at, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event.event_type.to_string(),
            event.entity_type.to_string(),
            event.entity_id,
            event.summary,
            metadata_json,
            event.created_at.to_rfc3339(),
            event.session_id,
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use crate::event_store::*;
    use tempfile::TempDir;

    fn setup_store() -> (SqliteEventStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = SqliteEventStore::open(temp_dir.path()).unwrap();
        (store, temp_dir)
    }

    #[test]
    fn test_record_and_list() {
        let (store, _dir) = setup_store();

        let event = Event::new(
            EventType::TaskStarted,
            EventEntityType::Task,
            "cas-abc1",
            "Task started: Fix the bug",
        );

        let id = store.record(&event).unwrap();
        assert!(id > 0);

        let events = store.list_recent(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity_id, "cas-abc1");
        assert_eq!(events[0].event_type, EventType::TaskStarted);
    }

    #[test]
    fn test_list_by_type() {
        let (store, _dir) = setup_store();

        store
            .record(&Event::new(
                EventType::TaskStarted,
                EventEntityType::Task,
                "cas-1",
                "Started task 1",
            ))
            .unwrap();

        store
            .record(&Event::new(
                EventType::MemoryStored,
                EventEntityType::Entry,
                "entry-1",
                "Stored memory",
            ))
            .unwrap();

        store
            .record(&Event::new(
                EventType::TaskStarted,
                EventEntityType::Task,
                "cas-2",
                "Started task 2",
            ))
            .unwrap();

        let task_events = store.list_by_type(EventType::TaskStarted, 10).unwrap();
        assert_eq!(task_events.len(), 2);

        let memory_events = store.list_by_type(EventType::MemoryStored, 10).unwrap();
        assert_eq!(memory_events.len(), 1);
    }

    #[test]
    fn test_list_for_entity() {
        let (store, _dir) = setup_store();

        store
            .record(&Event::new(
                EventType::TaskStarted,
                EventEntityType::Task,
                "cas-abc",
                "Started",
            ))
            .unwrap();

        store
            .record(&Event::new(
                EventType::TaskCompleted,
                EventEntityType::Task,
                "cas-abc",
                "Completed",
            ))
            .unwrap();

        store
            .record(&Event::new(
                EventType::TaskStarted,
                EventEntityType::Task,
                "cas-def",
                "Started another",
            ))
            .unwrap();

        let events = store
            .list_for_entity(EventEntityType::Task, "cas-abc", 10)
            .unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_count_by_type() {
        let (store, _dir) = setup_store();

        for _ in 0..3 {
            store
                .record(&Event::new(
                    EventType::TaskStarted,
                    EventEntityType::Task,
                    "task",
                    "Started",
                ))
                .unwrap();
        }

        for _ in 0..2 {
            store
                .record(&Event::new(
                    EventType::MemoryStored,
                    EventEntityType::Entry,
                    "entry",
                    "Stored",
                ))
                .unwrap();
        }

        let counts = store.count_by_type().unwrap();

        let task_count = counts.iter().find(|(t, _)| *t == EventType::TaskStarted);
        assert_eq!(task_count.map(|(_, c)| *c), Some(3));

        let memory_count = counts.iter().find(|(t, _)| *t == EventType::MemoryStored);
        assert_eq!(memory_count.map(|(_, c)| *c), Some(2));
    }

    #[test]
    fn test_prune() {
        let (store, _dir) = setup_store();

        // Record an event
        store
            .record(&Event::new(
                EventType::TaskStarted,
                EventEntityType::Task,
                "task",
                "Started",
            ))
            .unwrap();

        // Prune events older than 30 days (should delete nothing)
        let deleted = store.prune(30).unwrap();
        assert_eq!(deleted, 0);

        let events = store.list_recent(10).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_list_by_session() {
        let (store, _dir) = setup_store();

        // Create events with different session IDs
        let mut event1 = Event::new(
            EventType::TaskStarted,
            EventEntityType::Task,
            "cas-1",
            "Started task 1",
        );
        event1.session_id = Some("session-abc".to_string());
        store.record(&event1).unwrap();

        let mut event2 = Event::new(
            EventType::TaskCompleted,
            EventEntityType::Task,
            "cas-1",
            "Completed task 1",
        );
        event2.session_id = Some("session-abc".to_string());
        store.record(&event2).unwrap();

        let mut event3 = Event::new(
            EventType::TaskStarted,
            EventEntityType::Task,
            "cas-2",
            "Started task 2",
        );
        event3.session_id = Some("session-xyz".to_string());
        store.record(&event3).unwrap();

        // Query by session ID
        let abc_events = store.list_by_session("session-abc", 10).unwrap();
        assert_eq!(abc_events.len(), 2);
        assert!(
            abc_events
                .iter()
                .all(|e| e.session_id == Some("session-abc".to_string()))
        );

        let xyz_events = store.list_by_session("session-xyz", 10).unwrap();
        assert_eq!(xyz_events.len(), 1);
        assert_eq!(xyz_events[0].entity_id, "cas-2");

        // Query non-existent session
        let empty_events = store.list_by_session("session-nonexistent", 10).unwrap();
        assert!(empty_events.is_empty());
    }
}
