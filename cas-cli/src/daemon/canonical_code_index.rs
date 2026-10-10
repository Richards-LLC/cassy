//! One code index per project (cas-8256).
//!
//! The code index lives in the project's shared `cas.db` and its code BM25
//! directory. Only the canonical checkout (the store's own checkout) writes
//! it; factory workers and linked worktrees read it. Before this, every
//! worker's `cas serve` indexed its full worktree under a repository named
//! after the worktree directory, and nothing removed the copy afterwards: the
//! cassy store carried 58 copies (364k symbols with full source, ~730 MB).

use std::path::{Path, PathBuf};

use cas_store::{
    CodeIndexPurgeStats, RepositoryScope, ScanReceiptScope, SqliteCodeIndexPurge, WriteBatching,
};

use crate::daemon::indexing::{
    WRITER_LOCK_BUDGET, WriterLockBudget, canonical_source_path, code_index_dir,
    code_project_root_from, code_scan_key, is_index_lock_busy, publish_code_symbols,
    resolve_repository, retire_cached_code_vectors,
};
use crate::error::CasError;

/// The checkout whose code this project indexes: the store's own checkout.
pub(crate) fn canonical_code_root(cas_root: &Path) -> PathBuf {
    cas_root.parent().unwrap_or(cas_root).to_path_buf()
}

/// Whether this process may write the shared code index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodeIndexRole {
    /// The canonical checkout's process: indexes, reconciles and purges.
    Writer,
    /// Reads the canonical index; the reason names why it does not write.
    Reader(String),
}

impl CodeIndexRole {
    pub(crate) fn is_writer(&self) -> bool {
        matches!(self, CodeIndexRole::Writer)
    }
}

/// Role of the current process, from its working directory and factory env.
pub(crate) fn code_index_role(cas_root: &Path) -> CodeIndexRole {
    code_index_role_from(
        cas_root,
        &std::env::current_dir().unwrap_or_default(),
        crate::harness_policy::is_worker_from_env(),
    )
}

pub(crate) fn code_index_role_from(
    cas_root: &Path,
    cwd: &Path,
    factory_worker: bool,
) -> CodeIndexRole {
    if factory_worker {
        return CodeIndexRole::Reader("factory worker (CAS_AGENT_ROLE=worker)".to_string());
    }
    // Factory worktrees live under `<cas_root>/worktrees`, even while git is
    // mid-removal and cannot answer for them.
    let cwd_physical = canonical_source_path(cwd);
    if cwd_physical.starts_with(canonical_source_path(&cas_root.join("worktrees"))) {
        return CodeIndexRole::Reader(format!("factory worktree {}", cwd.display()));
    }
    let Some(checkout) = resolve_repository(cwd).0 else {
        return CodeIndexRole::Writer;
    };
    let canonical = canonical_code_root(cas_root);
    let canonical_checkout = resolve_repository(&canonical).0.unwrap_or(canonical);
    let checkout_physical = canonical_source_path(&checkout);
    if checkout_physical == canonical_source_path(&canonical_checkout) {
        return CodeIndexRole::Writer;
    }
    // Same repository as the store, different checkout: a linked worktree.
    // An unrelated checkout keeps writing the explicit store's own checkout.
    if canonical_source_path(&code_project_root_from(cas_root, cwd)) == checkout_physical {
        CodeIndexRole::Reader(format!("linked worktree {}", checkout.display()))
    } else {
        CodeIndexRole::Writer
    }
}

/// Canonical checkout roots: the store's own checkout plus any separately
/// rooted checkout explicitly configured in `code.watch_paths`.
///
/// `None` when a root is outside any git checkout. There, files take their
/// parent directory's name as repository, so no keep-set is safe to purge by.
fn canonical_checkouts(cas_root: &Path) -> Option<Vec<(PathBuf, String)>> {
    let root = canonical_code_root(cas_root);
    let config = crate::config::Config::load(cas_root)
        .unwrap_or_default()
        .code();
    let mut roots = vec![root.clone()];
    roots.extend(config.watch_paths.iter().map(|path| root.join(path)));
    let mut checkouts: Vec<(PathBuf, String)> = Vec::new();
    for path in roots {
        let (checkout, name) = resolve_repository(&path);
        let checkout = canonical_source_path(&checkout?);
        if !checkouts.iter().any(|(_, known)| *known == name) {
            checkouts.push((checkout, name));
        }
    }
    Some(checkouts)
}

/// Repository names the canonical index keeps (see [`canonical_checkouts`]).
pub(crate) fn canonical_code_repositories(cas_root: &Path) -> Option<Vec<String>> {
    canonical_checkouts(cas_root)
        .map(|checkouts| checkouts.into_iter().map(|(_, name)| name).collect())
}

/// What a purge removed, and whether it stopped early.
#[derive(Debug, Clone, Default)]
pub struct CodeIndexPurgeOutcome {
    pub stats: CodeIndexPurgeStats,
    /// The code BM25 writer stayed busy; the remaining rows are retried next time.
    pub deferred: bool,
    /// Why nothing was attempted, when nothing was.
    pub skipped: Option<String>,
    pub errors: Vec<String>,
}

impl CodeIndexPurgeOutcome {
    /// One log line for the daemon and doctor.
    pub fn summary(&self) -> String {
        if let Some(reason) = &self.skipped {
            return format!("skipped: {reason}");
        }
        let stats = &self.stats;
        let mut line = format!(
            "removed {} symbol(s), {} file(s), {} queue row(s), {} scan receipt(s) in {} \
             transaction(s) of at most {} row(s)",
            stats.symbols_deleted,
            stats.files_deleted,
            stats.queue_rows_deleted,
            stats.scan_receipts_deleted,
            stats.writes.transactions,
            stats.writes.largest_transaction_rows,
        );
        if self.deferred {
            line.push_str("; deferred: code search writer busy, the rest is retried next start");
        }
        if !self.errors.is_empty() {
            line.push_str(&format!(
                "; {} error(s): {}",
                self.errors.len(),
                self.errors.join("; ")
            ));
        }
        line
    }
}

/// Remove every code-index row whose repository is not canonical. Idempotent.
pub fn purge_non_canonical_code_index(
    cas_root: &Path,
    batching: WriteBatching,
) -> Result<CodeIndexPurgeOutcome, CasError> {
    let Some(checkouts) = canonical_checkouts(cas_root) else {
        return Ok(CodeIndexPurgeOutcome {
            skipped: Some(
                "the canonical checkout is not a git checkout, so copies cannot be told apart"
                    .to_string(),
            ),
            ..Default::default()
        });
    };
    let keep: Vec<String> = checkouts.iter().map(|(_, name)| name.clone()).collect();
    let keep_keys: Vec<String> = checkouts
        .iter()
        .map(|(checkout, _)| code_scan_key(checkout))
        .collect();
    purge_scope(
        cas_root,
        RepositoryScope::AllExcept(&keep),
        ScanReceiptScope::WorktreeKeysExcept(&keep_keys),
        batching,
    )
}

/// Remove the code-index copy of a worktree just removed from `repo_root`.
/// Never fails the removal: an error is logged and the daemon-start purge
/// retries it.
pub fn purge_removed_worktree_code_index(
    repo_root: &Path,
    worktree_path: &Path,
) -> Option<CodeIndexPurgeOutcome> {
    let cas_root = repo_root.join(".cas");
    if !cas_root.join("cas.db").is_file() {
        return None;
    }
    let repository = worktree_path.file_name()?.to_string_lossy().to_string();
    let keep = canonical_code_repositories(&cas_root)?;
    if keep.contains(&repository) {
        return None;
    }
    let key = code_scan_key(worktree_path);
    match purge_scope(
        &cas_root,
        RepositoryScope::Only(&repository),
        ScanReceiptScope::Exact(&key),
        WriteBatching::background(),
    ) {
        Ok(outcome) => {
            if !outcome.stats.is_noop() || outcome.deferred || !outcome.errors.is_empty() {
                eprintln!(
                    "[Cassy] Code index copy of removed worktree {repository}: {}",
                    outcome.summary()
                );
            }
            Some(outcome)
        }
        Err(error) => {
            tracing::warn!(%error, repository, "code index purge after worktree removal failed");
            None
        }
    }
}

/// Retire each batch from the code BM25 index and the LMDB vector cache, then
/// delete its SQLite rows. The SQLite rows stay the retry manifest until the
/// secondary indexes have let go, so an interrupted purge resumes cleanly.
fn purge_scope(
    cas_root: &Path,
    scope: RepositoryScope<'_>,
    receipts: ScanReceiptScope<'_>,
    batching: WriteBatching,
) -> Result<CodeIndexPurgeOutcome, CasError> {
    let store_error =
        |error: cas_store::StoreError| CasError::Other(format!("code index purge: {error}"));
    let mut outcome = CodeIndexPurgeOutcome::default();
    let Some(store) = SqliteCodeIndexPurge::open_existing(cas_root).map_err(store_error)? else {
        return Ok(outcome);
    };
    let bm25_present = code_index_dir(cas_root).join("meta.json").exists();
    let mut budget = WriterLockBudget::new(WRITER_LOCK_BUDGET);
    let mut after = 0;
    loop {
        let batch = store
            .next_symbol_batch(scope, after, batching.batch_size)
            .map_err(store_error)?;
        let Some(last) = batch.last() else {
            break;
        };
        after = last.rowid;
        let ids: Vec<String> = batch.iter().map(|symbol| symbol.id.clone()).collect();
        if bm25_present {
            match budget.run(|| publish_code_symbols(cas_root, &[], &ids)) {
                Ok(_) => {}
                Err(error) if is_index_lock_busy(&error) => {
                    outcome.deferred = true;
                    return Ok(outcome);
                }
                Err(error) => {
                    outcome.errors.push(format!("code search index: {error}"));
                    return Ok(outcome);
                }
            }
        }
        if let Err(error) = retire_cached_code_vectors(cas_root, &ids) {
            outcome.errors.push(format!("code vector cache: {error}"));
            return Ok(outcome);
        }
        store
            .delete_symbols(&batch, batching, &mut outcome.stats)
            .map_err(store_error)?;
        batching.yield_between();
    }
    store
        .delete_files(scope, batching, &mut outcome.stats)
        .map_err(store_error)?;
    store
        .delete_scan_receipts(receipts, batching, &mut outcome.stats)
        .map_err(store_error)?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "canonical_code_index_tests.rs"]
mod tests;
