use crate::ui::factory::app::imports::*;

impl FactoryApp {
    /// Get the project path
    pub fn project_path(&self) -> &std::path::Path {
        &self.project_dir
    }

    /// Create an epic branch based on the configured trunk (not supervisor HEAD)
    ///
    /// Base resolution order (cas-b082): `.cas/config.toml`
    /// `[factory] epic_base_branch` if set, else the repo's detected
    /// default branch. Either way, the base is fetched and resolved
    /// against its remote tip before branching — a stale local base can
    /// never silently seed a new epic branch (BUG-epic-branch-stale-local-base).
    pub fn create_epic_branch(
        &self,
        epic_title: &str,
        epic_id: &str,
    ) -> anyhow::Result<String> {
        use crate::config::Config;
        use crate::worktree::GitOperations;

        let branch_name = epic_branch_name(epic_title, epic_id);
        let git_ops = GitOperations::new(self.project_dir.clone());
        let trunk = Config::configured_epic_base_branch(&self.project_dir)
            .unwrap_or_else(|| git_ops.detect_default_branch());
        let resolved = git_ops.resolve_fresh_base(&trunk)?;

        // cas-a85e (GH #99): a checkout on the previous epic branch must not
        // silently strand it; base from it, or state what was left out.
        let base_choice = git_ops.resolve_epic_base(&resolved.branch_ref);
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
        // `resolved.behind_count` is local trunk vs origin/trunk; once the base
        // is HEAD's epic branch it no longer describes the printed base.
        let (base_sha, base_behind) = if base_choice.used_head {
            (
                git_ops.ref_sha(&base_choice.base_ref).unwrap_or_default(),
                base_choice.head_behind,
            )
        } else {
            (resolved.sha.clone(), resolved.behind_count)
        };

        if git_ops.create_branch_from(&branch_name, &base_sha)? {
            tracing::info!(
                "Created epic branch {} from base '{}' (sha={}, behind={})",
                branch_name,
                base_choice.base_ref,
                &base_sha[..base_sha.len().min(7)],
                base_behind,
            );
        } else {
            tracing::info!("Epic branch already exists: {}", branch_name);
        }

        Ok(branch_name)
    }

    /// Merge all worker branches to the epic branch
    pub fn merge_workers_to_epic(&self) -> anyhow::Result<Vec<(String, bool, Option<String>)>> {
        let epic_branch = self
            .epic_branch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No epic branch active"))?;

        if let Some(manager) = &self.worktree_manager {
            let results = manager.merge_workers_to_epic(epic_branch)?;
            Ok(results)
        } else {
            // No worktrees - nothing to merge
            Ok(Vec::new())
        }
    }

    /// Cleanup worker branches after epic completion
    ///
    /// Deletes all worker branches that have been merged into the epic branch.
    pub fn cleanup_worker_branches(&self, force: bool) -> anyhow::Result<Vec<String>> {
        let epic_branch = self
            .epic_branch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No epic branch active"))?;

        if let Some(manager) = &self.worktree_manager {
            let deleted = manager.cleanup_worker_branches(epic_branch, force)?;
            Ok(deleted)
        } else {
            // No worktrees - nothing to cleanup
            Ok(Vec::new())
        }
    }
}
