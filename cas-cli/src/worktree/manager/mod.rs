//! High-level worktree manager that integrates git operations with CAS storage
//!
//! This module coordinates between git worktree operations and CAS's epic/task system.
//! Worktrees are scoped to epics, allowing multiple tasks within an epic to share
//! a single development environment and git branch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::types::{GitContext, Worktree};

use crate::worktree::git::{GitError, GitOperations};

/// Configuration for worktree management
#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    /// Whether worktree creation is enabled
    pub enabled: bool,

    /// Base directory for worktrees (relative to repo root's parent)
    /// Supports {project} placeholder
    pub base_path: String,

    /// Prefix for branch names (e.g., "cas/")
    pub branch_prefix: String,

    /// Auto-merge on epic close
    pub auto_merge: bool,

    /// Auto-cleanup worktree directory on epic close
    pub cleanup_on_close: bool,

    /// Promote entries with positive feedback on merge
    pub promote_entries_on_merge: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            base_path: "{project}/.cas/worktrees".to_string(),
            branch_prefix: "cas/".to_string(),
            auto_merge: false,
            cleanup_on_close: true,
            promote_entries_on_merge: true,
        }
    }
}

/// Result type for worktree operations
pub type WorktreeResult<T> = std::result::Result<T, WorktreeError>;

/// Errors that can occur during worktree management
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("Worktrees are not enabled in configuration")]
    NotEnabled,

    #[error("Git error: {0}")]
    Git(#[from] GitError),

    #[error("Not in a git repository")]
    NotAGitRepo,

    #[error("Already in a worktree - cannot create nested worktrees")]
    AlreadyInWorktree,

    #[error("Worktree not found: {0}")]
    NotFound(String),

    /// cas-006c: names the offending tracked paths (with status) rather
    /// than forcing a manual `git status` to find out what's blocking.
    #[error("Worktree has uncommitted changes: {0}")]
    UncommittedChanges(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// High-level worktree manager
pub struct WorktreeManager {
    /// Git operations wrapper
    git: GitOperations,

    /// Configuration
    config: WorktreeConfig,

    /// Path to main repository root
    repo_root: PathBuf,

    /// Current git context
    context: GitContext,

    /// Factory worker worktrees (worker_name -> worktree)
    workers: HashMap<String, Worktree>,
}

mod epic_ops;
pub mod worker_ops;

pub use worker_ops::{CleanupReport, DirtyWorktreeWarning, RemoveOutcome};

impl WorktreeManager {
    fn worker_ref(&self, worker_name: &str) -> WorktreeResult<&Worktree> {
        self.workers
            .get(worker_name)
            .ok_or_else(|| WorktreeError::NotFound(worker_name.to_string()))
    }

    /// Shared dirty-check gate for force-free merge/removal (cas-006c).
    ///
    /// Blocks on tracked modified/added/deleted paths, naming each with its
    /// status. Untracked paths and CAS-generated artifacts (`.husky/_/`)
    /// never block — untracked paths only get a `tracing::warn!` naming
    /// them, since a merge or removal cannot lose data git never tracked.
    fn reject_or_warn_on_dirty(&self, path: &Path) -> WorktreeResult<()> {
        let dirty = self.git.classify_dirty_status(path)?;
        if dirty.is_blocked() {
            return Err(WorktreeError::UncommittedChanges(dirty.describe_blocking()));
        }
        if !dirty.warnings.is_empty() {
            tracing::warn!(
                "Worktree {} has untracked files (not blocking): {}",
                path.display(),
                dirty.describe_warnings()
            );
        }
        Ok(())
    }

    /// Create a new WorktreeManager
    ///
    /// # Arguments
    /// * `cwd` - Current working directory (used to detect repo)
    /// * `config` - Worktree configuration
    pub fn new(cwd: &Path, config: WorktreeConfig) -> WorktreeResult<Self> {
        // Check if git is available
        if !GitOperations::is_git_available() {
            return Err(WorktreeError::Git(GitError::GitNotAvailable(
                "git command not found".to_string(),
            )));
        }

        // Detect repo root
        let repo_root = GitOperations::detect_repo_root(cwd)?;

        // Get current context
        let context = GitOperations::get_context(cwd)?;

        let git = GitOperations::new(repo_root.clone());

        Ok(Self {
            git,
            config,
            repo_root,
            context,
            workers: HashMap::new(),
        })
    }

    /// Check if worktrees are enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the current git context
    pub fn context(&self) -> &GitContext {
        &self.context
    }

    /// Get the current branch
    pub fn current_branch(&self) -> Option<&str> {
        self.context.branch.as_deref()
    }

    /// Check if we're currently in a worktree
    pub fn is_in_worktree(&self) -> bool {
        self.context.is_worktree
    }

    /// Calculate the worktree path for an epic
    pub fn worktree_path_for_epic(&self, epic_id: &str) -> PathBuf {
        let project_name = self
            .repo_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");

        let base = self.config.base_path.replace("{project}", project_name);
        let base_path = if base.starts_with('/') {
            PathBuf::from(base)
        } else {
            self.repo_root
                .parent()
                .unwrap_or(&self.repo_root)
                .join(base)
        };

        base_path.join(epic_id)
    }

    /// Calculate the branch name for an epic
    pub fn branch_name_for_epic(&self, epic_id: &str) -> String {
        format!("{}{}", self.config.branch_prefix, epic_id)
    }

    /// Create a worktree for an epic
    ///
    /// This is the preferred way to create worktrees. Multiple tasks within
    /// the same epic share this worktree.
    ///
    /// # Arguments
    /// * `epic_id` - The epic ID
    /// * `agent_id` - Optional agent ID that's creating the worktree
    ///
    /// # Returns
    /// A Worktree struct with the details
    pub fn create_for_epic(
        &self,
        epic_id: &str,
        agent_id: Option<&str>,
    ) -> WorktreeResult<Worktree> {
        if !self.config.enabled {
            return Err(WorktreeError::NotEnabled);
        }

        // Don't allow nested worktrees
        if self.context.is_worktree {
            return Err(WorktreeError::AlreadyInWorktree);
        }

        let worktree_path = self.worktree_path_for_epic(epic_id);
        let branch_name = self.branch_name_for_epic(epic_id);
        let parent_branch = self.git.current_branch()?;

        // Ensure parent directory exists
        if let Some(parent) = worktree_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create the git worktree
        self.git
            .create_worktree(&worktree_path, &branch_name, Some(&parent_branch))?;

        // Mark tracked config files as skip-worktree so workers can't
        // accidentally commit CAS-synced changes (rules, skills, settings).
        let _ = self.git.mark_config_skip_worktree(&worktree_path);

        // Symlink gitignored config (.mcp.json, .claude/) into the worktree
        // so workers get MCP server access even when these files aren't tracked.
        symlink_project_config(&self.repo_root, &worktree_path);

        // Build the Worktree record
        let worktree = Worktree::for_epic(
            Worktree::generate_id(),
            epic_id.to_string(),
            branch_name,
            parent_branch,
            worktree_path,
            agent_id.map(String::from),
        );

        Ok(worktree)
    }

    /// Check if a worktree exists for an epic
    pub fn worktree_exists_for_epic(&self, epic_id: &str) -> bool {
        let path = self.worktree_path_for_epic(epic_id);
        path.exists()
    }

    /// Merge a worktree into its parent branch, optionally removing it.
    ///
    /// # Arguments
    /// * `worktree` - The worktree to merge
    /// * `force` - Bypass dirty-tree protection only (cas-369f / cas-0b32).
    ///   Does **not** imply worktree removal.
    /// * `cleanup` - When true, remove the worktree directory and delete the
    ///   branch after a successful merge. When false, leave both intact so a
    ///   live factory worker can keep working mid-epic (cas-369f). Independent
    ///   of `force` and of `config.cleanup_on_close` — callers decide.
    ///
    /// # Returns
    /// The merge commit hash if successful, or None if merge was skipped
    pub fn merge_and_cleanup(
        &self,
        worktree: &mut Worktree,
        force: bool,
        cleanup: bool,
    ) -> WorktreeResult<Option<String>> {
        // Check for uncommitted changes (cas-006c: named-path classification,
        // not a raw "any porcelain output" check — see
        // GitOperations::classify_dirty_status).
        if !force {
            self.reject_or_warn_on_dirty(&worktree.path)?;
        }

        let merge_commit = if self.config.auto_merge {
            // Switch to parent branch in main repo
            self.git.checkout(&worktree.parent_branch)?;

            // Merge the worktree branch
            match self.git.merge_branch(&worktree.branch, true) {
                Ok(commit) => {
                    worktree.mark_merged(commit.clone());
                    commit
                }
                Err(GitError::MergeConflict) => {
                    worktree.mark_conflict();
                    return Err(WorktreeError::Git(GitError::MergeConflict));
                }
                Err(e) => return Err(WorktreeError::Git(e)),
            }
        } else {
            worktree.mark_abandoned();
            None
        };

        // cas-369f: force ≠ cleanup. Only remove when the caller explicitly
        // requested cleanup (or System-A path that passed config-driven true).
        //
        // cas-006c: always pass force=true to the low-level git removal
        // here. By this point either the caller explicitly forced past our
        // dirty-check gate above, or (force=false) `reject_or_warn_on_dirty`
        // already vetted the tree as safe (clean, or dirty only in
        // untracked/CAS-artifact paths). `git worktree remove` without
        // --force independently refuses on ANY untracked file though — the
        // same over-broad check this task exists to fix — so re-running it
        // here would silently reinstate the false-positive for a worktree
        // whose only dirty entry is `.husky/_/` and cleanup=true.
        if cleanup {
            self.git.remove_worktree(&worktree.path, true)?;

            // Delete the branch
            let _ = self.git.delete_branch(&worktree.branch, true);

            worktree.mark_removed();
        }

        Ok(merge_commit)
    }

    /// Abandon a worktree without merging
    pub fn abandon(&self, worktree: &mut Worktree, force: bool) -> WorktreeResult<()> {
        // Check for uncommitted changes (cas-006c: same named-path
        // classification as merge_and_cleanup).
        if !force {
            self.reject_or_warn_on_dirty(&worktree.path)?;
        }

        // Remove the worktree. cas-006c: pass force=true unconditionally —
        // by this point `reject_or_warn_on_dirty` already vetted the tree
        // (or the caller explicitly forced past it), so the low-level
        // `git worktree remove` must not independently re-block on
        // untracked-only debris it has no way to classify.
        self.git.remove_worktree(&worktree.path, true)?;

        // Delete the branch
        let _ = self.git.delete_branch(&worktree.branch, true);

        worktree.mark_abandoned();
        worktree.mark_removed();

        Ok(())
    }

    /// List all worktrees (git + CAS context)
    pub fn list_git_worktrees(&self) -> WorktreeResult<Vec<super::git::WorktreeInfo>> {
        Ok(self.git.list_worktrees()?)
    }

    /// Prune orphaned worktree references
    pub fn prune(&self) -> WorktreeResult<()> {
        Ok(self.git.prune_worktrees()?)
    }

    /// Get worktree info by path
    pub fn get_worktree_by_path(
        &self,
        path: &Path,
    ) -> WorktreeResult<Option<super::git::WorktreeInfo>> {
        let worktrees = self.git.list_worktrees()?;
        Ok(worktrees.into_iter().find(|wt| wt.path == path))
    }

    // =========================================================================
    // Factory Worker Methods
    // =========================================================================

    /// Get the base directory for worktrees
    pub fn worktree_root(&self) -> PathBuf {
        let project_name = self
            .repo_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");

        let base = self.config.base_path.replace("{project}", project_name);
        if base.starts_with('/') {
            PathBuf::from(base)
        } else {
            self.repo_root
                .parent()
                .unwrap_or(&self.repo_root)
                .join(base)
        }
    }

    /// Get the main repository root path
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    // Worker and epic operations are split into dedicated modules.
}

/// Symlink `.mcp.json` and `.claude/` from the main project into a worktree.
///
/// These files are typically gitignored (`.mcp.json` contains API keys, `.claude/`
/// contains local settings), so `git worktree add` doesn't check them out.
/// Without them, workers have no MCP server config and lose access to CAS tools.
///
/// Safe to call on worktrees where the files are already present (tracked in git):
/// existing paths are silently skipped.
pub fn symlink_project_config(repo_root: &Path, worktree_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        // .mcp.json — MCP server definitions (CAS, Context7, etc.)
        let mcp_src = repo_root.join(".mcp.json");
        let mcp_dst = worktree_path.join(".mcp.json");
        if mcp_src.exists() && !mcp_dst.exists() {
            let _ = symlink(&mcp_src, &mcp_dst);
        }

        // .claude/ — settings, permissions, skills, agents, hooks
        let claude_src = repo_root.join(".claude");
        let claude_dst = worktree_path.join(".claude");
        if claude_src.is_dir() && !claude_dst.exists() {
            let _ = symlink(&claude_src, &claude_dst);
        }
    }
}

/// Convert a title to a branch-safe slug
pub(super) fn slugify_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(50)
        .collect()
}

#[cfg(test)]
mod tests;
