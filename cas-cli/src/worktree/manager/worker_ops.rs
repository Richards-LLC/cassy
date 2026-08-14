use std::collections::HashMap;
use std::path::PathBuf;

use crate::types::Worktree;
use crate::worktree::external_symlinks::{scan_external_symlinks_into, ExternalSymlink};
use crate::worktree::git::GitOperations;
use crate::worktree::manager::{WorktreeError, WorktreeManager, WorktreeResult, symlink_project_config};

/// Describes a worker worktree that was left on disk because it held uncommitted work.
///
/// Surfaced to the factory UI so dirty teardowns are loud, and to the daemon reaper
/// (Unit 3) via the agent metadata flag for eventual TTL-based salvage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyWorktreeWarning {
    pub worker_name: String,
    pub path: PathBuf,
    pub file_count: usize,
}

/// Describes a worker worktree that was left on disk because live external
/// symlinks resolve into it (cas-df97). Removing the worktree would leave
/// every one of `links` dangling — real incident: a stow/install step run
/// from inside a worktree repointed ~21 `$HOME` symlinks (`.gitconfig`,
/// `.ssh/config`, `~/bin/*`, systemd user units, ...) into it, and a later
/// routine cleanup silently orphaned all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSymlinkWarning {
    pub worker_name: String,
    pub path: PathBuf,
    pub links: Vec<ExternalSymlink>,
}

/// Outcome of attempting a non-force shutdown of a single worker worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    /// Worker wasn't tracked by the manager (no worktree in the map).
    NotTracked,
    /// Worktree was clean and has been removed; branch deleted best-effort.
    Removed,
    /// Worktree had uncommitted work; left on disk for deferred salvage.
    DirtyDeferred(DirtyWorktreeWarning),
    /// Live external symlinks resolve into the worktree; left on disk so
    /// nothing goes dangling (cas-df97).
    ExternalSymlinksBlocked(ExternalSymlinkWarning),
}

/// Result of `cleanup_workers` — what was removed, what was deferred as
/// dirty, and what was blocked by live external symlinks.
#[derive(Debug, Clone, Default)]
pub struct CleanupReport {
    pub cleaned: Vec<String>,
    pub dirty_deferred: Vec<DirtyWorktreeWarning>,
    pub external_symlinks_blocked: Vec<ExternalSymlinkWarning>,
}

impl WorktreeManager {
    /// Calculate the worktree path for a factory worker
    pub fn worktree_path_for_worker(&self, worker_name: &str) -> PathBuf {
        self.worktree_root().join(worker_name)
    }

    /// Calculate the branch name for a factory worker
    pub fn branch_name_for_worker(&self, worker_name: &str) -> String {
        format!("factory/{worker_name}")
    }

    /// Check if a worktree exists for a worker
    pub fn worktree_exists_for_worker(&self, worker_name: &str) -> bool {
        let path = self.worktree_path_for_worker(worker_name);
        path.exists()
    }

    /// Create a worktree for a factory worker
    pub fn create_for_worker(&mut self, worker_name: &str) -> WorktreeResult<Worktree> {
        if self.context.is_worktree {
            return Err(WorktreeError::AlreadyInWorktree);
        }

        let worktree_path = self.worktree_path_for_worker(worker_name);
        let branch_name = self.branch_name_for_worker(worker_name);
        let parent_branch = self.git.current_branch()?;
        let resolved = self
            .git
            .resolve_fresh_base(&parent_branch)
            .map_err(WorktreeError::Git)?;

        if let Some(parent) = worktree_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.git
            .create_worktree(&worktree_path, &branch_name, Some(&resolved.branch_ref))?;

        let _ = self.git.mark_config_skip_worktree(&worktree_path);
        symlink_project_config(&self.repo_root, &worktree_path);

        let worktree = Worktree::new(
            Worktree::generate_id(),
            branch_name,
            parent_branch,
            worktree_path,
        );

        self.workers
            .insert(worker_name.to_string(), worktree.clone());

        Ok(worktree)
    }

    /// Create a worktree for a factory worker from a specific parent branch
    pub fn create_for_worker_from(
        &mut self,
        worker_name: &str,
        parent_branch: &str,
    ) -> WorktreeResult<Worktree> {
        if self.context.is_worktree {
            return Err(WorktreeError::AlreadyInWorktree);
        }

        let worktree_path = self.worktree_path_for_worker(worker_name);
        let branch_name = self.branch_name_for_worker(worker_name);
        let resolved = self
            .git
            .resolve_fresh_base(parent_branch)
            .map_err(WorktreeError::Git)?;

        if let Some(parent) = worktree_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.git
            .create_worktree(&worktree_path, &branch_name, Some(&resolved.branch_ref))?;

        let _ = self.git.mark_config_skip_worktree(&worktree_path);
        symlink_project_config(&self.repo_root, &worktree_path);

        let worktree = Worktree::new(
            Worktree::generate_id(),
            branch_name,
            parent_branch.to_string(),
            worktree_path,
        );

        self.workers
            .insert(worker_name.to_string(), worktree.clone());

        Ok(worktree)
    }

    /// Ensure a worktree exists for a worker (idempotent)
    pub fn ensure_worker_worktree(&mut self, worker_name: &str) -> WorktreeResult<&Worktree> {
        if self.workers.contains_key(worker_name) {
            return self.worker_ref(worker_name);
        }

        let worktree_path = self.worktree_path_for_worker(worker_name);
        if worktree_path.exists() {
            let _ = self.git.mark_config_skip_worktree(&worktree_path);
            let _ = self.git.init_submodules(&worktree_path);
            symlink_project_config(&self.repo_root, &worktree_path);

            let branch_name = self.branch_name_for_worker(worker_name);
            let parent_branch = self
                .context
                .branch
                .clone()
                .unwrap_or_else(|| self.git.detect_default_branch());

            let worktree = Worktree::new(
                Worktree::generate_id(),
                branch_name,
                parent_branch,
                worktree_path,
            );
            self.workers.insert(worker_name.to_string(), worktree);
            return self.worker_ref(worker_name);
        }

        self.create_for_worker(worker_name)?;
        self.worker_ref(worker_name)
    }

    /// Ensure a worktree exists for a worker from a specific parent branch (idempotent)
    pub fn ensure_worker_worktree_from(
        &mut self,
        worker_name: &str,
        parent_branch: &str,
    ) -> WorktreeResult<&Worktree> {
        if self.workers.contains_key(worker_name) {
            return self.worker_ref(worker_name);
        }

        let worktree_path = self.worktree_path_for_worker(worker_name);
        if worktree_path.exists() {
            let _ = self.git.mark_config_skip_worktree(&worktree_path);
            let _ = self.git.init_submodules(&worktree_path);
            symlink_project_config(&self.repo_root, &worktree_path);

            let branch_name = self.branch_name_for_worker(worker_name);

            let worktree = Worktree::new(
                Worktree::generate_id(),
                branch_name,
                parent_branch.to_string(),
                worktree_path,
            );
            self.workers.insert(worker_name.to_string(), worktree);
            return self.worker_ref(worker_name);
        }

        self.create_for_worker_from(worker_name, parent_branch)?;
        self.worker_ref(worker_name)
    }

    /// Get worker working directories for MuxConfig
    pub fn worker_cwds(&self) -> HashMap<String, PathBuf> {
        self.workers
            .iter()
            .filter(|(_, wt)| wt.path.exists())
            .map(|(name, wt)| (name.clone(), wt.path.clone()))
            .collect()
    }

    /// Get a worker's worktree if it exists
    pub fn get_worker(&self, worker_name: &str) -> Option<&Worktree> {
        self.workers.get(worker_name)
    }

    /// Register a worktree that was created externally.
    pub fn register_worktree(&mut self, worker_name: &str, worktree: Worktree) {
        self.workers.insert(worker_name.to_string(), worktree);
    }

    /// Get a reference to the git operations wrapper
    pub fn git(&self) -> &GitOperations {
        &self.git
    }

    /// Cleanup worker worktrees.
    ///
    /// With `force = true`, every tracked worktree is removed regardless of
    /// state. With `force = false`, dirty worktrees are left on disk and
    /// reported via [`CleanupReport::dirty_deferred`] so the caller can warn
    /// the operator — callers must no longer silently treat dirty trees as
    /// "removed and forgotten".
    pub fn cleanup_workers(&mut self, force: bool) -> WorktreeResult<CleanupReport> {
        let mut report = CleanupReport::default();

        let worker_names: Vec<String> = self.workers.keys().cloned().collect();

        for name in worker_names {
            if let Some(mut worktree) = self.workers.remove(&name) {
                // cas-df97: live external symlinks block regardless of
                // `force` — force means "bypass git dirty-tree protection",
                // not "I'm aware this will orphan $HOME symlinks".
                if worktree.path.exists() {
                    let links = scan_external_symlinks_into(&worktree.path);
                    if !links.is_empty() {
                        report.external_symlinks_blocked.push(ExternalSymlinkWarning {
                            worker_name: name.clone(),
                            path: worktree.path.clone(),
                            links,
                        });
                        self.workers.insert(name, worktree);
                        continue;
                    }
                }

                if !force && worktree.path.exists() {
                    let file_count = self
                        .git
                        .uncommitted_file_count(&worktree.path)
                        .unwrap_or(0);
                    if file_count > 0 {
                        report.dirty_deferred.push(DirtyWorktreeWarning {
                            worker_name: name.clone(),
                            path: worktree.path.clone(),
                            file_count,
                        });
                        self.workers.insert(name, worktree);
                        continue;
                    }
                }

                if worktree.path.exists() {
                    let _ = self.git.remove_worktree(&worktree.path, force);
                }

                let _ = self.git.delete_branch(&worktree.branch, true);

                worktree.mark_abandoned();
                worktree.mark_removed();

                report.cleaned.push(name);
            }
        }

        Ok(report)
    }

    /// Remove a single worker's worktree
    pub fn remove_worker(&mut self, worker_name: &str, force: bool) -> WorktreeResult<()> {
        if let Some(mut worktree) = self.workers.remove(worker_name) {
            // cas-df97: live external symlinks block regardless of `force`
            // — see the identical guard in cleanup_workers.
            if worktree.path.exists() {
                let links = scan_external_symlinks_into(&worktree.path);
                if !links.is_empty() {
                    let warning = ExternalSymlinkWarning {
                        worker_name: worker_name.to_string(),
                        path: worktree.path.clone(),
                        links,
                    };
                    self.workers.insert(worker_name.to_string(), worktree);
                    return Err(WorktreeError::ExternalSymlinksDetected(warning));
                }
            }

            // cas-006c: named-path classification, not a raw "any porcelain
            // output" check — see GitOperations::classify_dirty_status.
            // will_remove=true always: remove_worker unconditionally deletes
            // the worktree directory when it exists, so untracked files
            // must block exactly like tracked ones (supervisor review
            // finding cas-006c — untracked-only debris is destroyed, not
            // preserved, by an actual removal).
            if !force && worktree.path.exists() {
                if let Err(e) = self.reject_or_warn_on_dirty(&worktree.path, true) {
                    self.workers.insert(worker_name.to_string(), worktree);
                    return Err(e);
                }
            }

            // cas-006c: force=true unconditionally at the git layer — by
            // this point either the caller forced past our dirty-check gate
            // above, or `reject_or_warn_on_dirty(path, true)` already vetted
            // the tree as safe to remove (blocking on untracked too, not
            // just tracked changes).
            if worktree.path.exists() {
                self.git.remove_worktree(&worktree.path, true)?;
            }

            let _ = self.git.delete_branch(&worktree.branch, true);

            worktree.mark_abandoned();
            worktree.mark_removed();
        }

        Ok(())
    }

    /// Attempt to remove a single worker's worktree on graceful shutdown.
    ///
    /// Non-force semantics: clean trees are removed and the branch is deleted
    /// best-effort; dirty trees are left on disk and described in
    /// [`RemoveOutcome::DirtyDeferred`] so the caller can warn and mark the
    /// worker for later salvage. Callers who need to force-remove a dirty
    /// tree should use [`WorktreeManager::remove_worker`] with `force = true`.
    pub fn attempt_remove_worker(
        &mut self,
        worker_name: &str,
    ) -> WorktreeResult<RemoveOutcome> {
        let mut worktree = match self.workers.remove(worker_name) {
            Some(wt) => wt,
            None => return Ok(RemoveOutcome::NotTracked),
        };

        if worktree.path.exists() {
            // cas-df97: live external symlinks block regardless of dirty
            // state — this is the actual production path
            // (finalize_worker_worktree) that the reported incident went
            // through.
            let links = scan_external_symlinks_into(&worktree.path);
            if !links.is_empty() {
                let warning = ExternalSymlinkWarning {
                    worker_name: worker_name.to_string(),
                    path: worktree.path.clone(),
                    links,
                };
                self.workers.insert(worker_name.to_string(), worktree);
                return Ok(RemoveOutcome::ExternalSymlinksBlocked(warning));
            }

            let file_count = self
                .git
                .uncommitted_file_count(&worktree.path)
                .unwrap_or(0);
            if file_count > 0 {
                let warning = DirtyWorktreeWarning {
                    worker_name: worker_name.to_string(),
                    path: worktree.path.clone(),
                    file_count,
                };
                self.workers.insert(worker_name.to_string(), worktree);
                return Ok(RemoveOutcome::DirtyDeferred(warning));
            }

            self.git.remove_worktree(&worktree.path, false)?;
        }

        let _ = self.git.delete_branch(&worktree.branch, true);

        worktree.mark_abandoned();
        worktree.mark_removed();

        Ok(RemoveOutcome::Removed)
    }
}
