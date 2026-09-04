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

/// What one [`SqliteCodeVectorStore::reconcile`] pass actually changed.
///
/// Every field is a row count the operator can check against the numbers
/// `cas doctor` prints, which is the point: the doctor line used to name
/// `cas index code` as the remedy for orphaned and never-queued rows while
/// that command touched neither (cas-8a03).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeVectorReconcile {
    /// Queue rows deleted because no eligible symbol carries their id.
    pub orphaned_dropped: usize,
    /// `failed` rows returned to `pending` so the drain retries them.
    pub failed_rearmed: usize,
    /// `failed` rows deliberately left alone because their recorded error
    /// describes input the provider will reject again. Cleared by `force`.
    pub failed_retained: usize,
    /// Rows whose `content_hash` no longer matched the symbol's. The drain
    /// skips those rows forever (`embed_pending_code` refuses to mark a hash
    /// the symbol no longer has), so they are permanent pending work until
    /// something rewrites the hash.
    pub stale_rearmed: usize,
    /// Eligible symbols that had no queue row at all and now have one.
    pub requeued: usize,
    /// Ids of the dropped orphan rows, so the caller can retire their cached
    /// vectors from LMDB in the same pass.
    pub dropped_symbol_ids: Vec<String>,
}

impl CodeVectorReconcile {
    /// True when the pass changed nothing — used to keep quiet runs quiet.
    pub fn is_noop(&self) -> bool {
        self.orphaned_dropped == 0
            && self.failed_rearmed == 0
            && self.stale_rearmed == 0
            && self.requeued == 0
    }
}

/// Whether a recorded queue failure is worth retrying without `--force`.
///
/// Deliberately optimistic: everything the drain records today is an
/// environment fact (provider request error, missing embedding capability
/// while logged out, an unusable response), and one wasted embed call is
/// cheaper than a warning no operator command can clear. Only errors that name
/// input the provider will reject identically on every retry are retained, and
/// `cas index code --force` re-arms even those.
pub fn is_retryable_vector_failure(last_error: Option<&str>) -> bool {
    let Some(error) = last_error.map(str::to_lowercase) else {
        return true;
    };
    const PERMANENT: &[&str] = &[
        "too large",
        "exceeds maximum",
        "invalid request",
        "unsupported input",
        "400 bad request",
        "422 unprocessable",
    ];
    !PERMANENT.iter().any(|marker| error.contains(marker))
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
        // `retire_code_file` opens this store once per retired file, so schema
        // creation sits on a hot path that runs beside `cas doctor`. Retry the
        // DDL rather than surfacing SQLITE_BUSY as a file failure (cas-8a03).
        crate::shared_db::with_write_retry(|| {
            conn.lock()
                .map_err(|_| StoreError::Other("lock poisoned".to_string()))?
                .execute_batch(CODE_VECTOR_SCHEMA)
                .map_err(StoreError::from)
        })?;
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

        let conn = self.lock()?;
        // The write lock is taken up front with bounded retry: a concurrent
        // `cas doctor` or second `cas serve` holding the writer used to turn
        // this into a hard "database is locked" file failure (cas-8a03,
        // mirroring cas-759f).
        let tx = crate::shared_db::begin_immediate_with_retry(&conn)?;
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
        let conn = self.lock()?;
        crate::shared_db::with_immediate_write_txn(&conn, |tx| {
            for id in symbol_ids {
                tx.execute(
                    "DELETE FROM code_vector_queue WHERE symbol_id = ?1",
                    params![id],
                )?;
            }
            Ok(())
        })
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

    /// Make the queue describe the symbol table again, and report what moved.
    ///
    /// Three inconsistencies accumulate that no incremental indexing pass can
    /// remove, because incremental indexing only visits files whose content
    /// changed (cas-8a03):
    ///
    /// 1. **Orphaned rows** — a queue row whose symbol no longer exists (or is
    ///    no longer an embeddable kind). The drain only retires these if it
    ///    happens to reach them, and it never reaches them while thousands of
    ///    live rows sort ahead.
    /// 2. **Failed rows** — durable, retryable in principle, but nothing
    ///    re-arms them on demand. Without `force`, rows whose recorded error
    ///    names permanently-invalid input are retained and counted; `force`
    ///    re-arms every failed row.
    /// 3. **Never-queued and stale-hash symbols** — an eligible symbol with no
    ///    queue row is invisible to the drain, and a row whose `content_hash`
    ///    disagrees with its symbol is skipped by the drain on every tick
    ///    (`embed_pending_code` refuses to complete a hash the symbol no
    ///    longer has), so it is pending work that can never finish.
    ///
    /// The whole pass runs in one `BEGIN IMMEDIATE` transaction with bounded
    /// retry, so a concurrent reader-writer (`cas doctor`, a second
    /// `cas serve`) delays it instead of failing it.
    pub fn reconcile(&self, force: bool) -> Result<CodeVectorReconcile> {
        let conn = self.lock()?;
        let kinds = cas_code::SymbolKind::embeddable_kind_names();
        let placeholders = vec!["?"; kinds.len()].join(", ");

        // No `code_symbols` table means structural indexing has never run in
        // this store. Every queue row would then read as orphaned; emptying
        // the queue on the strength of a table that merely has not been
        // created yet would delete real work.
        let symbols_table = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'code_symbols'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0);
        if symbols_table == 0 {
            return Ok(CodeVectorReconcile::default());
        }

        crate::shared_db::with_immediate_write_txn(&conn, |tx| {
            let now = Utc::now().to_rfc3339();
            let mut outcome = CodeVectorReconcile::default();

            let dropped: Vec<String> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT symbol_id FROM code_vector_queue q
                     WHERE NOT EXISTS (
                         SELECT 1 FROM code_symbols s
                         WHERE s.id = q.symbol_id AND s.kind IN ({placeholders})
                     )"
                ))?;
                let rows = stmt.query_map(rusqlite::params_from_iter(kinds.iter()), |row| {
                    row.get::<_, String>(0)
                })?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            for id in &dropped {
                tx.execute(
                    "DELETE FROM code_vector_queue WHERE symbol_id = ?1",
                    params![id],
                )?;
            }
            outcome.orphaned_dropped = dropped.len();
            outcome.dropped_symbol_ids = dropped;

            let failed: Vec<(String, Option<String>)> = {
                let mut stmt = tx.prepare(
                    "SELECT symbol_id, last_error FROM code_vector_queue WHERE status = 'failed'",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            for (symbol_id, last_error) in &failed {
                if !force && !is_retryable_vector_failure(last_error.as_deref()) {
                    outcome.failed_retained += 1;
                    continue;
                }
                tx.execute(
                    "UPDATE code_vector_queue
                     SET status = 'pending', last_error = NULL, updated_at = ?2
                     WHERE symbol_id = ?1",
                    params![symbol_id, now],
                )?;
                outcome.failed_rearmed += 1;
            }

            outcome.stale_rearmed = tx.execute(
                &format!(
                    "UPDATE code_vector_queue
                     SET content_hash = (
                             SELECT s.content_hash FROM code_symbols s
                             WHERE s.id = code_vector_queue.symbol_id
                         ),
                         status = 'pending',
                         last_error = NULL,
                         updated_at = ?1
                     WHERE EXISTS (
                         SELECT 1 FROM code_symbols s
                         WHERE s.id = code_vector_queue.symbol_id
                           AND s.kind IN ({placeholders})
                           AND s.content_hash <> code_vector_queue.content_hash
                     )"
                ),
                rusqlite::params_from_iter(
                    std::iter::once(now.clone()).chain(kinds.iter().map(|kind| kind.to_string())),
                ),
            )?;

            outcome.requeued = tx.execute(
                &format!(
                    "INSERT OR IGNORE INTO code_vector_queue
                         (symbol_id, content_hash, status, last_error, updated_at)
                     SELECT s.id, s.content_hash, 'pending', NULL, ?1
                     FROM code_symbols s
                     WHERE s.kind IN ({placeholders})
                       AND NOT EXISTS (
                           SELECT 1 FROM code_vector_queue q WHERE q.symbol_id = s.id
                       )"
                ),
                rusqlite::params_from_iter(
                    std::iter::once(now.clone()).chain(kinds.iter().map(|kind| kind.to_string())),
                ),
            )?;

            Ok(outcome)
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

    /// The cas-8a03 shape, all three inconsistencies at once: a queue row for a
    /// deleted symbol, a failed row, a symbol the indexer never queued, and a
    /// row whose hash no longer matches its symbol. One reconcile pass has to
    /// leave doctor with nothing left to warn about.
    #[test]
    fn reconcile_drops_orphans_rearms_failures_and_queues_missing_symbols() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();

        let failed = symbol("sym-failed", "a", SymbolKind::Function);
        let stale = symbol("sym-stale", "old", SymbolKind::Struct);
        let never_queued = symbol("sym-new", "c", SymbolKind::Trait);
        let ghost = symbol("sym-gone", "g", SymbolKind::Function);

        // The queue knows about the ghost, the failure and the stale row.
        store
            .sync_file_symbols(&[ghost, failed.clone(), stale.clone()], &[])
            .unwrap();
        assert!(
            store
                .mark_failed("sym-failed", "a", "provider request failed: 503")
                .unwrap()
        );
        // The symbol table has everything except the ghost, and carries a newer
        // hash for the stale row.
        seed_symbols(
            root.path(),
            &[
                failed,
                symbol("sym-stale", "new", SymbolKind::Struct),
                never_queued,
            ],
        );

        let before = store.coverage().unwrap();
        assert_eq!(before.orphaned, 1);
        assert_eq!(before.unqueued, 1);
        assert_eq!(before.failed, 1);

        let outcome = store.reconcile(false).unwrap();
        assert_eq!(outcome.orphaned_dropped, 1);
        assert_eq!(outcome.dropped_symbol_ids, vec!["sym-gone".to_string()]);
        assert_eq!(outcome.failed_rearmed, 1);
        assert_eq!(outcome.failed_retained, 0);
        assert_eq!(outcome.stale_rearmed, 1);
        assert_eq!(outcome.requeued, 1);
        assert!(!outcome.is_noop());

        let after = store.coverage().unwrap();
        assert_eq!(after.orphaned, 0, "orphaned queue rows survived reconcile");
        assert_eq!(after.unqueued, 0, "never-queued symbols survived reconcile");
        assert_eq!(after.failed, 0, "retryable failure survived reconcile");
        assert_eq!(after.eligible, 3);
        assert_eq!(after.pending, 3);

        // The stale row is now drainable: its hash matches the symbol, which is
        // the drain's own completion condition.
        let pending = store.list_pending(10).unwrap();
        assert!(
            pending
                .iter()
                .any(|work| work.symbol_id == "sym-stale" && work.content_hash == "new"),
            "stale queue row kept an unreachable hash: {pending:?}"
        );
    }

    /// A second pass with nothing to do reports nothing, so the command can
    /// stay quiet on a healthy store.
    #[test]
    fn reconcile_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let one = symbol("sym-1", "a", SymbolKind::Function);
        seed_symbols(root.path(), std::slice::from_ref(&one));

        assert_eq!(store.reconcile(false).unwrap().requeued, 1);
        let second = store.reconcile(false).unwrap();
        assert_eq!(second, CodeVectorReconcile::default());
        assert!(second.is_noop());
    }

    /// Permanently-invalid input is not re-armed by a plain run — it would fail
    /// identically — but `--force` re-arms it, which is the documented
    /// remediation doctor prints for the residual.
    #[test]
    fn reconcile_retains_permanent_failures_until_forced() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let broken = symbol("sym-1", "a", SymbolKind::Function);
        seed_symbols(root.path(), std::slice::from_ref(&broken));
        store
            .sync_file_symbols(std::slice::from_ref(&broken), &[])
            .unwrap();
        assert!(
            store
                .mark_failed("sym-1", "a", "input too large for the embedding model")
                .unwrap()
        );

        let plain = store.reconcile(false).unwrap();
        assert_eq!(plain.failed_rearmed, 0);
        assert_eq!(plain.failed_retained, 1);
        assert_eq!(store.coverage().unwrap().failed, 1);

        let forced = store.reconcile(true).unwrap();
        assert_eq!(forced.failed_rearmed, 1);
        assert_eq!(forced.failed_retained, 0);
        assert_eq!(store.coverage().unwrap().failed, 0);
    }

    /// A store whose structural index never ran has no symbol table. Reconcile
    /// must not read that as "every queue row is an orphan" and empty the queue.
    #[test]
    fn reconcile_without_a_symbol_table_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        store
            .sync_file_symbols(&[symbol("sym-1", "a", SymbolKind::Function)], &[])
            .unwrap();

        assert_eq!(store.reconcile(false).unwrap(), CodeVectorReconcile::default());
        assert_eq!(store.stats().unwrap().pending, 1);
    }

    /// Hold the write lock from a foreign connection — what a concurrent
    /// `cas doctor` or second `cas serve` does — and both the reconcile and the
    /// retirement path must wait it out instead of failing with
    /// "database is locked" (cas-8a03; the retire failure is what turned a
    /// parallel doctor run into 36 file failures).
    #[test]
    fn queue_writes_wait_out_a_foreign_write_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let store = SqliteCodeVectorStore::open(root.path()).unwrap();
        let symbols = [
            symbol("sym-1", "a", SymbolKind::Function),
            symbol("sym-2", "b", SymbolKind::Struct),
        ];
        seed_symbols(root.path(), &symbols);
        store.sync_file_symbols(&symbols, &[]).unwrap();

        let db_path = root.path().join("cas.db");
        let (holding, held) = mpsc::channel();
        let blocker = std::thread::spawn(move || {
            // A separate `Connection::open` deliberately bypasses the
            // process-wide shared connection: an in-process mutex would
            // serialize instead of reproducing SQLITE_BUSY.
            let conn = Connection::open(&db_path).unwrap();
            conn.busy_timeout(crate::SQLITE_BUSY_TIMEOUT).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO code_vector_queue
                     (symbol_id, content_hash, status, last_error, updated_at)
                 VALUES ('foreign', 'h', 'pending', NULL, '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            holding.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            conn.execute_batch("COMMIT").unwrap();
        });

        held.recv().unwrap();
        store
            .retire(&["sym-2".to_string()])
            .expect("retire waited out the foreign write lock");
        assert!(
            store
                .list_pending(10)
                .unwrap()
                .iter()
                .all(|work| work.symbol_id != "sym-2"),
            "retire did not take effect after waiting"
        );

        // The symbol is still live, so the next reconcile is expected to queue
        // it again — that it does so proves the reconcile write landed too.
        let outcome = store
            .reconcile(false)
            .expect("reconcile waited out the foreign write lock");
        assert_eq!(outcome.requeued, 1);
        blocker.join().unwrap();
        assert!(
            store
                .list_pending(10)
                .unwrap()
                .iter()
                .any(|work| work.symbol_id == "sym-2"),
            "reconcile did not take effect after waiting"
        );
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
