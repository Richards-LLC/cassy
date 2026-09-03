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
    skipped_files INTEGER NOT NULL DEFAULT 0,
    skipped_detail TEXT,
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
        skipped_files INTEGER NOT NULL DEFAULT 0,
        skipped_detail TEXT,
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

/// Coverage of the semantic code-vector corpus, measured against the symbols
/// that are actually eligible for a vector.
///
/// [`CodeVectorStats`] counts queue rows and nothing else, so it cannot tell an
/// idle corpus ("nothing left to embed") apart from a lost queue ("13k symbols
/// with no vector and no row asking for one"). Both report zero pending. This
/// type joins `code_vector_queue` to `code_symbols` so every eligible symbol is
/// in exactly one bucket, and names the two inconsistencies that used to hide
/// inside the queue-only numbers (cas-73e7 / GH #696).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeVectorCoverage {
    /// Symbols whose kind the drain embeds. The denominator, taken from the
    /// symbol table rather than the queue, so it only moves when indexing does.
    pub eligible: usize,
    /// Eligible symbols with a `vectorized` queue row for their *current*
    /// content hash — the drain's own completion condition.
    pub vectorized: usize,
    /// Eligible symbols still awaiting a vector: queued-pending, queued against
    /// a stale hash, never queued at all, or failed-with-a-newer-hash.
    pub pending: usize,
    /// Eligible symbols whose current hash is recorded as `failed`. Retryable —
    /// `list_pending` picks them up again — but reported separately because a
    /// durable failure is not the same fact as work that has never been tried.
    pub failed: usize,
    /// Eligible symbols with no queue row whatsoever. Included in `pending`;
    /// broken out because it means the *indexer* dropped them, not the drain.
    pub unqueued: usize,
    /// Queue rows pointing at a symbol that no longer exists (or is no longer
    /// embeddable). The drain retires these when it reaches them; until then
    /// they are queue rows that describe no real work.
    pub orphaned: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeIndexState {
    pub repository: String,
    pub eligible_files: usize,
    pub indexed_files: usize,
    pub failed_files: usize,
    /// Files excluded from the eligible denominator because their bytes are
    /// not decodable source text. Kept apart from `failed_files` so a rerun
    /// cannot resurrect a warning that no retry could ever clear (GH #698).
    pub skipped_files: usize,
    /// Human-readable "path: reason" list for the skipped files.
    pub skipped_detail: Option<String>,
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

    /// Coverage measured against the symbol table, not just the queue.
    ///
    /// Every eligible symbol lands in exactly one of `vectorized` / `failed` /
    /// `pending`, so `pending` answers "how many symbols are still missing a
    /// vector" instead of "how many rows happen to sit in the queue". Queue
    /// rows with no live eligible symbol are reported as `orphaned` rather than
    /// counted as work.
    ///
    /// Deliberately unscoped by repository: `code_vector_queue` has no
    /// repository column, so scoping only the symbol side would misfile every
    /// other repository's rows as orphaned. Both sides span the whole store.
    pub fn coverage(&self) -> Result<CodeVectorCoverage> {
        let conn = self.lock()?;
        let kinds = cas_code::SymbolKind::embeddable_kind_names();
        let queued: usize = conn
            .query_row("SELECT COUNT(*) FROM code_vector_queue", [], |row| {
                row.get::<_, i64>(0)
            })?
            .max(0) as usize;

        // The code store owns `code_symbols` and creates it on open. A store
        // where structural indexing has never run has no symbol table at all;
        // that is zero eligible symbols, and any queue row is then orphaned.
        let symbols_table = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'code_symbols'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0);
        if symbols_table == 0 {
            return Ok(CodeVectorCoverage {
                orphaned: queued,
                ..Default::default()
            });
        }

        let placeholders = vec!["?"; kinds.len()].join(", ");
        let (eligible, vectorized, failed, unqueued) = conn.query_row(
            &format!(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN q.status = 'vectorized'
                                          AND q.content_hash = s.content_hash
                                     THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN q.status = 'failed'
                                          AND q.content_hash = s.content_hash
                                     THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN q.symbol_id IS NULL THEN 1 ELSE 0 END), 0)
                 FROM code_symbols s
                 LEFT JOIN code_vector_queue q ON q.symbol_id = s.id
                 WHERE s.kind IN ({placeholders})"
            ),
            rusqlite::params_from_iter(kinds.iter()),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;

        let orphaned: usize = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM code_vector_queue q
                     WHERE NOT EXISTS (
                         SELECT 1 FROM code_symbols s
                         WHERE s.id = q.symbol_id AND s.kind IN ({placeholders})
                     )"
                ),
                rusqlite::params_from_iter(kinds.iter()),
                |row| row.get::<_, i64>(0),
            )?
            .max(0) as usize;

        let eligible = eligible.max(0) as usize;
        let vectorized = vectorized.max(0) as usize;
        let failed = failed.max(0) as usize;
        Ok(CodeVectorCoverage {
            eligible,
            vectorized,
            failed,
            pending: eligible.saturating_sub(vectorized).saturating_sub(failed),
            unqueued: unqueued.max(0) as usize,
            orphaned,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_scan(
        &self,
        repository: &str,
        eligible_files: usize,
        indexed_files: usize,
        failed_files: usize,
        skipped_files: usize,
        skipped_detail: Option<&str>,
        last_head: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO code_index_state
                 (repository, eligible_files, indexed_files, failed_files,
                  skipped_files, skipped_detail, last_head, last_scan_at, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(repository) DO UPDATE SET
                 eligible_files = excluded.eligible_files,
                 indexed_files = excluded.indexed_files,
                 failed_files = excluded.failed_files,
                 skipped_files = excluded.skipped_files,
                 skipped_detail = excluded.skipped_detail,
                 last_head = excluded.last_head,
                 last_scan_at = excluded.last_scan_at,
                 last_error = excluded.last_error",
            params![
                repository,
                eligible_files as i64,
                indexed_files as i64,
                failed_files as i64,
                skipped_files as i64,
                skipped_detail,
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
                    skipped_files, skipped_detail, last_head, last_scan_at, last_error
             FROM code_index_state WHERE repository = ?1",
            params![repository],
            |row| {
                Ok(CodeIndexState {
                    repository: row.get(0)?,
                    eligible_files: row.get::<_, i64>(1)?.max(0) as usize,
                    indexed_files: row.get::<_, i64>(2)?.max(0) as usize,
                    failed_files: row.get::<_, i64>(3)?.max(0) as usize,
                    skipped_files: row.get::<_, i64>(4)?.max(0) as usize,
                    skipped_detail: row.get(5)?,
                    last_head: row.get(6)?,
                    last_scan_at: row.get(7)?,
                    last_error: row.get(8)?,
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

    /// Put `symbols` in the code store so `coverage()` has a denominator, the
    /// same way the structural indexer would.
    fn seed_symbols(root: &Path, symbols: &[CodeSymbol]) {
        use crate::CodeStore;
        let store = crate::sqlite_code_store::SqliteCodeStore::open(root).unwrap();
        for symbol in symbols {
            store.add_symbol(symbol).unwrap();
        }
    }

    /// The GH #696 shape: the queue was emptied (daemon restart, retirement
    /// storm) while thousands of symbols still have no vector. Queue-only stats
    /// call that "0 pending" — a clean bill of health for a corpus with no
    /// vectors at all.
    #[test]
    fn coverage_counts_symbols_with_no_queue_row_as_pending() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let a = symbol("sym-1", "a", SymbolKind::Function);
        let b = symbol("sym-2", "b", SymbolKind::Struct);
        let ignored = symbol("sym-3", "c", SymbolKind::Import);
        seed_symbols(root.path(), &[a, b, ignored]);

        assert_eq!(store.stats().unwrap(), CodeVectorStats::default());
        let coverage = store.coverage().unwrap();
        assert_eq!(coverage.eligible, 2, "low-value kinds are not eligible");
        assert_eq!(coverage.pending, 2);
        assert_eq!(coverage.unqueued, 2);
        assert_eq!(coverage.vectorized, 0);
        assert_eq!(coverage.orphaned, 0);
    }

    #[test]
    fn coverage_splits_vectorized_pending_and_failed_against_current_hashes() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let done = symbol("sym-1", "a", SymbolKind::Function);
        let waiting = symbol("sym-2", "b", SymbolKind::Struct);
        let broken = symbol("sym-3", "c", SymbolKind::Trait);
        seed_symbols(root.path(), &[done.clone(), waiting.clone(), broken.clone()]);
        store
            .sync_file_symbols(&[done, waiting, broken], &[])
            .unwrap();
        assert!(store.mark_vectorized("sym-1", "a").unwrap());
        assert!(store.mark_failed("sym-3", "c", "provider refused").unwrap());

        let coverage = store.coverage().unwrap();
        assert_eq!(coverage.eligible, 3);
        assert_eq!(coverage.vectorized, 1);
        assert_eq!(coverage.pending, 1);
        assert_eq!(coverage.failed, 1);
        assert_eq!(coverage.unqueued, 0);
        assert_eq!(coverage.orphaned, 0);
    }

    /// A `vectorized` row for a hash the symbol no longer has is not coverage:
    /// the drain will re-embed it, so it belongs in `pending`.
    #[test]
    fn coverage_treats_a_stale_vectorized_row_as_pending() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let first = symbol("sym-1", "a", SymbolKind::Function);
        seed_symbols(root.path(), std::slice::from_ref(&first));
        store
            .sync_file_symbols(std::slice::from_ref(&first), &[])
            .unwrap();
        assert!(store.mark_vectorized("sym-1", "a").unwrap());

        // The symbol was edited; the code store carries the new hash while the
        // queue row still records the old one.
        seed_symbols(root.path(), &[symbol("sym-1", "a2", SymbolKind::Function)]);

        assert_eq!(store.stats().unwrap().vectorized, 1, "queue-only view");
        let coverage = store.coverage().unwrap();
        assert_eq!(coverage.vectorized, 0);
        assert_eq!(coverage.pending, 1);
        assert_eq!(coverage.unqueued, 0);
    }

    /// The inverse lie: queue rows outliving their symbols would report pending
    /// work that no drain tick can ever complete.
    #[test]
    fn coverage_reports_queue_rows_without_a_symbol_as_orphaned() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let ghost = symbol("sym-gone", "a", SymbolKind::Function);
        store.sync_file_symbols(&[ghost], &[]).unwrap();
        seed_symbols(root.path(), &[symbol("sym-1", "b", SymbolKind::Function)]);

        assert_eq!(store.stats().unwrap().pending, 1, "queue-only view");
        let coverage = store.coverage().unwrap();
        assert_eq!(coverage.eligible, 1);
        assert_eq!(coverage.pending, 1);
        assert_eq!(coverage.unqueued, 1);
        assert_eq!(coverage.orphaned, 1);
    }

    /// Two reads with no indexing and no drain in between must agree — the
    /// stability GH #696 asks for.
    #[test]
    fn coverage_is_stable_across_repeated_reads() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let one = symbol("sym-1", "a", SymbolKind::Function);
        let two = symbol("sym-2", "b", SymbolKind::Struct);
        seed_symbols(root.path(), &[one.clone(), two.clone()]);
        store.sync_file_symbols(&[one, two], &[]).unwrap();
        assert!(store.mark_vectorized("sym-1", "a").unwrap());

        let first = store.coverage().unwrap();
        let second = store.coverage().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.eligible, 2);
        assert_eq!(first.vectorized, 1);
        assert_eq!(first.pending, 1);
    }

    /// A store where code indexing never ran has no `code_symbols` table. That
    /// must read as "nothing eligible", not as an error or a panic.
    #[test]
    fn coverage_without_a_symbol_table_reports_no_eligible_work() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        assert_eq!(store.coverage().unwrap(), CodeVectorCoverage::default());
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
