//! SQLite-based storage for recording text search using FTS5
//!
//! This module provides full-text search capabilities for factory recording
//! content, enabling search across terminal output with timestamp context.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;

/// A text snippet from a recording with search context
#[derive(Debug, Clone)]
pub struct RecordingTextEntry {
    /// Unique ID
    pub id: i64,
    /// Recording session ID
    pub recording_id: String,
    /// Agent name that produced this output
    pub agent_name: String,
    /// Timestamp in milliseconds from recording start
    pub timestamp_ms: i64,
    /// The text content (terminal viewport snapshot)
    pub text_content: String,
    /// When this entry was created
    pub created_at: DateTime<Utc>,
}

/// Search result with FTS5 match highlighting
#[derive(Debug, Clone)]
pub struct RecordingSearchResult {
    /// The matching entry
    pub entry: RecordingTextEntry,
    /// BM25 relevance score (lower is better)
    pub score: f64,
    /// Highlighted snippet with match context
    pub snippet: String,
}

/// Trait for recording text storage operations
pub trait RecordingTextStore: Send + Sync {
    /// Initialize the store (create tables if needed)
    fn init(&self) -> Result<()>;

    /// Index text content from a recording snapshot
    fn index_text(
        &self,
        recording_id: &str,
        agent_name: &str,
        timestamp_ms: i64,
        text_content: &str,
    ) -> Result<i64>;

    /// Search recordings using FTS5 full-text search
    fn search(&self, query: &str, limit: usize) -> Result<Vec<RecordingSearchResult>>;

    /// Search within a specific recording
    fn search_recording(
        &self,
        recording_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecordingSearchResult>>;

    /// Get all indexed entries for a recording
    fn list_for_recording(&self, recording_id: &str) -> Result<Vec<RecordingTextEntry>>;

    /// Delete all indexed entries for a recording
    fn delete_for_recording(&self, recording_id: &str) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite implementation of RecordingTextStore
pub struct SqliteRecordingTextStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRecordingTextStore {
    /// Open or create a recording text store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Parse a row into a RecordingTextEntry
    fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<RecordingTextEntry> {
        let created_str: String = row.get("created_at")?;

        Ok(RecordingTextEntry {
            id: row.get("id")?,
            recording_id: row.get("recording_id")?,
            agent_name: row.get("agent_name")?,
            timestamp_ms: row.get("timestamp_ms")?,
            text_content: row.get("text_content")?,
            created_at: parse_datetime(&created_str),
        })
    }
}

impl RecordingTextStore for SqliteRecordingTextStore {
    fn init(&self) -> Result<()> {
        // Tables are created via migration m167_recording_text_fts5
        // This just verifies the tables exist
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        // Check if tables exist (migration should have run)
        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='recording_text'",
            [],
            |row| row.get(0),
        )?;

        if !table_exists {
            return Err(StoreError::Other(
                "recording_text table not found - run migrations first".to_string(),
            ));
        }

        Ok(())
    }

    fn index_text(
        &self,
        recording_id: &str,
        agent_name: &str,
        timestamp_ms: i64,
        text_content: &str,
    ) -> Result<i64> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        conn.execute(
            "INSERT INTO recording_text (recording_id, agent_name, timestamp_ms, text_content)
             VALUES (?1, ?2, ?3, ?4)",
            params![recording_id, agent_name, timestamp_ms, text_content],
        )?;

        Ok(conn.last_insert_rowid())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<RecordingSearchResult>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(
            "SELECT
                rt.id, rt.recording_id, rt.agent_name, rt.timestamp_ms, rt.text_content, rt.created_at,
                bm25(recording_text_fts) as score,
                snippet(recording_text_fts, 0, '>>>>', '<<<<', '...', 64) as snippet
             FROM recording_text_fts
             JOIN recording_text rt ON recording_text_fts.rowid = rt.id
             WHERE recording_text_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;

        let results = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok(RecordingSearchResult {
                    entry: RecordingTextEntry {
                        id: row.get("id")?,
                        recording_id: row.get("recording_id")?,
                        agent_name: row.get("agent_name")?,
                        timestamp_ms: row.get("timestamp_ms")?,
                        text_content: row.get("text_content")?,
                        created_at: parse_datetime(&row.get::<_, String>("created_at")?),
                    },
                    score: row.get("score")?,
                    snippet: row.get("snippet")?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    fn search_recording(
        &self,
        recording_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecordingSearchResult>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(
            "SELECT
                rt.id, rt.recording_id, rt.agent_name, rt.timestamp_ms, rt.text_content, rt.created_at,
                bm25(recording_text_fts) as score,
                snippet(recording_text_fts, 0, '>>>>', '<<<<', '...', 64) as snippet
             FROM recording_text_fts
             JOIN recording_text rt ON recording_text_fts.rowid = rt.id
             WHERE recording_text_fts MATCH ?1 AND rt.recording_id = ?2
             ORDER BY score
             LIMIT ?3",
        )?;

        let results = stmt
            .query_map(params![query, recording_id, limit as i64], |row| {
                Ok(RecordingSearchResult {
                    entry: RecordingTextEntry {
                        id: row.get("id")?,
                        recording_id: row.get("recording_id")?,
                        agent_name: row.get("agent_name")?,
                        timestamp_ms: row.get("timestamp_ms")?,
                        text_content: row.get("text_content")?,
                        created_at: parse_datetime(&row.get::<_, String>("created_at")?),
                    },
                    score: row.get("score")?,
                    snippet: row.get("snippet")?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    fn list_for_recording(&self, recording_id: &str) -> Result<Vec<RecordingTextEntry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, recording_id, agent_name, timestamp_ms, text_content, created_at
             FROM recording_text
             WHERE recording_id = ?1
             ORDER BY timestamp_ms ASC",
        )?;

        let entries = stmt
            .query_map(params![recording_id], Self::row_to_entry)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    fn delete_for_recording(&self, recording_id: &str) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let deleted = conn.execute(
            "DELETE FROM recording_text WHERE recording_id = ?1",
            params![recording_id],
        )?;

        Ok(deleted)
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

/// Format a duration in milliseconds as human-readable time
pub fn format_timestamp(ms: i64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let hours = mins / 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, mins % 60, secs % 60)
    } else {
        format!("{}:{:02}", mins, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use crate::recording_text_store::*;
    use tempfile::TempDir;

    fn setup_store() -> (SqliteRecordingTextStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("cas.db");

        // Create the database with required tables
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS recording_text (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recording_id TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                text_content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS recording_text_fts USING fts5(
                text_content,
                content='recording_text',
                content_rowid='id'
            );
            CREATE TRIGGER IF NOT EXISTS recording_text_ai AFTER INSERT ON recording_text BEGIN
                INSERT INTO recording_text_fts(rowid, text_content) VALUES (new.id, new.text_content);
            END;",
        )
        .unwrap();
        drop(conn);

        let store = SqliteRecordingTextStore::open(temp_dir.path()).unwrap();
        (store, temp_dir)
    }

    #[test]
    fn test_index_and_search() {
        let (store, _dir) = setup_store();

        store
            .index_text(
                "session-123",
                "swift-fox",
                5000,
                "Hello world this is a test of full text search",
            )
            .unwrap();

        store
            .index_text(
                "session-123",
                "swift-fox",
                10000,
                "Another line with different content about Rust programming",
            )
            .unwrap();

        let results = store.search("test search", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].entry.text_content.contains("test"));

        let results = store.search("Rust programming", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].entry.text_content.contains("Rust"));
    }

    #[test]
    fn test_search_recording() {
        let (store, _dir) = setup_store();

        store
            .index_text("session-a", "agent-1", 1000, "content in session A")
            .unwrap();

        store
            .index_text("session-b", "agent-2", 2000, "content in session B")
            .unwrap();

        // Search only in session-a
        let results = store.search_recording("session-a", "content", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.recording_id, "session-a");

        // Search only in session-b
        let results = store.search_recording("session-b", "content", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.recording_id, "session-b");
    }

    #[test]
    fn test_delete_for_recording() {
        let (store, _dir) = setup_store();

        store
            .index_text("session-to-delete", "agent", 1000, "some content")
            .unwrap();

        store
            .index_text("session-to-keep", "agent", 1000, "other content")
            .unwrap();

        let deleted = store.delete_for_recording("session-to-delete").unwrap();
        assert_eq!(deleted, 1);

        let entries = store.list_for_recording("session-to-delete").unwrap();
        assert!(entries.is_empty());

        let entries = store.list_for_recording("session-to-keep").unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_format_timestamp() {
        assert_eq!(format_timestamp(0), "0:00");
        assert_eq!(format_timestamp(5000), "0:05");
        assert_eq!(format_timestamp(65000), "1:05");
        assert_eq!(format_timestamp(3661000), "1:01:01");
    }
}
