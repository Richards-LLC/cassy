//! SQLite-based event storage for activity tracking
//!
//! This module provides storage for events that track significant actions in CAS,
//! powering the sidecar activity feed.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Result;
use crate::write_jsonl_archive;
use cas_types::{Event, EventEntityType, EventType};

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

/// Upper bound on rows removed by one retention delete transaction (cas-e193).
///
/// The events table is shared by every agent's writes. A retention pass that
/// deletes hundreds of thousands of rows in one transaction holds the write
/// lock for seconds; capping each transaction keeps every hold short, and the
/// pause between batches lets queued writers in.
pub const EVENT_PRUNE_MAX_BATCH: usize = 1_000;

/// Event types that are operational telemetry, kept only for the
/// `factory.event_telemetry_retention_days` window (cas-e193).
///
/// Every reader of these types looks at a recent window: injection acks scan
/// the newest 25 `supervisor_injected` rows, worker status and the activity
/// feed read minutes to hours, and the director reads the newest 50 rows.
/// Every other type is lifecycle history. Commit provenance
/// (`history_store`, `worker_git_commit`) and task-ownership inference
/// (`foreign_rows`, `migration`, first task event per task) read those
/// unbounded, so telemetry retention never touches them.
pub const TELEMETRY_EVENT_TYPES: &[EventType] = &[
    EventType::SupervisorInjected,
    EventType::SupervisorNotified,
    EventType::AgentHeartbeat,
    EventType::WorkerFileEdited,
    EventType::WorkerSubagentSpawned,
    EventType::WorkerSubagentCompleted,
];

/// Outcome of one bounded telemetry-retention run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventPruneReport {
    /// Rows deleted across all batches.
    pub deleted: usize,
    /// Delete transactions committed.
    pub batches: usize,
    /// Largest single delete transaction (never above [`EVENT_PRUNE_MAX_BATCH`]).
    pub largest_batch: usize,
    /// False when the run stopped at its batch budget with aged rows left.
    pub complete: bool,
}

/// Delete aged telemetry events in bounded batches (cas-e193).
///
/// Each batch is its own short IMMEDIATE transaction of at most
/// [`EVENT_PRUNE_MAX_BATCH`] rows; the store's connection lock is released and
/// the thread sleeps `pause` between batches. At most `max_batches` batches run,
/// so one call is bounded in both rows and wall time; a backlog is reported as
/// `complete == false` and drains on later runs. `days <= 0` disables retention.
pub fn prune_telemetry_events(
    store: &dyn EventStore,
    days: i64,
    batch_size: usize,
    max_batches: usize,
    pause: Duration,
) -> Result<EventPruneReport> {
    let mut report = EventPruneReport {
        complete: true,
        ..Default::default()
    };
    if days <= 0 {
        return Ok(report);
    }
    let batch = batch_size.clamp(1, EVENT_PRUNE_MAX_BATCH);
    loop {
        if report.batches >= max_batches {
            report.complete = false;
            return Ok(report);
        }
        let deleted = store.prune_telemetry_batch(days, batch)?;
        if deleted == 0 {
            return Ok(report);
        }
        report.deleted += deleted;
        report.batches += 1;
        report.largest_batch = report.largest_batch.max(deleted);
        if deleted < batch {
            return Ok(report);
        }
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
}

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

    /// Whether this author recorded the specified activity strictly after `since`.
    /// Query the author and type directly so fleet traffic cannot evict evidence.
    fn has_event_since(
        &self,
        event_type: EventType,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<bool>;

    /// Get event count by type (for stats)
    fn count_by_type(&self) -> Result<Vec<(EventType, i64)>>;

    /// Prune old events (keep last N days)
    fn prune(&self, days: i64) -> Result<usize>;

    /// Archive old events to immutable compressed JSONL, then remove them from
    /// the live table, in batches of at most [`EVENT_PRUNE_MAX_BATCH`] rows.
    fn archive_old(&self, archive_dir: &Path, days: i64) -> Result<usize>;

    /// Delete at most `batch_size` (clamped to [`EVENT_PRUNE_MAX_BATCH`])
    /// [`TELEMETRY_EVENT_TYPES`] rows older than `days`, oldest first, in one
    /// IMMEDIATE transaction. Returns the rows deleted; 0 means none are due.
    fn prune_telemetry_batch(&self, days: i64, batch_size: usize) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite implementation of EventStore
pub struct SqliteEventStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEventStore {

    /// A store on an existing connection, e.g. a
    /// [`crate::shared_db::dedicated_connection`] (cas-ee9ab). The caller owns
    /// schema setup; this never runs DDL.
    pub fn with_connection(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(EVENT_SCHEMA)?;
        Ok(())
    }

    fn record(&self, event: &Event) -> Result<i64> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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

    fn has_event_since(
        &self,
        event_type: EventType,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<bool> {
        // GH #1165: the factory daemon loop calls this; honour its wait budget.
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        Ok(conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM events
             WHERE session_id = ?1 AND event_type = ?2
               AND julianday(created_at) > julianday(?3))",
            params![session_id, event_type.to_string(), since.to_rfc3339()],
            |row| row.get(0),
        )?)
    }

    fn count_by_type(&self) -> Result<Vec<(EventType, i64)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let cutoff = Utc::now() - chrono::Duration::days(days);

        let deleted = conn.execute(
            "DELETE FROM events WHERE created_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;

        Ok(deleted)
    }

    fn archive_old(&self, archive_dir: &Path, days: i64) -> Result<usize> {
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let mut archived = 0usize;
        // Rows are taken in id order above a moving watermark, so each batch
        // reads a fresh slice and a row is never archived twice.
        let mut after_id = 0i64;
        loop {
            let events = {
                let conn = crate::shared_db::lock_connection(&self.conn)?;
                let mut stmt = conn.prepare_cached(
                    "SELECT id, event_type, entity_type, entity_id, summary, metadata, created_at, session_id
                     FROM events WHERE created_at < ?1 AND id > ?2 ORDER BY id LIMIT ?3",
                )?;
                stmt.query_map(
                    params![cutoff, after_id, EVENT_PRUNE_MAX_BATCH as i64],
                    Self::row_to_event,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let Some(last) = events.last() else {
                return Ok(archived);
            };
            after_id = last.id;

            // The archive is written before the rows go, so a failed write
            // leaves them live for the next maintenance cycle.
            write_jsonl_archive(archive_dir, "events", &events)?;
            let ids: Vec<i64> = events.iter().map(|event| event.id).collect();
            {
                let conn = crate::shared_db::lock_connection(&self.conn)?;
                crate::shared_db::with_immediate_write_txn(&conn, |tx| {
                    delete_ids(tx, &ids)?;
                    Ok(())
                })?;
            }
            archived += events.len();
            if events.len() < EVENT_PRUNE_MAX_BATCH {
                return Ok(archived);
            }
        }
    }

    fn prune_telemetry_batch(&self, days: i64, batch_size: usize) -> Result<usize> {
        if days <= 0 {
            return Ok(0);
        }
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let limit = batch_size.clamp(1, EVENT_PRUNE_MAX_BATCH);
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        // Choose the victims with a plain read first: a pass with nothing due
        // never takes the write lock. One type at a time keeps the read on
        // `idx_events_type` in rowid order (oldest first, no sort).
        let mut ids: Vec<i64> = Vec::with_capacity(limit);
        {
            let mut stmt = conn.prepare_cached(
                "SELECT id FROM events WHERE event_type = ?1 AND created_at < ?2
                 ORDER BY id LIMIT ?3",
            )?;
            for event_type in TELEMETRY_EVENT_TYPES {
                let remaining = limit - ids.len();
                if remaining == 0 {
                    break;
                }
                let found = stmt
                    .query_map(
                        params![event_type.to_string(), cutoff, remaining as i64],
                        |row| row.get::<_, i64>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids.extend(found);
            }
        }
        if ids.is_empty() {
            return Ok(0);
        }
        let deleted = crate::shared_db::with_immediate_write_txn(&conn, |tx| delete_ids(tx, &ids))?;
        Ok(deleted)
    }

    fn close(&self) -> Result<()> {
        // Connection will be closed when dropped
        Ok(())
    }
}

/// Delete the given event ids (at most [`EVENT_PRUNE_MAX_BATCH`]) with one
/// statement inside the caller's transaction.
fn delete_ids(conn: &Connection, ids: &[i64]) -> Result<usize> {
    debug_assert!(ids.len() <= EVENT_PRUNE_MAX_BATCH);
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!("DELETE FROM events WHERE id IN ({placeholders})");
    Ok(conn.execute(&sql, rusqlite::params_from_iter(ids.iter()))?)
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

    fn record_aged(store: &SqliteEventStore, event_type: EventType, days_old: i64, entity: &str) {
        let mut event = Event::new(event_type, EventEntityType::Agent, entity, entity);
        event.created_at = Utc::now() - chrono::Duration::days(days_old);
        store.record(&event).unwrap();
    }

    fn count_type(store: &SqliteEventStore, event_type: EventType) -> i64 {
        store
            .count_by_type()
            .unwrap()
            .into_iter()
            .find(|(t, _)| *t == event_type)
            .map(|(_, c)| c)
            .unwrap_or(0)
    }

    /// cas-e193: telemetry retention deletes only aged telemetry types; every
    /// lifecycle type (task, commit, verification, ...) survives at any age
    /// because provenance and task-ownership readers scan them unbounded.
    #[test]
    fn telemetry_retention_prunes_only_aged_telemetry_types_cas_e193() {
        let (store, _dir) = setup_store();
        record_aged(&store, EventType::SupervisorInjected, 20, "old-inject");
        record_aged(&store, EventType::SupervisorInjected, 2, "new-inject");
        record_aged(&store, EventType::WorkerFileEdited, 20, "old-edit");
        record_aged(&store, EventType::AgentHeartbeat, 20, "old-heartbeat");
        record_aged(&store, EventType::WorkerGitCommit, 400, "old-commit");
        record_aged(&store, EventType::TaskCreated, 400, "old-task");

        let report =
            prune_telemetry_events(&store, 14, EVENT_PRUNE_MAX_BATCH, 10, Duration::ZERO).unwrap();
        assert_eq!(report.deleted, 3);
        assert!(report.complete, "{report:?}");
        assert_eq!(count_type(&store, EventType::SupervisorInjected), 1);
        assert_eq!(count_type(&store, EventType::WorkerFileEdited), 0);
        assert_eq!(count_type(&store, EventType::AgentHeartbeat), 0);
        assert_eq!(count_type(&store, EventType::WorkerGitCommit), 1);
        assert_eq!(count_type(&store, EventType::TaskCreated), 1);

        // days = 0 disables retention outright.
        record_aged(&store, EventType::SupervisorInjected, 90, "ancient");
        let report =
            prune_telemetry_events(&store, 0, EVENT_PRUNE_MAX_BATCH, 10, Duration::ZERO).unwrap();
        assert_eq!(report.deleted, 0);
        assert_eq!(count_type(&store, EventType::SupervisorInjected), 2);
    }

    /// cas-e193: one delete transaction never removes more than
    /// EVENT_PRUNE_MAX_BATCH rows, whatever batch size the caller asks for,
    /// and a run stops at its batch budget with the backlog reported.
    #[test]
    fn telemetry_retention_batches_never_exceed_the_bound_cas_e193() {
        let (store, _dir) = setup_store();
        let aged = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        {
            let conn = store.conn.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..2_500 {
                tx.execute(
                    "INSERT INTO events (event_type, entity_type, entity_id, summary, created_at)
                     VALUES ('supervisor_injected', 'agent', ?1, 's', ?2)",
                    params![format!("w{i}"), aged],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }

        // An oversized request is clamped to the bound.
        let first = store.prune_telemetry_batch(14, 50_000).unwrap();
        assert_eq!(first, EVENT_PRUNE_MAX_BATCH);

        // A run with a one-batch budget stops early and says so.
        let partial = prune_telemetry_events(&store, 14, 50_000, 1, Duration::ZERO).unwrap();
        assert_eq!(partial.deleted, EVENT_PRUNE_MAX_BATCH);
        assert_eq!(partial.batches, 1);
        assert!(!partial.complete);

        let rest = prune_telemetry_events(&store, 14, 50_000, 10, Duration::ZERO).unwrap();
        assert_eq!(rest.deleted, 500);
        assert!(rest.largest_batch <= EVENT_PRUNE_MAX_BATCH);
        assert!(rest.complete);
        assert_eq!(count_type(&store, EventType::SupervisorInjected), 0);
    }

    /// cas-e193: archive_old works in bounded batches — each batch is archived
    /// before its rows are deleted, and the run still drains the backlog.
    #[test]
    fn archive_old_archives_and_deletes_in_bounded_batches_cas_e193() {
        let (store, dir) = setup_store();
        let aged = (Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        {
            let conn = store.conn.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..2_100 {
                tx.execute(
                    "INSERT INTO events (event_type, entity_type, entity_id, summary, created_at)
                     VALUES ('task_created', 'task', ?1, 's', ?2)",
                    params![format!("t{i}"), aged],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        record_aged(&store, EventType::TaskCreated, 1, "recent");

        let archive_dir = dir.path().join("archive");
        let archived = store.archive_old(&archive_dir, 30).unwrap();
        assert_eq!(archived, 2_100);
        let files = std::fs::read_dir(&archive_dir).unwrap().count();
        assert_eq!(
            files, 3,
            "2,100 rows archive as three batches of at most 1,000"
        );
        assert_eq!(count_type(&store, EventType::TaskCreated), 1);
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
