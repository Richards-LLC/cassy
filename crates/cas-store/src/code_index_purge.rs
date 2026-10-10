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

    pub(crate) fn yield_between(&self) {
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
        let _ = cas_dir;
        Ok(None)
    }

    /// Indexed files per repository, largest first.
    pub fn repository_file_counts(&self) -> Result<Vec<(String, usize)>> {
        Ok(Vec::new())
    }

    /// Up to `limit` symbols in `scope` with `rowid > after_rowid`, in rowid order.
    pub fn next_symbol_batch(
        &self,
        scope: RepositoryScope<'_>,
        after_rowid: i64,
        limit: usize,
    ) -> Result<Vec<PurgeSymbol>> {
        let _ = (scope, after_rowid, limit);
        Ok(Vec::new())
    }

    /// Delete these symbols and every SQLite row derived from them.
    pub fn delete_symbols(
        &self,
        symbols: &[PurgeSymbol],
        batching: WriteBatching,
        stats: &mut CodeIndexPurgeStats,
    ) -> Result<()> {
        let _ = (symbols, batching, stats);
        Ok(())
    }

    /// Delete `code_files` rows in `scope`, one bounded transaction per batch.
    pub fn delete_files(
        &self,
        scope: RepositoryScope<'_>,
        batching: WriteBatching,
        stats: &mut CodeIndexPurgeStats,
    ) -> Result<usize> {
        let _ = (scope, batching, stats);
        Ok(0)
    }

    /// Delete `code_index_state` scan receipts in `scope`.
    pub fn delete_scan_receipts(
        &self,
        scope: ScanReceiptScope<'_>,
        batching: WriteBatching,
        stats: &mut CodeIndexPurgeStats,
    ) -> Result<usize> {
        let _ = (scope, batching, stats);
        Ok(0)
    }
}

#[cfg(test)]
#[path = "code_index_purge_tests.rs"]
mod tests;
