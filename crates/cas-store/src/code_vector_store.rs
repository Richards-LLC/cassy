//! Durable queue and coverage ledger for semantic source-code vectors.
//!
//! The vectors themselves live in their own LMDB environment. SQLite only
//! records which symbol content hash that environment contains, plus the last
//! full-tree reconciliation receipt. Keeping this state outside `entries` and
//! `knowledge_pages` is the boundary that prevents source code from entering
//! memory/task/knowledge ranking.

use std::path::Path;
use std::sync::{Arc, Mutex};

use cas_code::CodeSymbol;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, StoreError};

pub const CODE_VECTOR_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS code_vector_queue (
    symbol_id TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'vectorized', 'failed')),
    last_error TEXT,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_code_vector_queue_status
    ON code_vector_queue(status, updated_at);
CREATE TABLE IF NOT EXISTS code_index_state (
    repository TEXT PRIMARY KEY,
    eligible_files INTEGER NOT NULL DEFAULT 0,
    indexed_files INTEGER NOT NULL DEFAULT 0,
    failed_files INTEGER NOT NULL DEFAULT 0,
    last_head TEXT,
    last_scan_at TEXT NOT NULL,
    last_error TEXT
);
"#;

pub const CODE_VECTOR_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS code_vector_queue (
        symbol_id TEXT PRIMARY KEY,
        content_hash TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending'
            CHECK(status IN ('pending', 'vectorized', 'failed')),
        last_error TEXT,
        updated_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_code_vector_queue_status
        ON code_vector_queue(status, updated_at)",
    "CREATE TABLE IF NOT EXISTS code_index_state (
        repository TEXT PRIMARY KEY,
        eligible_files INTEGER NOT NULL DEFAULT 0,
        indexed_files INTEGER NOT NULL DEFAULT 0,
        failed_files INTEGER NOT NULL DEFAULT 0,
        last_head TEXT,
        last_scan_at TEXT NOT NULL,
        last_error TEXT
    )",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeVectorWork {
    pub symbol_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeVectorStats {
    pub eligible: usize,
    pub vectorized: usize,
    pub pending: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeIndexState {
    pub repository: String,
    pub eligible_files: usize,
    pub indexed_files: usize,
    pub failed_files: usize,
    pub last_head: Option<String>,
    pub last_scan_at: String,
    pub last_error: Option<String>,
}

pub struct SqliteCodeVectorStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCodeVectorStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = crate::shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        conn.lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?
            .execute_batch(CODE_VECTOR_SCHEMA)?;
        Ok(Self { conn })
    }

    /// Reconcile one parsed file's semantic queue.
    ///
    /// An unchanged `(symbol_id, content_hash)` preserves `vectorized`; a
    /// changed hash is re-armed. Symbols that disappeared or became a
    /// low-value kind are removed and returned so their LMDB keys can be
    /// retired in the same indexing pass.
    pub fn sync_file_symbols(
        &self,
        current: &[CodeSymbol],
        previous_ids: &[String],
    ) -> Result<Vec<String>> {
        let eligible: Vec<&CodeSymbol> = current
            .iter()
            .filter(|symbol| symbol.kind.should_embed())
            .collect();
        let eligible_ids: std::collections::HashSet<&str> =
            eligible.iter().map(|symbol| symbol.id.as_str()).collect();
        let retired: Vec<String> = previous_ids
            .iter()
            .filter(|id| !eligible_ids.contains(id.as_str()))
            .cloned()
            .collect();

        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let tx = conn.transaction()?;
        for id in &retired {
            tx.execute(
                "DELETE FROM code_vector_queue WHERE symbol_id = ?1",
                params![id],
            )?;
        }
        let now = Utc::now().to_rfc3339();
        for symbol in eligible {
            tx.execute(
                "INSERT INTO code_vector_queue
                     (symbol_id, content_hash, status, last_error, updated_at)
                 VALUES (?1, ?2, 'pending', NULL, ?3)
                 ON CONFLICT(symbol_id) DO UPDATE SET
                     content_hash = excluded.content_hash,
                     status = CASE
                         WHEN code_vector_queue.content_hash = excluded.content_hash
                         THEN code_vector_queue.status ELSE 'pending' END,
                     last_error = CASE
                         WHEN code_vector_queue.content_hash = excluded.content_hash
                         THEN code_vector_queue.last_error ELSE NULL END,
                     updated_at = CASE
                         WHEN code_vector_queue.content_hash = excluded.content_hash
                         THEN code_vector_queue.updated_at ELSE excluded.updated_at END",
                params![symbol.id, symbol.content_hash, now],
            )?;
        }
        tx.commit()?;
        Ok(retired)
    }

    pub fn retire(&self, symbol_ids: &[String]) -> Result<()> {
        if symbol_ids.is_empty() {
            return Ok(());
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let tx = conn.transaction()?;
        for id in symbol_ids {
            tx.execute(
                "DELETE FROM code_vector_queue WHERE symbol_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_pending(&self, limit: usize) -> Result<Vec<CodeVectorWork>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT symbol_id, content_hash FROM code_vector_queue
             WHERE status IN ('pending', 'failed')
             ORDER BY updated_at, symbol_id LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(CodeVectorWork {
                symbol_id: row.get(0)?,
                content_hash: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn mark_vectorized(&self, symbol_id: &str, content_hash: &str) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        Ok(conn.execute(
            "UPDATE code_vector_queue
             SET status = 'vectorized', last_error = NULL, updated_at = ?3
             WHERE symbol_id = ?1 AND content_hash = ?2",
            params![symbol_id, content_hash, Utc::now().to_rfc3339()],
        )? == 1)
    }

    pub fn mark_failed(&self, symbol_id: &str, content_hash: &str, error: &str) -> Result<bool> {
        let conn = self.lock()?;
        Ok(conn.execute(
            "UPDATE code_vector_queue
             SET status = 'failed', last_error = ?3, updated_at = ?4
             WHERE symbol_id = ?1 AND content_hash = ?2",
            params![symbol_id, content_hash, error, Utc::now().to_rfc3339()],
        )? == 1)
    }

    pub fn mark_all_pending(&self) -> Result<usize> {
        let conn = self.lock()?;
        Ok(conn.execute(
            "UPDATE code_vector_queue
             SET status = 'pending', last_error = NULL, updated_at = ?1
             WHERE status != 'pending'",
            params![Utc::now().to_rfc3339()],
        )?)
    }

    pub fn stats(&self) -> Result<CodeVectorStats> {
        let conn = self.lock()?;
        let mut stats = CodeVectorStats::default();
        let mut stmt =
            conn.prepare_cached("SELECT status, COUNT(*) FROM code_vector_queue GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (status, count) = row?;
            let count = count.max(0) as usize;
            stats.eligible += count;
            match status.as_str() {
                "vectorized" => stats.vectorized = count,
                "failed" => stats.failed = count,
                _ => stats.pending = count,
            }
        }
        Ok(stats)
    }

    pub fn record_scan(
        &self,
        repository: &str,
        eligible_files: usize,
        indexed_files: usize,
        failed_files: usize,
        last_head: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO code_index_state
                 (repository, eligible_files, indexed_files, failed_files,
                  last_head, last_scan_at, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(repository) DO UPDATE SET
                 eligible_files = excluded.eligible_files,
                 indexed_files = excluded.indexed_files,
                 failed_files = excluded.failed_files,
                 last_head = excluded.last_head,
                 last_scan_at = excluded.last_scan_at,
                 last_error = excluded.last_error",
            params![
                repository,
                eligible_files as i64,
                indexed_files as i64,
                failed_files as i64,
                last_head,
                Utc::now().to_rfc3339(),
                last_error,
            ],
        )?;
        Ok(())
    }

    pub fn index_state(&self, repository: &str) -> Result<Option<CodeIndexState>> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT repository, eligible_files, indexed_files, failed_files,
                    last_head, last_scan_at, last_error
             FROM code_index_state WHERE repository = ?1",
            params![repository],
            |row| {
                Ok(CodeIndexState {
                    repository: row.get(0)?,
                    eligible_files: row.get::<_, i64>(1)?.max(0) as usize,
                    indexed_files: row.get::<_, i64>(2)?.max(0) as usize,
                    failed_files: row.get::<_, i64>(3)?.max(0) as usize,
                    last_head: row.get(4)?,
                    last_scan_at: row.get(5)?,
                    last_error: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_code::{Language, SymbolKind};

    fn symbol(id: &str, hash: &str, kind: SymbolKind) -> CodeSymbol {
        CodeSymbol {
            id: id.into(),
            qualified_name: format!("repo::{id}"),
            name: id.into(),
            kind,
            language: Language::Rust,
            file_path: "src/lib.rs".into(),
            file_id: "file-1".into(),
            line_start: 1,
            line_end: 2,
            source: "fn f() {}".into(),
            documentation: None,
            signature: None,
            parent_id: None,
            repository: "repo".into(),
            commit_hash: None,
            created: Utc::now(),
            updated: Utc::now(),
            content_hash: hash.into(),
            scope: "project".into(),
        }
    }

    #[test]
    fn unchanged_hash_stays_vectorized_but_changed_hash_rearms() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let first = symbol("sym-1", "a", SymbolKind::Function);
        store
            .sync_file_symbols(std::slice::from_ref(&first), &[])
            .unwrap();
        assert!(store.mark_vectorized("sym-1", "a").unwrap());
        store
            .sync_file_symbols(std::slice::from_ref(&first), &["sym-1".into()])
            .unwrap();
        assert_eq!(store.stats().unwrap().vectorized, 1);

        let changed = symbol("sym-1", "b", SymbolKind::Function);
        store
            .sync_file_symbols(&[changed], &["sym-1".into()])
            .unwrap();
        assert_eq!(store.stats().unwrap().pending, 1);
        assert!(!store.mark_vectorized("sym-1", "a").unwrap());
    }

    #[test]
    fn removed_or_low_value_symbols_are_retired() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let high = symbol("sym-1", "a", SymbolKind::Function);
        store.sync_file_symbols(&[high], &[]).unwrap();
        let low = symbol("sym-1", "b", SymbolKind::Import);
        let retired = store.sync_file_symbols(&[low], &["sym-1".into()]).unwrap();
        assert_eq!(retired, vec!["sym-1"]);
        assert_eq!(store.stats().unwrap(), CodeVectorStats::default());
    }

    #[test]
    fn editing_one_symbol_rearms_only_that_chunk() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let first = symbol("sym-1", "a", SymbolKind::Function);
        let second = symbol("sym-2", "b", SymbolKind::Struct);
        store.sync_file_symbols(&[first, second], &[]).unwrap();
        assert!(store.mark_vectorized("sym-1", "a").unwrap());
        assert!(store.mark_vectorized("sym-2", "b").unwrap());

        let changed = symbol("sym-1", "a2", SymbolKind::Function);
        let unchanged = symbol("sym-2", "b", SymbolKind::Struct);
        store
            .sync_file_symbols(&[changed, unchanged], &["sym-1".into(), "sym-2".into()])
            .unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.vectorized, 1);
        assert_eq!(stats.eligible, 2);
    }
}
