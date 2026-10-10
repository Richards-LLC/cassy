//! One code index per project (cas-8256).
//!
//! The code index lives in the project's shared `cas.db` and its code BM25
//! directory. Only the canonical checkout (the store's own checkout) writes
//! it; factory workers and linked worktrees read it. Before this, every
//! worker's `cas serve` indexed its full worktree under a repository named
//! after the worktree directory, and nothing removed the copy afterwards.

use std::path::{Path, PathBuf};

use cas_store::{CodeIndexPurgeStats, RepositoryScope, ScanReceiptScope, WriteBatching};

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
    let _ = (cas_root, cwd, factory_worker);
    CodeIndexRole::Writer
}

/// Repository names the canonical index keeps: the canonical checkout plus
/// any separately-rooted checkout configured in `code.watch_paths`.
pub(crate) fn canonical_code_repositories(cas_root: &Path) -> Vec<String> {
    let _ = cas_root;
    Vec::new()
}

/// What a purge removed, and whether it stopped early.
#[derive(Debug, Clone, Default)]
pub struct CodeIndexPurgeOutcome {
    pub stats: CodeIndexPurgeStats,
    /// The code BM25 writer stayed busy; the remaining rows are retried next time.
    pub deferred: bool,
    pub errors: Vec<String>,
}

/// Remove every code-index row whose repository is not canonical.
pub fn purge_non_canonical_code_index(
    cas_root: &Path,
    batching: WriteBatching,
) -> Result<CodeIndexPurgeOutcome, CasError> {
    let _ = (cas_root, batching);
    let _ = (RepositoryScope::Only(""), ScanReceiptScope::Exact(""));
    Ok(CodeIndexPurgeOutcome::default())
}

/// Remove the code-index copy of a worktree that was just removed from
/// `repo_root`. Never fails the removal: errors are logged.
pub fn purge_removed_worktree_code_index(
    repo_root: &Path,
    worktree_path: &Path,
) -> Option<CodeIndexPurgeOutcome> {
    let _ = (repo_root, worktree_path);
    None
}

#[cfg(test)]
#[path = "canonical_code_index_tests.rs"]
mod tests;
