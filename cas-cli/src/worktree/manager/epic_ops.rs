use crate::config::Config;
use crate::worktree::git::GitError;
use crate::worktree::manager::{
    MergeVenue, WorktreeError, WorktreeManager, WorktreeResult, slugify_title,
};

/// First 7 chars of a SHA for log/echo output, or the whole string if shorter
/// (e.g. the empty string when a ref couldn't be resolved).
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

impl WorktreeManager {
    /// Create an epic branch from the configured trunk (not the current HEAD)
    ///
    /// Base resolution order (cas-b082): `.cas/config.toml`
    /// `[factory] epic_base_branch` if set, else the repo's detected
    /// default branch. Either way, the base is fetched and resolved
    /// against its remote tip before branching — a stale local base can
    /// never silently seed a new epic branch (BUG-epic-branch-stale-local-base).
    pub fn create_epic_branch(&self, epic_title: &str) -> WorktreeResult<String> {
        let slug = slugify_title(epic_title);
        let branch_name = format!("epic/{slug}");
        let trunk = Config::configured_epic_base_branch(&self.repo_root)
            .unwrap_or_else(|| self.git.detect_default_branch());
        let resolved = self
            .git
            .resolve_fresh_base(&trunk)
            .map_err(WorktreeError::Git)?;

        // cas-a85e (GH #99): prefer the active epic branch when the checkout
        // is on one that trunk does not contain, so a follow-on epic does not
        // start empty; otherwise keep trunk and say what was excluded.
        let base_choice = self.git.resolve_epic_base(&resolved.branch_ref);
        if let Some(notice) = base_choice.notice.as_deref() {
            // stack_depth is emitted as a structured field (cas-aae6) so ops
            // tooling can alert on deep stacks without parsing prose.
            let stack_depth = base_choice.stacked_on.len();
            if base_choice.used_head {
                tracing::info!(stack_depth, stacked_on = ?base_choice.stacked_on, "{}", notice);
            } else {
                tracing::warn!(stack_depth, stacked_on = ?base_choice.stacked_on, "{}", notice);
            }
        }
        // `resolved.behind_count` describes local trunk vs origin/trunk. Once
        // the base is HEAD's epic branch instead, that number describes a
        // different pair of refs — report the base's own gap to trunk.
        let (base_sha, base_behind) = if base_choice.used_head {
            (
                self.git.ref_sha(&base_choice.base_ref).unwrap_or_default(),
                base_choice.head_behind,
            )
        } else {
            (resolved.sha.clone(), resolved.behind_count)
        };

        let newly_created = match self
            .git
            .create_branch_from(&branch_name, &base_choice.base_ref)
        {
            Ok(true) => {
                tracing::info!(
                    "Created epic branch {} from base '{}' (sha={}, behind={})",
                    branch_name,
                    base_choice.base_ref,
                    short_sha(&base_sha),
                    base_behind,
                );
                true
            }
            Ok(false) => {
                tracing::info!("Using existing epic branch: {}", branch_name);
                false
            }
            Err(e) => {
                return Err(WorktreeError::Git(e));
            }
        };

        if newly_created {
            if let Err(e) = self.git.push_branch(&branch_name) {
                tracing::warn!("Failed to push epic branch to remote: {}", e);
            } else {
                tracing::info!("Pushed epic branch to remote: {}", branch_name);
            }
        }

        Ok(branch_name)
    }

    /// Merge all worker branches into the epic branch
    pub fn merge_workers_to_epic(
        &self,
        epic_branch: &str,
    ) -> WorktreeResult<Vec<(String, bool, Option<String>)>> {
        let mut results = Vec::new();

        // cas-4702 / GH #68: epic close must never leave the main checkout on
        // the epic branch. When the checkout is already there the merges run
        // in place; otherwise each merge runs in an ephemeral detached
        // worktree and advances the epic ref by compare-and-swap, so HEAD is
        // exactly where the operator left it when this returns.
        let venue = self.merge_venue(epic_branch);

        for (name, worktree) in &self.workers {
            let worker_branch = &worktree.branch;

            if !self.git.branch_exists(worker_branch)? {
                results.push((name.clone(), false, Some("Branch not found".to_string())));
                continue;
            }

            let merge_result = match venue {
                MergeVenue::SharedCheckout => {
                    self.git.merge_branch(epic_branch, worker_branch, true)
                }
                MergeVenue::TempWorktree => {
                    self.git
                        .merge_branch_via_temp_worktree(epic_branch, worker_branch, true)
                }
            };

            match merge_result {
                Ok(_commit) => {
                    tracing::info!("Merged {} into {}", worker_branch, epic_branch);
                    results.push((name.clone(), true, None));
                }
                Err(GitError::MergeConflict) => {
                    // cas-e18f: `merge_branch` now aborts-on-failure itself,
                    // so the checkout is already clean here — no manual
                    // `git merge --abort` needed.
                    results.push((
                        name.clone(),
                        false,
                        Some("Merge conflict - manual resolution required".to_string()),
                    ));
                }
                Err(GitError::MergeConflictPaths(paths)) => {
                    results.push((
                        name.clone(),
                        false,
                        Some(format!(
                            "Merge conflict in: {} - manual resolution required",
                            paths.join(", ")
                        )),
                    ));
                }
                Err(e) => {
                    results.push((name.clone(), false, Some(e.to_string())));
                }
            }
        }

        Ok(results)
    }

    /// Cleanup worker branches after epic completion
    pub fn cleanup_worker_branches(
        &self,
        epic_branch: &str,
        force: bool,
    ) -> WorktreeResult<Vec<String>> {
        let mut deleted = Vec::new();

        for (name, worktree) in &self.workers {
            let worker_branch = &worktree.branch;

            if !self.git.branch_exists(worker_branch)? {
                continue;
            }

            let is_merged = self.is_branch_merged(worker_branch, epic_branch)?;

            if is_merged || force {
                match self.git.delete_branch(worker_branch, force) {
                    Ok(()) => {
                        tracing::info!(
                            "Deleted worker branch: {} (worker: {})",
                            worker_branch,
                            name
                        );
                        deleted.push(worker_branch.clone());
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to delete branch {} (may still be checked out in worktree): {}",
                            worker_branch,
                            e
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "Branch {} not merged into {} - skipping cleanup",
                    worker_branch,
                    epic_branch
                );
            }
        }

        Ok(deleted)
    }

    /// Check if a branch is merged into another branch
    pub(crate) fn is_branch_merged(&self, branch: &str, into: &str) -> WorktreeResult<bool> {
        use std::process::Command;

        let output = Command::new("git")
            .args(["branch", "--merged", into])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().any(|line| {
            let trimmed = line
                .trim()
                .trim_start_matches('*')
                .trim_start_matches('+')
                .trim_start_matches('-')
                .trim();
            trimmed == branch
        }))
    }

    /// Get a list of orphaned epic branches (epic branches with no active workers)
    pub fn list_orphaned_epic_branches(&self) -> WorktreeResult<Vec<String>> {
        let worktrees = self.git.list_worktrees()?;

        let mut epic_branches: Vec<String> = worktrees
            .iter()
            .filter_map(|wt| wt.branch.as_ref())
            .filter(|b| b.starts_with("epic/"))
            .cloned()
            .collect();

        epic_branches.sort();
        epic_branches.dedup();

        Ok(epic_branches)
    }
}
