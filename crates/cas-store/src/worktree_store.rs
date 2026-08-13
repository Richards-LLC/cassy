//! Worktree storage backend
//!
//! Stores worktree records for tracking git worktrees associated with epics.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::StoreError;
use cas_types::{Worktree, WorktreeStatus};

type Result<T> = std::result::Result<T, StoreError>;

/// SQLite DDL for the `worktrees` table (epic worktree tracking).
///
/// **IMPORTANT:** This constant is used ONLY by `SqliteWorktreeStore::init()` —
/// it is DELIBERATELY EXCLUDED from the migration-runner bootstrap
/// (`Subsystem::Worktrees::ensure_base_schema` returns `(None, None)`).
///
/// Rationale: the `worktrees` table has a multi-stage migration history —
/// `m111_worktrees_create_table` creates it with `task_id`, and
/// `m120_worktrees_add_epic_id` later renames `task_id` → `epic_id`. The
/// constant below reflects the post-m120 modern shape (with `epic_id`, no
/// `task_id`). Installing this DDL before the migration ledger runs would
/// cause `m112_worktrees_idx_task` to fail with
/// `no such column: task_id` while creating
/// `idx_worktrees_task ON worktrees(task_id)`.
///
/// If you ever want to add `Worktrees` to the migration-runner bootstrap
/// path, you must FIRST either: (a) shape this constant to match the m111
/// baseline (re-introducing `task_id`), or (b) rewrite the migration chain
/// so it no longer relies on the renamed column.
///
/// See cas-bdb9 / EPIC cas-9fdb.
pub const WORKTREE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS worktrees (
    id TEXT PRIMARY KEY,
    epic_id TEXT,
    branch TEXT NOT NULL,
    parent_branch TEXT NOT NULL,
    path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL,
    merged_at TEXT,
    removed_at TEXT,
    created_by_agent TEXT,
    merge_commit TEXT
);
"#;

/// Worktree storage operations
pub trait WorktreeStore: Send + Sync {
    /// Initialize schema
    fn init(&self) -> Result<()>;

    /// Generate a unique worktree ID
    fn generate_id(&self) -> Result<String>;

    /// Add a new worktree record
    fn add(&self, worktree: &Worktree) -> Result<()>;

    /// Get a worktree by ID
    fn get(&self, id: &str) -> Result<Worktree>;

    /// Get worktree by epic ID (active only)
    fn get_by_epic(&self, epic_id: &str) -> Result<Option<Worktree>>;

    /// Get worktree by branch name
    fn get_by_branch(&self, branch: &str) -> Result<Option<Worktree>>;

    /// Get worktree by path
    fn get_by_path(&self, path: &Path) -> Result<Option<Worktree>>;

    /// Update a worktree record
    fn update(&self, worktree: &Worktree) -> Result<()>;

    /// List all worktrees
    fn list(&self) -> Result<Vec<Worktree>>;

    /// List active worktrees
    fn list_active(&self) -> Result<Vec<Worktree>>;

    /// List worktrees by status
    fn list_by_status(&self, status: WorktreeStatus) -> Result<Vec<Worktree>>;

    /// Delete a worktree record
    fn delete(&self, id: &str) -> Result<()>;

    /// Delete worktrees older than the given number of days
    fn prune(&self, older_than_days: i64) -> Result<usize>;

    /// Close connection
    fn close(&self) -> Result<()>;
}

/// SQLite-based worktree store
pub struct SqliteWorktreeStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteWorktreeStore {
    /// Open or create a SQLite worktree store
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

    // Column order: id, epic_id, branch, parent_branch, path, status,
    //               created_at, merged_at, removed_at, created_by_agent, merge_commit
    fn worktree_from_row(row: &rusqlite::Row) -> rusqlite::Result<Worktree> {
        let status_str: String = row.get(5)?;
        let status = status_str.parse().unwrap_or(WorktreeStatus::Active);

        let created_at_str: String = row.get(6)?;
        let created_at = Self::parse_datetime(&created_at_str).unwrap_or_else(Utc::now);

        let merged_at: Option<DateTime<Utc>> = row
            .get::<_, Option<String>>(7)?
            .and_then(|s| Self::parse_datetime(&s));

        let removed_at: Option<DateTime<Utc>> = row
            .get::<_, Option<String>>(8)?
            .and_then(|s| Self::parse_datetime(&s));

        let path_str: String = row.get(4)?;

        Ok(Worktree {
            id: row.get(0)?,
            epic_id: row.get(1)?,
            branch: row.get(2)?,
            parent_branch: row.get(3)?,
            path: std::path::PathBuf::from(path_str),
            status,
            created_at,
            merged_at,
            removed_at,
            created_by_agent: row.get(9)?,
            merge_commit: row.get(10)?,
        })
    }
}

impl WorktreeStore for SqliteWorktreeStore {
    fn init(&self) -> Result<()> {
        // Ensure schema exists for tests/standalone usage (migrations in production).
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute_batch(WORKTREE_SCHEMA)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        Ok(Worktree::generate_id())
    }

    fn add(&self, worktree: &Worktree) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO worktrees (id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                worktree.id,
                worktree.epic_id,
                worktree.branch,
                worktree.parent_branch,
                worktree.path.to_string_lossy().to_string(),
                worktree.status.to_string(),
                worktree.created_at.to_rfc3339(),
                worktree.merged_at.map(|t| t.to_rfc3339()),
                worktree.removed_at.map(|t| t.to_rfc3339()),
                worktree.created_by_agent,
                worktree.merge_commit,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Worktree> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees WHERE id = ?",
            params![id],
            Self::worktree_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::Other(format!("Worktree not found: {id}")))
    }

    fn get_by_epic(&self, epic_id: &str) -> Result<Option<Worktree>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees WHERE epic_id = ? AND status = 'active'",
            params![epic_id],
            Self::worktree_from_row,
        )
        .optional()
        .map_err(|e| e.into())
    }

    fn get_by_branch(&self, branch: &str) -> Result<Option<Worktree>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees WHERE branch = ?",
            params![branch],
            Self::worktree_from_row,
        )
        .optional()
        .map_err(|e| e.into())
    }

    fn get_by_path(&self, path: &Path) -> Result<Option<Worktree>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let path_str = path.to_string_lossy().to_string();
        conn.query_row(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees WHERE path = ?",
            params![path_str],
            Self::worktree_from_row,
        )
        .optional()
        .map_err(|e| e.into())
    }

    fn update(&self, worktree: &Worktree) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE worktrees SET epic_id = ?1, branch = ?2, parent_branch = ?3,
             path = ?4, status = ?5, merged_at = ?6, removed_at = ?7,
             created_by_agent = ?8, merge_commit = ?9 WHERE id = ?10",
            params![
                worktree.epic_id,
                worktree.branch,
                worktree.parent_branch,
                worktree.path.to_string_lossy().to_string(),
                worktree.status.to_string(),
                worktree.merged_at.map(|t| t.to_rfc3339()),
                worktree.removed_at.map(|t| t.to_rfc3339()),
                worktree.created_by_agent,
                worktree.merge_commit,
                worktree.id,
            ],
        )?;
        if rows == 0 {
            return Err(StoreError::Other(format!(
                "Worktree not found: {}",
                worktree.id
            )));
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<Worktree>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees ORDER BY created_at DESC",
        )?;

        let worktrees = stmt
            .query_map([], Self::worktree_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(worktrees)
    }

    fn list_active(&self) -> Result<Vec<Worktree>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees WHERE status = 'active' ORDER BY created_at DESC",
        )?;

        let worktrees = stmt
            .query_map([], Self::worktree_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(worktrees)
    }

    fn list_by_status(&self, status: WorktreeStatus) -> Result<Vec<Worktree>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, epic_id, branch, parent_branch, path, status,
             created_at, merged_at, removed_at, created_by_agent, merge_commit
             FROM worktrees WHERE status = ? ORDER BY created_at DESC",
        )?;

        let worktrees = stmt
            .query_map(params![status.to_string()], Self::worktree_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(worktrees)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute("DELETE FROM worktrees WHERE id = ?", params![id])?;
        if rows == 0 {
            return Err(StoreError::Other(format!("Worktree not found: {id}")));
        }
        Ok(())
    }

    fn prune(&self, older_than_days: i64) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        let rows = conn.execute(
            "DELETE FROM worktrees WHERE created_at < ?",
            params![cutoff],
        )?;

        Ok(rows)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::worktree_store::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqliteWorktreeStore) {
        let temp = TempDir::new().unwrap();
        let conn = Connection::open(temp.path().join("cas.db")).unwrap();

        // Create worktrees table with epic_id (no task_id)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS worktrees (
                id TEXT PRIMARY KEY,
                epic_id TEXT,
                branch TEXT NOT NULL,
                parent_branch TEXT NOT NULL,
                path TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL,
                merged_at TEXT,
                removed_at TEXT,
                created_by_agent TEXT,
                merge_commit TEXT
            )",
        )
        .unwrap();

        drop(conn);

        let store = SqliteWorktreeStore::open(temp.path()).unwrap();
        (temp, store)
    }

    #[test]
    fn test_worktree_crud() {
        let (_temp, store) = create_test_store();

        // Create worktree for an epic
        let worktree = Worktree::for_epic(
            Worktree::generate_id(),
            "cas-epic-1234".to_string(),
            "cas/cas-epic-1234".to_string(),
            "main".to_string(),
            std::path::PathBuf::from("/tmp/worktree"),
            Some("agent-123".to_string()),
        );

        store.add(&worktree).unwrap();

        // Read
        let fetched = store.get(&worktree.id).unwrap();
        assert_eq!(fetched.epic_id, Some("cas-epic-1234".to_string()));
        assert_eq!(fetched.branch, worktree.branch);

        // Update
        let mut updated = fetched;
        updated.status = WorktreeStatus::Merged;
        updated.merged_at = Some(Utc::now());
        store.update(&updated).unwrap();

        let refetched = store.get(&worktree.id).unwrap();
        assert_eq!(refetched.status, WorktreeStatus::Merged);

        // Delete
        store.delete(&worktree.id).unwrap();
        assert!(store.get(&worktree.id).is_err());
    }

    #[test]
    fn test_get_by_epic() {
        let (_temp, store) = create_test_store();

        let worktree = Worktree::for_epic(
            Worktree::generate_id(),
            "cas-epic-5678".to_string(),
            "cas/cas-epic-5678".to_string(),
            "main".to_string(),
            std::path::PathBuf::from("/tmp/wt1"),
            None,
        );
        store.add(&worktree).unwrap();

        let found = store.get_by_epic("cas-epic-5678").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, worktree.id);

        let not_found = store.get_by_epic("cas-epic-9999").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_list_active() {
        let (_temp, store) = create_test_store();

        let wt1 = Worktree::for_epic(
            Worktree::generate_id(),
            "cas-epic-1".to_string(),
            "cas/cas-epic-1".to_string(),
            "main".to_string(),
            std::path::PathBuf::from("/tmp/wt1"),
            None,
        );
        let mut wt2 = Worktree::for_epic(
            Worktree::generate_id(),
            "cas-epic-2".to_string(),
            "cas/cas-epic-2".to_string(),
            "main".to_string(),
            std::path::PathBuf::from("/tmp/wt2"),
            None,
        );
        wt2.status = WorktreeStatus::Merged;

        store.add(&wt1).unwrap();
        store.add(&wt2).unwrap();

        let active = store.list_active().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, wt1.id);
    }
}
