//! SQLite-based file change storage for tracking AI-generated code modifications
//!
//! This module provides storage for file changes that enables code attribution:
//! tracking which files were modified by which session, agent, and prompt.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{ChangeType, FileChange, Scope};

/// Schema for file_changes table
///
/// Note: existing databases may still have diff/hunks_json/line_attributions_json/
/// human_modified_lines columns from earlier migrations. New inserts leave them empty.
pub const FILE_CHANGE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS file_changes (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    prompt_id TEXT,
    repository TEXT NOT NULL,
    file_path TEXT NOT NULL,
    file_id TEXT,
    change_type TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    old_content_hash TEXT,
    new_content_hash TEXT NOT NULL,
    diff TEXT NOT NULL DEFAULT '',
    hunks_json TEXT NOT NULL DEFAULT '[]',
    commit_hash TEXT,
    committed_at TEXT,
    created_at TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'project',
    line_attributions_json TEXT,
    human_modified_lines TEXT,
    FOREIGN KEY (prompt_id) REFERENCES prompts(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_file_changes_session ON file_changes(session_id);
CREATE INDEX IF NOT EXISTS idx_file_changes_file ON file_changes(repository, file_path);
CREATE INDEX IF NOT EXISTS idx_file_changes_commit ON file_changes(commit_hash) WHERE commit_hash IS NOT NULL;
"#;

/// Trait for file change storage operations
pub trait FileChangeStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Add a new file change
    fn add(&self, change: &FileChange) -> Result<()>;

    /// Get a file change by ID
    fn get(&self, id: &str) -> Result<Option<FileChange>>;

    /// Get file changes for a session
    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<FileChange>>;

    /// Get file changes linked to a specific prompt
    fn list_by_prompt(&self, prompt_id: &str, limit: usize) -> Result<Vec<FileChange>>;

    /// Get file changes for a specific file path
    fn list_by_file(
        &self,
        repository: &str,
        file_path: &str,
        limit: usize,
    ) -> Result<Vec<FileChange>>;

    /// Get uncommitted file changes (commit_hash is NULL)
    fn list_uncommitted(&self, session_id: &str) -> Result<Vec<FileChange>>;

    /// Link file changes to a commit
    fn link_to_commit(&self, ids: &[String], commit_hash: &str) -> Result<usize>;

    /// Get file changes for a specific commit
    fn list_by_commit(&self, commit_hash: &str) -> Result<Vec<FileChange>>;

    /// Get recent file changes (most recent first)
    fn list_recent(&self, limit: usize) -> Result<Vec<FileChange>>;

    /// Delete old file changes (keep last N days)
    fn prune(&self, days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// Select columns used in all queries
const SELECT_COLS: &str = "id, session_id, agent_id, prompt_id, repository, file_path, file_id,
                    change_type, tool_name, old_content_hash, new_content_hash,
                    commit_hash, committed_at, created_at, scope";

/// SQLite implementation of FileChangeStore
pub struct SqliteFileChangeStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteFileChangeStore {
    /// Open or create a file change store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Parse a row into a FileChange
    fn row_to_file_change(row: &rusqlite::Row) -> rusqlite::Result<FileChange> {
        let change_type_str: String = row.get("change_type")?;
        let created_at_str: String = row.get("created_at")?;
        let committed_at_str: Option<String> = row.get("committed_at")?;
        let scope_str: String = row.get("scope")?;

        Ok(FileChange {
            id: row.get("id")?,
            session_id: row.get("session_id")?,
            agent_id: row.get("agent_id")?,
            prompt_id: row.get("prompt_id")?,
            repository: row.get("repository")?,
            file_path: row.get("file_path")?,
            file_id: row.get("file_id")?,
            change_type: change_type_str.parse().unwrap_or(ChangeType::Unknown),
            tool_name: row.get("tool_name")?,
            old_content_hash: row.get("old_content_hash")?,
            new_content_hash: row.get("new_content_hash")?,
            commit_hash: row.get("commit_hash")?,
            committed_at: committed_at_str.map(|s| parse_datetime(&s)),
            created_at: parse_datetime(&created_at_str),
            scope: scope_str.parse().unwrap_or(Scope::Project),
        })
    }
}

impl FileChangeStore for SqliteFileChangeStore {
    fn init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(FILE_CHANGE_SCHEMA)?;
        Ok(())
    }

    fn add(&self, change: &FileChange) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        conn.execute(
            "INSERT OR REPLACE INTO file_changes
             (id, session_id, agent_id, prompt_id, repository, file_path, file_id,
              change_type, tool_name, old_content_hash, new_content_hash, diff, hunks_json,
              commit_hash, committed_at, created_at, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, '', '[]', ?12, ?13, ?14, ?15)",
            params![
                change.id,
                change.session_id,
                change.agent_id,
                change.prompt_id,
                change.repository,
                change.file_path,
                change.file_id,
                change.change_type.to_string(),
                change.tool_name,
                change.old_content_hash,
                change.new_content_hash,
                change.commit_hash,
                change.committed_at.map(|dt| dt.to_rfc3339()),
                change.created_at.to_rfc3339(),
                change.scope.to_string(),
            ],
        )?;

        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes WHERE id = ?1"
        ))?;

        let change = stmt
            .query_row(params![id], Self::row_to_file_change)
            .optional()?;

        Ok(change)
    }

    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes
             WHERE session_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2"
        ))?;

        let changes = stmt
            .query_map(params![session_id, limit as i64], Self::row_to_file_change)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(changes)
    }

    fn list_by_prompt(&self, prompt_id: &str, limit: usize) -> Result<Vec<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes
             WHERE prompt_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2"
        ))?;

        let changes = stmt
            .query_map(params![prompt_id, limit as i64], Self::row_to_file_change)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(changes)
    }

    fn list_by_file(
        &self,
        repository: &str,
        file_path: &str,
        limit: usize,
    ) -> Result<Vec<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes
             WHERE repository = ?1 AND file_path = ?2
             ORDER BY created_at DESC
             LIMIT ?3"
        ))?;

        let changes = stmt
            .query_map(
                params![repository, file_path, limit as i64],
                Self::row_to_file_change,
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(changes)
    }

    fn list_uncommitted(&self, session_id: &str) -> Result<Vec<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes
             WHERE session_id = ?1 AND commit_hash IS NULL
             ORDER BY created_at ASC"
        ))?;

        let changes = stmt
            .query_map(params![session_id], Self::row_to_file_change)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(changes)
    }

    fn link_to_commit(&self, ids: &[String], commit_hash: &str) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let now = Utc::now().to_rfc3339();
        let mut updated = 0;

        for chunk in ids.chunks(500) {
            let placeholders: Vec<String> = chunk.iter().enumerate().map(|(i, _)| format!("?{}", i + 3)).collect();
            let sql = format!(
                "UPDATE file_changes SET commit_hash = ?1, committed_at = ?2 WHERE id IN ({})",
                placeholders.join(", ")
            );

            let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(chunk.len() + 2);
            param_values.push(Box::new(commit_hash.to_string()));
            param_values.push(Box::new(now.clone()));
            for id in chunk {
                param_values.push(Box::new(id.clone()));
            }

            let rows = conn.execute(
                &sql,
                rusqlite::params_from_iter(param_values.iter().map(|p| p.as_ref())),
            )?;
            updated += rows;
        }

        Ok(updated)
    }

    fn list_by_commit(&self, commit_hash: &str) -> Result<Vec<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes
             WHERE commit_hash = ?1
             ORDER BY created_at ASC"
        ))?;

        let changes = stmt
            .query_map(params![commit_hash], Self::row_to_file_change)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(changes)
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<FileChange>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {SELECT_COLS} FROM file_changes
             ORDER BY created_at DESC
             LIMIT ?1"
        ))?;

        let changes = stmt
            .query_map(params![limit as i64], Self::row_to_file_change)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(changes)
    }

    fn prune(&self, days: i64) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let cutoff = Utc::now() - chrono::Duration::days(days);

        let deleted = conn.execute(
            "DELETE FROM file_changes WHERE created_at < ?1",
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

/// Helper to add a file change using the connection from another store
pub fn add_file_change_with_conn(
    conn: &Connection,
    change: &FileChange,
) -> std::result::Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO file_changes
         (id, session_id, agent_id, prompt_id, repository, file_path, file_id,
          change_type, tool_name, old_content_hash, new_content_hash, diff, hunks_json,
          commit_hash, committed_at, created_at, scope)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, '', '[]', ?12, ?13, ?14, ?15)",
        params![
            change.id,
            change.session_id,
            change.agent_id,
            change.prompt_id,
            change.repository,
            change.file_path,
            change.file_id,
            change.change_type.to_string(),
            change.tool_name,
            change.old_content_hash,
            change.new_content_hash,
            change.commit_hash,
            change.committed_at.map(|dt| dt.to_rfc3339()),
            change.created_at.to_rfc3339(),
            change.scope.to_string(),
        ],
    )?;

    Ok(())
}

#[cfg(test)]
#[path = "file_change_store_tests/tests.rs"]
mod tests;
