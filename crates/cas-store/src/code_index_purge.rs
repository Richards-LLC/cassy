//! Bounded removal of code-index rows that do not belong to a project's
//! canonical checkout (cas-8256).
//!
//! Every factory worker used to index its whole worktree into the project's
//! shared `cas.db`, under a `repository` named after the worktree directory.
//! Nothing removed a copy when its worktree went away: the cassy store carried
//! 58 copies (364k symbols with full source, ~730 MB). This module deletes
//! those rows without ever holding the write lock for long: every transaction
//! changes at most [`WriteBatching::batch_size`] rows, the in-process
//! connection mutex is released between transactions, and the caller may
//! pause between batches so other writers get the lock.
//!
//! Secondary indexes (the code BM25 directory and the LMDB vector cache) live
//! outside SQLite and are owned by the CLI. The caller therefore drives the
//! loop: read a batch of symbol ids with [`SqliteCodeIndexPurge::next_symbol_batch`],
//! retire them from the secondary indexes, then delete the SQLite rows with
//! [`SqliteCodeIndexPurge::delete_symbols`]. The SQLite rows stay the retry
//! manifest until the secondary indexes have let go of them.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::types::Value;

use crate::{Result, StoreError};

/// Hard ceiling on rows one purge or reconcile transaction may change.
pub const CODE_WRITE_MAX_BATCH: usize = 1000;

/// Rows per transaction used by daemon-start cleanup and reconcile.
pub const CODE_WRITE_DEFAULT_BATCH: usize = 500;

/// How rows are grouped into write transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteBatching {
    /// Maximum rows one transaction changes, clamped to `1..=CODE_WRITE_MAX_BATCH`.
    pub batch_size: usize,
    /// Sleep between transactions so other writers can take the lock.
    pub pause: Duration,
}

impl WriteBatching {
    pub fn new(batch_size: usize, pause: Duration) -> Self {
        Self {
            batch_size: batch_size.clamp(1, CODE_WRITE_MAX_BATCH),
            pause,
        }
    }

    /// Production shape: 500 rows per transaction, 10 ms between them.
    pub fn background() -> Self {
        Self::new(CODE_WRITE_DEFAULT_BATCH, Duration::from_millis(10))
    }

    pub fn yield_between(&self) {
        if !self.pause.is_zero() {
            std::thread::sleep(self.pause);
        }
    }
}

impl Default for WriteBatching {
    fn default() -> Self {
        Self::new(CODE_WRITE_DEFAULT_BATCH, Duration::ZERO)
    }
}

/// Measured transaction shape of a batched write pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchedWrites {
    /// Write transactions committed.
    pub transactions: usize,
    /// Most rows changed by any single committed transaction.
    pub largest_transaction_rows: usize,
}

impl BatchedWrites {
    pub(crate) fn record(&mut self, rows: usize) {
        self.transactions += 1;
        self.largest_transaction_rows = self.largest_transaction_rows.max(rows);
    }
}

/// Which repositories' rows a purge removes.
#[derive(Debug, Clone, Copy)]
pub enum RepositoryScope<'a> {
    /// Every repository except these (the canonical keep-set). Refused when empty.
    AllExcept(&'a [String]),
    /// Exactly this repository (a removed worktree).
    Only(&'a str),
}

impl RepositoryScope<'_> {
    /// SQL predicate on a `repository` column plus its bound values.
    fn predicate(&self, column: &str) -> Result<(String, Vec<Value>)> {
        match self {
            RepositoryScope::AllExcept(keep) => {
                if keep.is_empty() {
                    return Err(StoreError::Other(
                        "refusing to purge the code index: the canonical keep-set is empty"
                            .to_string(),
                    ));
                }
                let placeholders = vec!["?"; keep.len()].join(", ");
                Ok((
                    format!("{column} NOT IN ({placeholders})"),
                    keep.iter().map(|name| Value::Text(name.clone())).collect(),
                ))
            }
            RepositoryScope::Only(repository) => Ok((
                format!("{column} = ?"),
                vec![Value::Text((*repository).to_string())],
            )),
        }
    }
}

/// Which `code_index_state` scan receipts a purge removes.
#[derive(Debug, Clone, Copy)]
pub enum ScanReceiptScope<'a> {
    /// Every checkout-scoped (`worktree:<path>`) receipt except these keys.
    WorktreeKeysExcept(&'a [String]),
    /// Exactly this key.
    Exact(&'a str),
}

/// What one purge removed, and the transaction shape it used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeIndexPurgeStats {
    pub writes: BatchedWrites,
    pub symbols_deleted: usize,
    pub queue_rows_deleted: usize,
    pub relationships_deleted: usize,
    pub memory_links_deleted: usize,
    pub files_deleted: usize,
    pub scan_receipts_deleted: usize,
}

impl CodeIndexPurgeStats {
    pub fn is_noop(&self) -> bool {
        self.symbols_deleted == 0
            && self.queue_rows_deleted == 0
            && self.relationships_deleted == 0
            && self.memory_links_deleted == 0
            && self.files_deleted == 0
            && self.scan_receipts_deleted == 0
    }
}

/// One symbol row selected for removal. `rowid` is the keyset cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeSymbol {
    pub rowid: i64,
    pub id: String,
}

/// Batched deletes over the code-index tables of one `cas.db`.
pub struct SqliteCodeIndexPurge {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCodeIndexPurge {
    /// Open the store only if `cas.db` already exists. A purge never creates
    /// a database or a table.
    pub fn open_existing(cas_dir: &Path) -> Result<Option<Self>> {
        let db_path = cas_dir.join("cas.db");
        if !db_path.is_file() {
            return Ok(None);
        }
        Ok(Some(Self {
            conn: crate::shared_db::shared_connection(&db_path)?,
        }))
    }

    /// Indexed files per repository, largest first.
    ///
    /// Counted from `code_files`, whose `UNIQUE(repository, path)` index
    /// answers the grouping. `code_symbols` has no repository index and
    /// carries full source, so grouping it would read the whole table.
    pub fn repository_file_counts(&self) -> Result<Vec<(String, usize)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        if !table_exists(&conn, "code_files") {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT repository, COUNT(*) FROM code_files
             GROUP BY repository ORDER BY COUNT(*) DESC, repository",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Up to `limit` symbols in `scope` with `rowid > after_rowid`, in rowid
    /// order. Read-only; the rowid keyset makes a whole purge one table pass.
    pub fn next_symbol_batch(
        &self,
        scope: RepositoryScope<'_>,
        after_rowid: i64,
        limit: usize,
    ) -> Result<Vec<PurgeSymbol>> {
        let (predicate, mut values) = scope.predicate("repository")?;
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        if !table_exists(&conn, "code_symbols") {
            return Ok(Vec::new());
        }
        values.insert(0, Value::Integer(after_rowid));
        values.push(Value::Integer(limit.clamp(1, CODE_WRITE_MAX_BATCH) as i64));
        let mut stmt = conn.prepare(&format!(
            "SELECT rowid, id FROM code_symbols
             WHERE rowid > ? AND {predicate}
             ORDER BY rowid LIMIT ?"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(PurgeSymbol {
                rowid: row.get(0)?,
                id: row.get(1)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Delete these symbols and every SQLite row derived from them: queue
    /// rows, relationships and memory links first, the symbol rows last.
    ///
    /// Each table is cleared in its own transactions of at most
    /// `batching.batch_size` rows, so a failure part-way leaves the symbol
    /// rows behind as the manifest for the next run.
    pub fn delete_symbols(
        &self,
        symbols: &[PurgeSymbol],
        batching: WriteBatching,
        stats: &mut CodeIndexPurgeStats,
    ) -> Result<()> {
        for chunk in symbols.chunks(batching.batch_size) {
            let ids: Vec<Value> = chunk
                .iter()
                .map(|symbol| Value::Text(symbol.id.clone()))
                .collect();
            let id_list = vec!["?"; ids.len()].join(", ");

            stats.queue_rows_deleted += self.delete_bounded(
                "code_vector_queue",
                &format!("symbol_id IN ({id_list})"),
                &ids,
                batching,
                &mut stats.writes,
            )?;
            let both: Vec<Value> = ids.iter().chain(ids.iter()).cloned().collect();
            stats.relationships_deleted += self.delete_bounded(
                "code_relationships",
                &format!("source_id IN ({id_list}) OR target_id IN ({id_list})"),
                &both,
                batching,
                &mut stats.writes,
            )?;
            stats.memory_links_deleted += self.delete_bounded(
                "code_memory_links",
                &format!("code_id IN ({id_list})"),
                &ids,
                batching,
                &mut stats.writes,
            )?;
            let rowids: Vec<Value> = chunk
                .iter()
                .map(|symbol| Value::Integer(symbol.rowid))
                .collect();
            let rowid_list = vec!["?"; rowids.len()].join(", ");
            stats.symbols_deleted += self.delete_bounded(
                "code_symbols",
                &format!("rowid IN ({rowid_list})"),
                &rowids,
                batching,
                &mut stats.writes,
            )?;
        }
        Ok(())
    }

    /// Delete `code_files` rows in `scope`, one bounded transaction per batch.
    pub fn delete_files(
        &self,
        scope: RepositoryScope<'_>,
        batching: WriteBatching,
        stats: &mut CodeIndexPurgeStats,
    ) -> Result<usize> {
        let (predicate, values) = scope.predicate("repository")?;
        let deleted = self.delete_bounded(
            "code_files",
            &predicate,
            &values,
            batching,
            &mut stats.writes,
        )?;
        stats.files_deleted += deleted;
        Ok(deleted)
    }

    /// Delete `code_index_state` scan receipts in `scope`. Legacy receipts
    /// keyed by a bare repository name are never touched by
    /// [`ScanReceiptScope::WorktreeKeysExcept`].
    pub fn delete_scan_receipts(
        &self,
        scope: ScanReceiptScope<'_>,
        batching: WriteBatching,
        stats: &mut CodeIndexPurgeStats,
    ) -> Result<usize> {
        let (predicate, values) = match scope {
            ScanReceiptScope::WorktreeKeysExcept(keep) => {
                let mut predicate = "substr(repository, 1, 9) = 'worktree:'".to_string();
                if !keep.is_empty() {
                    predicate.push_str(&format!(
                        " AND repository NOT IN ({})",
                        vec!["?"; keep.len()].join(", ")
                    ));
                }
                (
                    predicate,
                    keep.iter().map(|key| Value::Text(key.clone())).collect(),
                )
            }
            ScanReceiptScope::Exact(key) => (
                "repository = ?".to_string(),
                vec![Value::Text(key.to_string())],
            ),
        };
        let deleted = self.delete_bounded(
            "code_index_state",
            &predicate,
            &values,
            batching,
            &mut stats.writes,
        )?;
        stats.scan_receipts_deleted += deleted;
        Ok(deleted)
    }

    /// Delete rows of `table` matching `predicate`, at most one batch per
    /// `BEGIN IMMEDIATE` transaction, releasing the in-process connection and
    /// pausing between transactions. Returns the rows deleted.
    fn delete_bounded(
        &self,
        table: &str,
        predicate: &str,
        values: &[Value],
        batching: WriteBatching,
        writes: &mut BatchedWrites,
    ) -> Result<usize> {
        let sql = format!(
            "DELETE FROM {table} WHERE rowid IN (
                 SELECT rowid FROM {table} WHERE {predicate} LIMIT ?
             )"
        );
        let mut bound: Vec<Value> = values.to_vec();
        bound.push(Value::Integer(batching.batch_size as i64));
        let mut total = 0;
        loop {
            let deleted = {
                let conn = crate::shared_db::lock_connection(&self.conn)?;
                if !table_exists(&conn, table) {
                    return Ok(total);
                }
                without_foreign_keys(&conn, |conn| {
                    crate::shared_db::with_immediate_write_txn(conn, |tx| {
                        Ok(tx.execute(&sql, rusqlite::params_from_iter(bound.iter()))?)
                    })
                })?
            };
            if deleted == 0 {
                return Ok(total);
            }
            writes.record(deleted);
            total += deleted;
            if deleted < batching.batch_size {
                return Ok(total);
            }
            batching.yield_between();
        }
    }
}

/// Run `body` with foreign-key enforcement suspended on this connection,
/// restoring the previous setting afterwards (also on error).
///
/// The migration-built `code_symbols` declares `parent_id REFERENCES
/// code_symbols(id) ON DELETE SET NULL` with no index on `parent_id`, so with
/// enforcement on every deleted symbol scans the whole table. Measured on a
/// copy of the cassy store: about 1,500 of 356k foreign symbols removed in six
/// minutes, each transaction holding the write lock for tens of seconds. The
/// purge deletes every dependent row (queue, relationships, memory links)
/// explicitly and removes a repository's symbols together, so the referential
/// actions have nothing left to do. `PRAGMA foreign_keys` is a no-op inside a
/// transaction, hence it is set before `BEGIN`; the caller holds the
/// connection mutex, so no other in-process user sees the suspended setting.
fn without_foreign_keys<T>(
    conn: &Connection,
    body: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let enforced = conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? != 0;
    if !enforced {
        return body(conn);
    }
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let result = body(conn);
    let restored = conn.execute_batch("PRAGMA foreign_keys = ON");
    let value = result?;
    restored?;
    Ok(value)
}

pub(crate) fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

#[cfg(test)]
#[path = "code_index_purge_tests.rs"]
mod tests;
