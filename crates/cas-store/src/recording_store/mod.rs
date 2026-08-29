//! Recording storage backend
//!
//! Stores terminal recording metadata for time-travel playback in factory sessions.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::error::StoreError;
use cas_types::{Recording, RecordingAgent, RecordingEvent, RecordingEventType, RecordingQuery};

type Result<T> = std::result::Result<T, StoreError>;

/// Recording storage operations
pub trait RecordingStore: Send + Sync {
    /// Initialize schema
    fn init(&self) -> Result<()>;

    /// Generate a unique recording ID
    fn generate_id(&self) -> Result<String>;

    /// Add a new recording record
    fn add(&self, recording: &Recording) -> Result<()>;

    /// Get a recording by ID
    fn get(&self, id: &str) -> Result<Recording>;

    /// Update a recording record
    fn update(&self, recording: &Recording) -> Result<()>;

    /// Delete a recording record
    fn delete(&self, id: &str) -> Result<()>;

    /// List all recordings
    fn list(&self) -> Result<Vec<Recording>>;

    /// Query recordings with filters
    fn query(&self, query: &RecordingQuery) -> Result<Vec<Recording>>;

    /// List recordings by session
    fn list_by_session(&self, session_id: &str) -> Result<Vec<Recording>>;

    /// List recordings in a date range
    fn list_by_date_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Recording>>;

    /// List recordings by agent name
    fn list_by_agent(&self, agent_name: &str) -> Result<Vec<Recording>>;

    // Recording agents

    /// Add an agent to a recording
    fn add_agent(&self, agent: &RecordingAgent) -> Result<i64>;

    /// Get agents for a recording
    fn get_agents(&self, recording_id: &str) -> Result<Vec<RecordingAgent>>;

    /// Delete agents for a recording
    fn delete_agents(&self, recording_id: &str) -> Result<()>;

    // Recording events

    /// Add an event to a recording
    fn add_event(&self, event: &RecordingEvent) -> Result<i64>;

    /// Get events for a recording
    fn get_events(&self, recording_id: &str) -> Result<Vec<RecordingEvent>>;

    /// Get events for a recording in a time range
    fn get_events_in_range(
        &self,
        recording_id: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<RecordingEvent>>;

    /// Get events linked to a specific CAS entity
    fn get_events_for_entity(
        &self,
        recording_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<Vec<RecordingEvent>>;

    /// Delete events for a recording
    fn delete_events(&self, recording_id: &str) -> Result<()>;

    // FTS operations

    /// Add content to FTS index
    fn add_fts_content(
        &self,
        recording_id: &str,
        content: &str,
        content_type: &str,
        timestamp_ms: i64,
    ) -> Result<()>;

    /// Search FTS content
    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<(String, i64)>>;

    /// Delete FTS content for a recording
    fn delete_fts_content(&self, recording_id: &str) -> Result<()>;

    /// Delete recordings older than the given number of days (cascades to agents/events)
    fn prune(&self, older_than_days: i64) -> Result<usize>;

    /// Archive old recordings and their entity-correlating children to
    /// immutable compressed JSONL, then remove them from live tables.
    fn archive_old(&self, archive_dir: &Path, older_than_days: i64) -> Result<usize>;

    /// Close connection
    fn close(&self) -> Result<()>;
}

/// SQLite-based recording store
pub struct SqliteRecordingStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRecordingStore {
    /// Open or create a SQLite recording store
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
            return Some(chrono::TimeZone::from_utc_datetime(&Utc, &dt));
        }
        None
    }

    fn recording_from_row(row: &rusqlite::Row) -> rusqlite::Result<Recording> {
        let started_at_str: String = row.get(2)?;
        let started_at = Self::parse_datetime(&started_at_str).unwrap_or_else(Utc::now);

        let ended_at: Option<DateTime<Utc>> = row
            .get::<_, Option<String>>(3)?
            .and_then(|s| Self::parse_datetime(&s));

        let created_at_str: String = row.get(9)?;
        let created_at = Self::parse_datetime(&created_at_str).unwrap_or_else(Utc::now);

        Ok(Recording {
            id: row.get(0)?,
            session_id: row.get(1)?,
            started_at,
            ended_at,
            duration_ms: row.get(4)?,
            file_path: row.get(5)?,
            file_size: row.get(6)?,
            title: row.get(7)?,
            description: row.get(8)?,
            created_at,
        })
    }

    fn recording_agent_from_row(row: &rusqlite::Row) -> rusqlite::Result<RecordingAgent> {
        let created_at_str: String = row.get(5)?;
        let created_at = Self::parse_datetime(&created_at_str).unwrap_or_else(Utc::now);

        Ok(RecordingAgent {
            id: row.get(0)?,
            recording_id: row.get(1)?,
            agent_name: row.get(2)?,
            agent_type: row.get(3)?,
            file_path: row.get(4)?,
            created_at,
        })
    }

    fn recording_event_from_row(row: &rusqlite::Row) -> rusqlite::Result<RecordingEvent> {
        let event_type_str: String = row.get(3)?;
        let event_type = event_type_str.parse().unwrap_or(RecordingEventType::Custom);

        Ok(RecordingEvent {
            id: row.get(0)?,
            recording_id: row.get(1)?,
            timestamp_ms: row.get(2)?,
            event_type,
            entity_type: row.get(4)?,
            entity_id: row.get(5)?,
            metadata: row.get(6)?,
        })
    }
}

mod capture_helpers;
mod store_impl;

pub use capture_helpers::{
    capture_agent_event, capture_memory_event, capture_message_event, capture_recording_event,
    capture_task_event, get_active_recording_with_conn, get_any_active_recording_with_conn,
    record_agent_event_with_conn, record_memory_event_with_conn, record_message_event_with_conn,
    record_recording_event_with_conn, record_task_event_with_conn,
};

#[cfg(test)]
mod tests;
