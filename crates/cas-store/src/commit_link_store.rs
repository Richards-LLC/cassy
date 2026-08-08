//! SQLite-based commit link storage for tracking git commits from AI sessions
//!
//! This module provides storage for commit links that enable code attribution:
//! tracing git commits back to the session, agent, and prompts that created them.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{CommitLink, Scope};

/// Helper to convert mutex poison error to StoreError
fn lock_error<T>(_: std::sync::PoisonError<T>) -> StoreError {
    StoreError::Other("lock poisoned".to_string())
}

/// `(files_changed, prompt_ids)` as JSON, shared by every writer so the two
/// insert paths cannot disagree about the encoding.
fn serialize_arrays(link: &CommitLink) -> Result<(String, String)> {
    let files_changed = serde_json::to_string(&link.files_changed)
        .map_err(|e| StoreError::Other(format!("Failed to serialize files_changed: {e}")))?;
    let prompt_ids = serde_json::to_string(&link.prompt_ids)
        .map_err(|e| StoreError::Other(format!("Failed to serialize prompt_ids: {e}")))?;
    Ok((files_changed, prompt_ids))
}

/// Schema for commit_links table (also defined in migration m143; the
/// `link_method` column arrived with m224 — EPIC cas-6212 / cas-519f).
pub const COMMIT_LINK_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS commit_links (
    commit_hash TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    message TEXT NOT NULL,
    files_changed TEXT NOT NULL,
    prompt_ids TEXT NOT NULL,
    committed_at TEXT NOT NULL,
    author TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'project',
    link_method TEXT
);

CREATE INDEX IF NOT EXISTS idx_commit_links_session ON commit_links(session_id);
CREATE INDEX IF NOT EXISTS idx_commit_links_branch ON commit_links(branch);
CREATE INDEX IF NOT EXISTS idx_commit_links_committed ON commit_links(committed_at DESC);
"#;

/// Add `link_method` to a `commit_links` table created before M5.
///
/// Kept as its own constant, and applied by `m224` rather than appended to
/// `m143`: m143 already reports "applied" on every live database, so a
/// statement appended to it would be invisible to exactly the installs that
/// have commit links to describe.
pub const COMMIT_LINK_LINK_METHOD_STATEMENTS: &[&str] =
    &["ALTER TABLE commit_links ADD COLUMN link_method TEXT"];

/// Trait for commit link storage operations
pub trait CommitLinkStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Add a new commit link
    fn add(&self, link: &CommitLink) -> Result<()>;

    /// Write a link the history indexer *reconstructed*, without ever
    /// overwriting one that was observed (EPIC cas-6212 / cas-519f, spec §5.3).
    ///
    /// Returns `true` when a row was written, `false` when one already existed.
    /// [`Self::add`] is `INSERT OR REPLACE`, which is right for an observation
    /// — the hook has better information than whatever is on disk — and exactly
    /// wrong for a reconstruction, which would then be able to demote a
    /// directly-observed link to a guess.
    fn add_reconstructed(&self, link: &CommitLink) -> Result<bool>;

    /// Get a commit link by hash
    fn get(&self, commit_hash: &str) -> Result<Option<CommitLink>>;

    /// Get commit links for a session
    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<CommitLink>>;

    /// Get commit links for a branch
    fn list_by_branch(&self, branch: &str, limit: usize) -> Result<Vec<CommitLink>>;

    /// Get recent commits (most recent first)
    fn list_recent(&self, limit: usize) -> Result<Vec<CommitLink>>;

    /// Find commits that changed a specific file
    fn find_by_file(&self, file_path: &str, limit: usize) -> Result<Vec<CommitLink>>;

    /// Delete old commit links (keep last N days)
    fn prune(&self, days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite implementation of CommitLinkStore
pub struct SqliteCommitLinkStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCommitLinkStore {
    /// Open or create a commit link store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Parse a row into a CommitLink
    fn row_to_commit_link(row: &rusqlite::Row) -> rusqlite::Result<CommitLink> {
        let committed_at_str: String = row.get("committed_at")?;
        let scope_str: String = row.get("scope")?;
        let files_changed_json: String = row.get("files_changed")?;
        let prompt_ids_json: String = row.get("prompt_ids")?;

        // Parse JSON arrays
        let files_changed: Vec<String> =
            serde_json::from_str(&files_changed_json).unwrap_or_default();
        let prompt_ids: Vec<String> = serde_json::from_str(&prompt_ids_json).unwrap_or_default();

        Ok(CommitLink {
            commit_hash: row.get("commit_hash")?,
            session_id: row.get("session_id")?,
            agent_id: row.get("agent_id")?,
            branch: row.get("branch")?,
            message: row.get("message")?,
            files_changed,
            prompt_ids,
            committed_at: parse_datetime(&committed_at_str),
            author: row.get("author")?,
            scope: scope_str.parse().unwrap_or(Scope::Project),
            link_method: row.get("link_method")?,
        })
    }

    /// The column list every read shares. Enumerated in one place because
    /// M5 added a column and four `SELECT`s had to learn about it; a fifth one
    /// added later must not be able to silently miss it.
    const COLUMNS: &'static str = "commit_hash, session_id, agent_id, branch, message, \
         files_changed, prompt_ids, committed_at, author, scope, link_method";
}

impl CommitLinkStore for SqliteCommitLinkStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;
        conn.execute_batch(COMMIT_LINK_SCHEMA)?;
        // `CREATE TABLE IF NOT EXISTS` is a no-op on a database whose
        // commit_links predates M5, so the new column would be missing on every
        // store opened outside the migration runner (hooks, tests, tools). The
        // failure that produces is not a nice one — every read errors on an
        // unknown column — so the column is reconciled here as well.
        let has_method: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('commit_links') WHERE name = 'link_method'")?
            .exists([])?;
        if !has_method {
            for sql in COMMIT_LINK_LINK_METHOD_STATEMENTS {
                conn.execute(sql, [])?;
            }
        }
        Ok(())
    }

    fn add(&self, link: &CommitLink) -> Result<()> {
        let conn = self.conn.lock().map_err(lock_error)?;
        let (files_changed_json, prompt_ids_json) = serialize_arrays(link)?;

        conn.execute(
            "INSERT OR REPLACE INTO commit_links
             (commit_hash, session_id, agent_id, branch, message, files_changed, prompt_ids, committed_at, author, scope, link_method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                link.commit_hash,
                link.session_id,
                link.agent_id,
                link.branch,
                link.message,
                files_changed_json,
                prompt_ids_json,
                link.committed_at.to_rfc3339(),
                link.author,
                link.scope.to_string(),
                link.link_method,
            ],
        )?;

        Ok(())
    }

    fn add_reconstructed(&self, link: &CommitLink) -> Result<bool> {
        let conn = self.conn.lock().map_err(lock_error)?;
        let (files_changed_json, prompt_ids_json) = serialize_arrays(link)?;

        let written = conn.execute(
            "INSERT INTO commit_links
             (commit_hash, session_id, agent_id, branch, message, files_changed, prompt_ids, committed_at, author, scope, link_method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(commit_hash) DO NOTHING",
            params![
                link.commit_hash,
                link.session_id,
                link.agent_id,
                link.branch,
                link.message,
                files_changed_json,
                prompt_ids_json,
                link.committed_at.to_rfc3339(),
                link.author,
                link.scope.to_string(),
                link.link_method,
            ],
        )?;

        Ok(written > 0)
    }

    fn get(&self, commit_hash: &str) -> Result<Option<CommitLink>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            &format!(
                "SELECT {cols} FROM commit_links WHERE commit_hash = ?1",
                cols = Self::COLUMNS
            ),
        )?;

        let link = stmt
            .query_row(params![commit_hash], Self::row_to_commit_link)
            .optional()?;

        Ok(link)
    }

    fn list_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<CommitLink>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            &format!(
                "SELECT {cols} FROM commit_links
                  WHERE session_id = ?1 ORDER BY committed_at DESC LIMIT ?2",
                cols = Self::COLUMNS
            ),
        )?;

        let links = stmt
            .query_map(params![session_id, limit as i64], Self::row_to_commit_link)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(links)
    }

    fn list_by_branch(&self, branch: &str, limit: usize) -> Result<Vec<CommitLink>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            &format!(
                "SELECT {cols} FROM commit_links
                  WHERE branch = ?1 ORDER BY committed_at DESC LIMIT ?2",
                cols = Self::COLUMNS
            ),
        )?;

        let links = stmt
            .query_map(params![branch, limit as i64], Self::row_to_commit_link)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(links)
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<CommitLink>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            &format!(
                "SELECT {cols} FROM commit_links ORDER BY committed_at DESC LIMIT ?1",
                cols = Self::COLUMNS
            ),
        )?;

        let links = stmt
            .query_map(params![limit as i64], Self::row_to_commit_link)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(links)
    }

    fn find_by_file(&self, file_path: &str, limit: usize) -> Result<Vec<CommitLink>> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let mut stmt = conn.prepare_cached(
            &format!(
                "SELECT {cols} FROM commit_links cl, json_each(cl.files_changed) je
                  WHERE je.value = ?1 ORDER BY cl.committed_at DESC LIMIT ?2",
                cols = Self::COLUMNS
                    .split(", ")
                    .map(|c| format!("cl.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )?;

        let links = stmt
            .query_map(params![file_path, limit as i64], Self::row_to_commit_link)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(links)
    }

    fn prune(&self, days: i64) -> Result<usize> {
        let conn = self.conn.lock().map_err(lock_error)?;

        let cutoff = Utc::now() - chrono::Duration::days(days);

        let deleted = conn.execute(
            "DELETE FROM commit_links WHERE committed_at < ?1",
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

/// Helper to add a commit link using the connection from another store
pub fn add_commit_link_with_conn(
    conn: &Connection,
    link: &CommitLink,
) -> std::result::Result<(), rusqlite::Error> {
    let files_changed_json =
        serde_json::to_string(&link.files_changed).unwrap_or_else(|_| "[]".to_string());
    let prompt_ids_json =
        serde_json::to_string(&link.prompt_ids).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT OR REPLACE INTO commit_links
         (commit_hash, session_id, agent_id, branch, message, files_changed, prompt_ids, committed_at, author, scope, link_method)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            link.commit_hash,
            link.session_id,
            link.agent_id,
            link.branch,
            link.message,
            files_changed_json,
            prompt_ids_json,
            link.committed_at.to_rfc3339(),
            link.author,
            link.scope.to_string(),
            link.link_method,
        ],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commit_link_store::*;
    use tempfile::TempDir;

    fn setup_store() -> (SqliteCommitLinkStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = SqliteCommitLinkStore::open(temp_dir.path()).unwrap();
        (store, temp_dir)
    }

    fn create_test_link(commit_hash: &str, session_id: &str, branch: &str) -> CommitLink {
        CommitLink::new(
            commit_hash.to_string(),
            session_id.to_string(),
            "agent-1".to_string(),
            branch.to_string(),
            "Test commit message".to_string(),
            vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            vec!["prompt-1".to_string()],
            "Test Author <test@example.com>".to_string(),
        )
    }

    #[test]
    fn test_add_and_get() {
        let (store, _dir) = setup_store();

        let link = create_test_link("abc123def456", "session-1", "main");
        store.add(&link).unwrap();

        let retrieved = store.get("abc123def456").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.commit_hash, "abc123def456");
        assert_eq!(retrieved.branch, "main");
        assert_eq!(retrieved.files_changed.len(), 2);
        assert_eq!(retrieved.prompt_ids.len(), 1);
    }

    #[test]
    fn test_list_by_session() {
        let (store, _dir) = setup_store();

        store
            .add(&create_test_link("commit-1", "session-A", "main"))
            .unwrap();
        store
            .add(&create_test_link("commit-2", "session-A", "main"))
            .unwrap();
        store
            .add(&create_test_link("commit-3", "session-B", "main"))
            .unwrap();

        let session_a = store.list_by_session("session-A", 10).unwrap();
        assert_eq!(session_a.len(), 2);

        let session_b = store.list_by_session("session-B", 10).unwrap();
        assert_eq!(session_b.len(), 1);
    }

    #[test]
    fn test_list_by_branch() {
        let (store, _dir) = setup_store();

        store
            .add(&create_test_link("commit-1", "session-1", "main"))
            .unwrap();
        store
            .add(&create_test_link("commit-2", "session-1", "feature"))
            .unwrap();
        store
            .add(&create_test_link("commit-3", "session-1", "main"))
            .unwrap();

        let main_commits = store.list_by_branch("main", 10).unwrap();
        assert_eq!(main_commits.len(), 2);

        let feature_commits = store.list_by_branch("feature", 10).unwrap();
        assert_eq!(feature_commits.len(), 1);
    }

    #[test]
    fn test_list_recent() {
        let (store, _dir) = setup_store();

        for i in 0..5 {
            store
                .add(&create_test_link(
                    &format!("commit-{i}"),
                    "session-1",
                    "main",
                ))
                .unwrap();
        }

        let recent = store.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_find_by_file() {
        let (store, _dir) = setup_store();

        let mut link1 = create_test_link("commit-1", "session-1", "main");
        link1.files_changed = vec!["src/main.rs".to_string()];

        let mut link2 = create_test_link("commit-2", "session-1", "main");
        link2.files_changed = vec!["src/lib.rs".to_string()];

        let mut link3 = create_test_link("commit-3", "session-1", "main");
        link3.files_changed = vec!["src/main.rs".to_string(), "tests/test.rs".to_string()];

        store.add(&link1).unwrap();
        store.add(&link2).unwrap();
        store.add(&link3).unwrap();

        let main_commits = store.find_by_file("src/main.rs", 10).unwrap();
        assert_eq!(main_commits.len(), 2);

        let lib_commits = store.find_by_file("src/lib.rs", 10).unwrap();
        assert_eq!(lib_commits.len(), 1);
    }

    #[test]
    fn test_prune() {
        let (store, _dir) = setup_store();

        store
            .add(&create_test_link("commit-1", "session-1", "main"))
            .unwrap();

        // Prune commits older than 30 days (should delete nothing)
        let deleted = store.prune(30).unwrap();
        assert_eq!(deleted, 0);

        let links = store.list_recent(10).unwrap();
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn test_json_arrays_serialization() {
        let (store, _dir) = setup_store();

        let link = CommitLink::new(
            "abc123".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "main".to_string(),
            "Test".to_string(),
            vec![
                "file1.rs".to_string(),
                "file2.rs".to_string(),
                "file3.rs".to_string(),
            ],
            vec!["prompt-1".to_string(), "prompt-2".to_string()],
            "Test <test@test.com>".to_string(),
        );

        store.add(&link).unwrap();

        let retrieved = store.get("abc123").unwrap().unwrap();
        assert_eq!(retrieved.files_changed.len(), 3);
        assert_eq!(retrieved.prompt_ids.len(), 2);
        assert!(retrieved.files_changed.contains(&"file2.rs".to_string()));
        assert!(retrieved.prompt_ids.contains(&"prompt-2".to_string()));
    }
}
