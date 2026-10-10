//! SQLite-based prompt storage for tracking user prompts in AI sessions
//!
//! This module provides storage for prompts that enables code attribution:
//! tracing any line of code back to the prompt that triggered its creation.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{Message, Prompt, Scope};

/// Helper to convert mutex poison error to StoreError
fn lock_error<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Other("lock poisoned".to_string())
}

/// Schema for prompts table (also defined in migration m141)
pub const PROMPT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS prompts (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    response_started TEXT,
    task_id TEXT,
    scope TEXT NOT NULL DEFAULT 'project',
    -- Blame v2 fields
    messages_json TEXT,
    model TEXT,
    tool_version TEXT,
    UNIQUE(content_hash, session_id)
);

CREATE INDEX IF NOT EXISTS idx_prompts_session ON prompts(session_id);
CREATE INDEX IF NOT EXISTS idx_prompts_timestamp ON prompts(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_prompts_task ON prompts(task_id);
"#;

/// Trait for prompt storage operations
pub trait PromptStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Add a new prompt
    fn add(&self, prompt: &Prompt) -> Result<()>;

    /// Get a prompt by ID
    fn get(&self, id: &str) -> Result<Option<Prompt>>;

    /// Get prompt by content hash and session (for deduplication)
    fn get_by_hash(&self, content_hash: &str, session_id: &str) -> Result<Option<Prompt>>;

    /// Get prompts for a session
    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<Prompt>>;

    /// Get prompts for a task
    fn list_by_task(&self, task_id: &str, limit: usize) -> Result<Vec<Prompt>>;

    /// Get recent prompts (most recent first)
    fn list_recent(&self, limit: usize) -> Result<Vec<Prompt>>;

    /// Get prompts since a specific timestamp
    fn list_since(&self, since: DateTime<Utc>, limit: usize) -> Result<Vec<Prompt>>;

    /// Update response_started timestamp
    fn mark_response_started(&self, id: &str) -> Result<()>;

    /// Update messages, model, and tool_version for a prompt (blame v2)
    fn update_blame_fields(
        &self,
        id: &str,
        messages: &[Message],
        model: Option<&str>,
        tool_version: Option<&str>,
    ) -> Result<()>;

    /// Delete old prompts (keep last N days)
    fn prune(&self, days: i64) -> Result<usize>;

    /// Null `messages_json` on at most `batch` prompts older than
    /// `older_than_secs`, in one IMMEDIATE transaction (cas-f207). The row,
    /// its content and its provenance keys (id, session, agent, task, hash)
    /// stay; only the session transcript, which no reader consumes, goes.
    /// Returns the rows trimmed.
    fn trim_transcripts_batch(&self, older_than_secs: i64, batch: usize) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite implementation of PromptStore
pub struct SqlitePromptStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqlitePromptStore {
    /// Open or create a prompt store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Parse a row into a Prompt
    fn row_to_prompt(row: &rusqlite::Row) -> rusqlite::Result<Prompt> {
        let timestamp_str: String = row.get("timestamp")?;
        let response_started_str: Option<String> = row.get("response_started")?;
        let scope_str: String = row.get("scope")?;

        // Try to get blame v2 columns (may not exist before migration)
        let messages_json: Option<String> = row.get("messages_json").ok().flatten();
        let model: Option<String> = row.get("model").ok().flatten();
        let tool_version: Option<String> = row.get("tool_version").ok().flatten();

        // Parse messages from JSON if present
        let messages: Vec<Message> = messages_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        Ok(Prompt {
            id: row.get("id")?,
            session_id: row.get("session_id")?,
            agent_id: row.get("agent_id")?,
            content: row.get("content")?,
            content_hash: row.get("content_hash")?,
            timestamp: parse_datetime(&timestamp_str),
            response_started: response_started_str.map(|s| parse_datetime(&s)),
            task_id: row.get("task_id")?,
            scope: scope_str.parse().unwrap_or(Scope::Project),
            messages,
            model,
            tool_version,
        })
    }

    /// Parse a row into a Prompt for queries that include blame v2 columns
    pub fn row_to_prompt_with_blame(row: &rusqlite::Row) -> rusqlite::Result<Prompt> {
        Self::row_to_prompt(row)
    }
}

impl PromptStore for SqlitePromptStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;
        conn.execute_batch(PROMPT_SCHEMA)?;
        Ok(())
    }

    fn add(&self, prompt: &Prompt) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;

        // Serialize messages to JSON
        let messages_json = if prompt.messages.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&prompt.messages)
                    .map_err(|e| StoreError::Other(format!("Failed to serialize messages: {e}")))?,
            )
        };

        conn.execute(
            "INSERT OR REPLACE INTO prompts
             (id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                prompt.id,
                prompt.session_id,
                prompt.agent_id,
                prompt.content,
                prompt.content_hash,
                prompt.timestamp.to_rfc3339(),
                prompt.response_started.map(|dt| dt.to_rfc3339()),
                prompt.task_id,
                prompt.scope.to_string(),
                messages_json,
                prompt.model,
                prompt.tool_version,
            ],
        )?;

        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Prompt>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
             FROM prompts
             WHERE id = ?1",
        )?;

        let prompt = stmt
            .query_row(params![id], Self::row_to_prompt)
            .optional()?;

        Ok(prompt)
    }

    fn get_by_hash(&self, content_hash: &str, session_id: &str) -> Result<Option<Prompt>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
             FROM prompts
             WHERE content_hash = ?1 AND session_id = ?2",
        )?;

        let prompt = stmt
            .query_row(params![content_hash, session_id], Self::row_to_prompt)
            .optional()?;

        Ok(prompt)
    }

    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<Prompt>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
             FROM prompts
             WHERE session_id = ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;

        let prompts = stmt
            .query_map(params![session_id, limit as i64], Self::row_to_prompt)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(prompts)
    }

    fn list_by_task(&self, task_id: &str, limit: usize) -> Result<Vec<Prompt>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
             FROM prompts
             WHERE task_id = ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;

        let prompts = stmt
            .query_map(params![task_id, limit as i64], Self::row_to_prompt)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(prompts)
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Prompt>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
             FROM prompts
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;

        let prompts = stmt
            .query_map(params![limit as i64], Self::row_to_prompt)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(prompts)
    }

    fn list_since(&self, since: DateTime<Utc>, limit: usize) -> Result<Vec<Prompt>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
             FROM prompts
             WHERE timestamp >= ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;

        let prompts = stmt
            .query_map(
                params![since.to_rfc3339(), limit as i64],
                Self::row_to_prompt,
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(prompts)
    }

    fn mark_response_started(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;

        conn.execute(
            "UPDATE prompts SET response_started = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;

        Ok(())
    }

    fn update_blame_fields(
        &self,
        id: &str,
        messages: &[Message],
        model: Option<&str>,
        tool_version: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let messages_json = if messages.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(messages)
                    .map_err(|e| StoreError::Other(format!("Failed to serialize messages: {e}")))?,
            )
        };

        conn.execute(
            "UPDATE prompts SET messages_json = ?1, model = ?2, tool_version = ?3 WHERE id = ?4",
            params![messages_json, model, tool_version, id],
        )?;

        Ok(())
    }

    fn trim_transcripts_batch(&self, older_than_secs: i64, batch: usize) -> Result<usize> {
        if older_than_secs <= 0 {
            return Err(crate::error::StoreError::Other(
                "trim_transcripts_batch requires a positive window".to_string(),
            ));
        }
        let batch = batch.clamp(1, crate::retention::RETENTION_MAX_BATCH);
        let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();
        // Pooled write lock, released between batches by the caller (cas-3f65e).
        crate::shared_db::with_immediate_write_txn_pooled(&self.conn, |tx| {
            // idx_prompts_timestamp bounds the scan to aged rows.
            Ok(tx.execute(
                "UPDATE prompts SET messages_json = NULL
                 WHERE rowid IN (
                     SELECT rowid FROM prompts
                     WHERE timestamp < ?1 AND messages_json IS NOT NULL
                     LIMIT ?2
                 )",
                params![cutoff, batch as i64],
            )?)
        })
    }

    fn prune(&self, days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let cutoff = Utc::now() - chrono::Duration::days(days);

        let deleted = conn.execute(
            "DELETE FROM prompts WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
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

/// Helper to add a prompt using the connection from another store
pub fn add_prompt_with_conn(
    conn: &Connection,
    prompt: &Prompt,
) -> std::result::Result<(), rusqlite::Error> {
    // Serialize messages to JSON
    let messages_json = if prompt.messages.is_empty() {
        None
    } else {
        serde_json::to_string(&prompt.messages).ok()
    };

    conn.execute(
        "INSERT OR REPLACE INTO prompts
         (id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            prompt.id,
            prompt.session_id,
            prompt.agent_id,
            prompt.content,
            prompt.content_hash,
            prompt.timestamp.to_rfc3339(),
            prompt.response_started.map(|dt| dt.to_rfc3339()),
            prompt.task_id,
            prompt.scope.to_string(),
            messages_json,
            prompt.model,
            prompt.tool_version,
        ],
    )?;

    Ok(())
}

/// Helper to get the most recent prompt for a session (for linking to file changes)
pub fn get_current_prompt_for_session(
    conn: &Connection,
    session_id: &str,
) -> std::result::Result<Option<Prompt>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, session_id, agent_id, content, content_hash, timestamp, response_started, task_id, scope, messages_json, model, tool_version
         FROM prompts
         WHERE session_id = ?1
         ORDER BY timestamp DESC
         LIMIT 1",
    )?;

    stmt.query_row(params![session_id], SqlitePromptStore::row_to_prompt)
        .optional()
}

#[cfg(test)]
mod tests {
    use crate::prompt_store::*;
    use tempfile::TempDir;

    fn setup_store() -> (SqlitePromptStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = SqlitePromptStore::open(temp_dir.path()).unwrap();
        (store, temp_dir)
    }

    #[test]
    fn test_add_and_get() {
        let (store, _dir) = setup_store();

        let prompt = Prompt::new(
            "prompt-abc123".to_string(),
            "session-xyz".to_string(),
            "agent-1".to_string(),
            "Add a login button".to_string(),
        );

        store.add(&prompt).unwrap();

        let retrieved = store.get("prompt-abc123").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "prompt-abc123");
        assert_eq!(retrieved.session_id, "session-xyz");
        assert_eq!(retrieved.content, "Add a login button");
    }

    #[test]
    fn test_get_by_hash() {
        let (store, _dir) = setup_store();

        let prompt = Prompt::new(
            "prompt-1".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "Test prompt".to_string(),
        );
        let hash = prompt.content_hash.clone();

        store.add(&prompt).unwrap();

        let retrieved = store.get_by_hash(&hash, "session-1").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "prompt-1");

        // Different session should not find it
        let not_found = store.get_by_hash(&hash, "session-2").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_list_by_session() {
        let (store, _dir) = setup_store();

        // Add prompts for different sessions
        store
            .add(&Prompt::new(
                "prompt-1".to_string(),
                "session-A".to_string(),
                "agent-1".to_string(),
                "First prompt".to_string(),
            ))
            .unwrap();

        store
            .add(&Prompt::new(
                "prompt-2".to_string(),
                "session-A".to_string(),
                "agent-1".to_string(),
                "Second prompt".to_string(),
            ))
            .unwrap();

        store
            .add(&Prompt::new(
                "prompt-3".to_string(),
                "session-B".to_string(),
                "agent-1".to_string(),
                "Other session prompt".to_string(),
            ))
            .unwrap();

        let session_a_prompts = store.list_by_session("session-A", 10).unwrap();
        assert_eq!(session_a_prompts.len(), 2);

        let session_b_prompts = store.list_by_session("session-B", 10).unwrap();
        assert_eq!(session_b_prompts.len(), 1);
    }

    #[test]
    fn test_list_by_task() {
        let (store, _dir) = setup_store();

        store
            .add(&Prompt::with_task(
                "prompt-1".to_string(),
                "session-1".to_string(),
                "agent-1".to_string(),
                "Working on task".to_string(),
                Some("cas-abc".to_string()),
            ))
            .unwrap();

        store
            .add(&Prompt::with_task(
                "prompt-2".to_string(),
                "session-1".to_string(),
                "agent-1".to_string(),
                "Still on task".to_string(),
                Some("cas-abc".to_string()),
            ))
            .unwrap();

        store
            .add(&Prompt::new(
                "prompt-3".to_string(),
                "session-1".to_string(),
                "agent-1".to_string(),
                "No task".to_string(),
            ))
            .unwrap();

        let task_prompts = store.list_by_task("cas-abc", 10).unwrap();
        assert_eq!(task_prompts.len(), 2);
    }

    #[test]
    fn test_list_recent() {
        let (store, _dir) = setup_store();

        for i in 0..5 {
            store
                .add(&Prompt::new(
                    format!("prompt-{i}"),
                    "session-1".to_string(),
                    "agent-1".to_string(),
                    format!("Prompt {i}"),
                ))
                .unwrap();
        }

        let recent = store.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_mark_response_started() {
        let (store, _dir) = setup_store();

        let prompt = Prompt::new(
            "prompt-1".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "Test".to_string(),
        );

        store.add(&prompt).unwrap();

        // Initially no response_started
        let retrieved = store.get("prompt-1").unwrap().unwrap();
        assert!(retrieved.response_started.is_none());

        // Mark response started
        store.mark_response_started("prompt-1").unwrap();

        let retrieved = store.get("prompt-1").unwrap().unwrap();
        assert!(retrieved.response_started.is_some());
    }

    #[test]
    fn test_prune() {
        let (store, _dir) = setup_store();

        // Add a prompt
        store
            .add(&Prompt::new(
                "prompt-1".to_string(),
                "session-1".to_string(),
                "agent-1".to_string(),
                "Test".to_string(),
            ))
            .unwrap();

        // Prune prompts older than 30 days (should delete nothing)
        let deleted = store.prune(30).unwrap();
        assert_eq!(deleted, 0);

        let prompts = store.list_recent(10).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn test_duplicate_hash_same_session_updates() {
        let (store, _dir) = setup_store();

        let prompt1 = Prompt::new(
            "prompt-1".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "Same content".to_string(),
        );

        let prompt2 = Prompt::new(
            "prompt-2".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "Same content".to_string(),
        );

        store.add(&prompt1).unwrap();
        // INSERT OR REPLACE means second insert with same hash+session replaces
        store.add(&prompt2).unwrap();

        // Should have latest prompt
        let prompts = store.list_by_session("session-1", 10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].id, "prompt-2");
    }

    #[test]
    fn test_update_blame_fields() {
        let (store, _dir) = setup_store();

        // Add a prompt
        let prompt = Prompt::new(
            "prompt-1".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "Test prompt".to_string(),
        );
        store.add(&prompt).unwrap();

        // Initially no messages
        let retrieved = store.get("prompt-1").unwrap().unwrap();
        assert!(retrieved.messages.is_empty());
        assert!(retrieved.model.is_none());
        assert!(retrieved.tool_version.is_none());

        // Update with messages
        let messages = vec![
            Message {
                role: cas_types::MessageRole::User,
                content: "Hello".to_string(),
                tool_name: None,
                tool_input: None,
                timestamp: chrono::Utc::now(),
            },
            Message {
                role: cas_types::MessageRole::Assistant,
                content: "Hi there!".to_string(),
                tool_name: None,
                tool_input: None,
                timestamp: chrono::Utc::now(),
            },
        ];

        store
            .update_blame_fields(
                "prompt-1",
                &messages,
                Some("claude-opus-4-5"),
                Some("1.0.0"),
            )
            .unwrap();

        // Verify update
        let updated = store.get("prompt-1").unwrap().unwrap();
        assert_eq!(updated.messages.len(), 2);
        assert_eq!(updated.messages[0].content, "Hello");
        assert_eq!(updated.messages[1].content, "Hi there!");
        assert_eq!(updated.model, Some("claude-opus-4-5".to_string()));
        assert_eq!(updated.tool_version, Some("1.0.0".to_string()));
    }

    /// cas-f207: aged transcripts are nulled in bounded batches; the prompt
    /// row and its provenance survive, and recent transcripts are untouched.
    #[test]
    fn cas_f207_trim_transcripts_is_batched_and_keeps_provenance() {
        let (store, _dir) = setup_store();
        let messages = vec![Message::user("transcript body".to_string())];
        let mut aged_ids = Vec::new();
        for n in 0..5 {
            let mut prompt = Prompt::new(
                format!("aged-{n}"),
                format!("session-{n}"),
                "agent-1".to_string(),
                format!("aged prompt {n}"),
            );
            prompt.timestamp = Utc::now() - chrono::Duration::days(30);
            prompt.task_id = Some("cas-x".to_string());
            store.add(&prompt).unwrap();
            store
                .update_blame_fields(&prompt.id, &messages, Some("model"), Some("v1"))
                .unwrap();
            aged_ids.push(prompt.id);
        }
        let fresh = Prompt::new(
            "fresh".to_string(),
            "session-fresh".to_string(),
            "agent-1".to_string(),
            "fresh prompt".to_string(),
        );
        store.add(&fresh).unwrap();
        store
            .update_blame_fields(&fresh.id, &messages, Some("model"), Some("v1"))
            .unwrap();

        let window = 14 * 24 * 60 * 60;
        let batches: Vec<usize> = (0..4)
            .map(|_| store.trim_transcripts_batch(window, 2).unwrap())
            .collect();
        assert_eq!(batches, vec![2, 2, 1, 0], "each transaction is bounded by the batch");

        for id in &aged_ids {
            let prompt = store.get(id).unwrap().expect("the aged row is kept");
            assert!(prompt.messages.is_empty(), "{id} transcript was not trimmed");
            assert!(prompt.content.starts_with("aged prompt"), "content is provenance");
            assert_eq!(prompt.task_id.as_deref(), Some("cas-x"));
        }
        let kept = store.get("fresh").unwrap().unwrap();
        assert_eq!(kept.messages.len(), 1, "a recent transcript is kept");
        assert!(store.trim_transcripts_batch(0, 10).is_err(), "a zero window is refused");
    }

}
