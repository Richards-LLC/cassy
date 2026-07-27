//! Low-level git operations for worktree management
//!
//! This module provides a safe wrapper around git commands for worktree operations.
//! It's independent of CAS storage - purely git operations.

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

use crate::hooks::handlers::session_hygiene::{PorcelainEntry, porcelain_status};
use crate::types::GitContext;

mod branch_ops;

/// Errors that can occur during git operations
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git is not available: {0}")]
    GitNotAvailable(String),

    #[error("Not in a git repository")]
    NotAGitRepo,

    #[error("Failed to execute git command: {0}")]
    CommandFailed(String),

    #[error("Worktree already exists at {0}")]
    WorktreeExists(PathBuf),

    #[error("Worktree not found at {0}")]
    WorktreeNotFound(PathBuf),

    #[error("Branch already exists: {0}")]
    BranchExists(String),

    #[error("Branch not found: {0}")]
    BranchNotFound(String),

    #[error("Merge conflict detected")]
    MergeConflict,

    /// cas-e18f: names the conflicting paths instead of a bare
    /// "Failed to execute git command" — the underlying `git merge`
    /// failure was previously mis-detected (see `merge_branch`: conflict
    /// markers land on stdout, not stderr, so the old stderr-only check
    /// never matched and fell through to `CommandFailed` with an empty
    /// message). The working tree has already been restored via
    /// `git merge --abort` by the time this is returned.
    #[error("Merge conflict in: {}", .0.join(", "))]
    MergeConflictPaths(Vec<String>),

    /// cas-e18f: a `MERGE_HEAD` was already present on entry — a prior
    /// conflicting merge was never aborted and left the shared checkout
    /// mid-merge. Reported distinctly so this doesn't look like the
    /// unrelated merge that happens to run next (the exact cascade this
    /// task fixes).
    #[error(
        "A merge is already in progress in this repository (MERGE_HEAD present) — \
         a previous merge did not complete cleanly. Run `git merge --abort` (or resolve \
         and commit it) before retrying: {0}"
    )]
    MergeInProgress(String),

    #[error("Uncommitted changes in worktree")]
    UncommittedChanges,

    #[error("Already inside a worktree at {0}")]
    AlreadyInWorktree(PathBuf),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for git operations
pub type Result<T> = std::result::Result<T, GitError>;

/// Status of a worktree's uncommitted/unmerged state
#[derive(Debug, Clone)]
pub struct WorktreeDirtyStatus {
    /// Number of modified/staged files
    pub modified_count: usize,
    /// Number of untracked files
    pub untracked_count: usize,
    /// Number of commits not merged to target branch
    pub unmerged_count: usize,
    /// Target branch for unmerged check (e.g., epic branch)
    pub target_branch: Option<String>,
}

impl WorktreeDirtyStatus {
    /// Check if the worktree is clean
    pub fn is_clean(&self) -> bool {
        self.modified_count == 0 && self.untracked_count == 0 && self.unmerged_count == 0
    }

    /// Format as human-readable message
    pub fn to_message(&self) -> String {
        let mut parts = Vec::new();

        if self.modified_count > 0 {
            parts.push(format!("{} modified file(s)", self.modified_count));
        }
        if self.untracked_count > 0 {
            parts.push(format!("{} untracked file(s)", self.untracked_count));
        }
        if self.unmerged_count > 0 {
            if let Some(ref branch) = self.target_branch {
                parts.push(format!(
                    "{} commit(s) not merged to {}",
                    self.unmerged_count, branch
                ));
            } else {
                parts.push(format!("{} unmerged commit(s)", self.unmerged_count));
            }
        }

        if parts.is_empty() {
            "clean".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Classification of a worktree's `git status --porcelain` entries for
/// merge/removal dirty-check gating (cas-006c). See
/// [`GitOperations::classify_dirty_status`] for the split rules.
#[derive(Debug, Clone, Default)]
pub struct DirtyClassification {
    /// Tracked modified/added/deleted paths — block a force-free operation.
    pub blocking: Vec<PorcelainEntry>,
    /// Untracked paths — surfaced but never block.
    pub warnings: Vec<PorcelainEntry>,
}

impl DirtyClassification {
    /// True if any tracked change blocks a force-free merge/removal.
    pub fn is_blocked(&self) -> bool {
        !self.blocking.is_empty()
    }

    /// "label path" listing of blocking entries, for error messages that
    /// must name the offending paths (cas-006c AC2).
    pub fn describe_blocking(&self) -> String {
        Self::describe(&self.blocking)
    }

    /// "label path" listing of warning-only (untracked) entries.
    pub fn describe_warnings(&self) -> String {
        Self::describe(&self.warnings)
    }

    fn describe(entries: &[PorcelainEntry]) -> String {
        entries
            .iter()
            .map(|entry| format!("{} {}", entry.label(), entry.path))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// True for paths CAS itself generates inside every worktree that must
/// never gate a dirty check — currently just the `.husky/_` git-hooks shim
/// the worker startup hook creates (cas-006c). Matches the directory itself
/// and anything nested under it, with or without a trailing slash.
fn is_cas_generated_artifact(path: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    trimmed == ".husky/_" || trimmed.starts_with(".husky/_/")
}

/// Result of resolving a branch-creation base against its remote tip
/// (cas-b082). `create_branch_from`-style callers should branch from
/// `branch_ref`, not the raw base name, so a stale local base never
/// silently anchors a new epic/worker branch.
#[derive(Debug, Clone)]
pub struct ResolvedBase {
    /// The ref actually used as the branch-creation start point — either
    /// `origin/<base>` (remote tip, when reachable) or the bare local
    /// `<base>` (offline / no remote / remote ref missing).
    pub branch_ref: String,
    /// Resolved SHA of `branch_ref` at resolution time (empty string if it
    /// could not be resolved, e.g. base branch doesn't exist locally yet).
    pub sha: String,
    /// Commits the local `<base>` branch was behind `origin/<base>` at
    /// resolution time. Can be nonzero even when `used_remote` is false —
    /// on true divergence (local carries commits origin lacks AND origin
    /// carries commits local lacks) the local ref is preferred to avoid
    /// silently dropping the caller's own commits, but the origin-only
    /// commits this represents are still worth surfacing (cas-0938).
    pub behind_count: u32,
    /// Commits the local `<base>` branch was ahead of `origin/<base>` at
    /// resolution time (unpushed local-only commits). Always 0 when
    /// `used_remote` is true (cas-0938 — resolve_fresh_base previously took
    /// `origin/<base>` unconditionally whenever it existed, silently
    /// dropping these on a local-ahead or diverged base).
    pub ahead_count: u32,
    /// Whether `branch_ref` points at the fetched remote tracking branch
    /// (true) or fell back to the local branch (false — offline, no
    /// remote, remote ref missing, OR local carries commits origin lacks).
    pub used_remote: bool,
}

/// Git operations wrapper
pub struct GitOperations {
    /// Path to the main repository root
    repo_root: PathBuf,
}

impl GitOperations {
    /// Create a new GitOperations instance for a repository
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }

    /// Detect the repository root from a path
    pub fn detect_repo_root(from: &Path) -> Result<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(from)
            .output()?;

        if !output.status.success() {
            return Err(GitError::NotAGitRepo);
        }

        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(path))
    }

    /// Check if git is available
    pub fn is_git_available() -> bool {
        Command::new("git")
            .args(["--version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get the current git context (branch, worktree info)
    pub fn get_context(from: &Path) -> Result<GitContext> {
        let mut context = GitContext::default();

        // Get current branch
        let branch_output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(from)
            .output()?;

        if branch_output.status.success() {
            context.branch = Some(
                String::from_utf8_lossy(&branch_output.stdout)
                    .trim()
                    .to_string(),
            );
        }

        // Check if we're in a worktree
        let wt_output = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(from)
            .output()?;

        if wt_output.status.success() {
            let common_dir = String::from_utf8_lossy(&wt_output.stdout)
                .trim()
                .to_string();

            let git_dir_output = Command::new("git")
                .args(["rev-parse", "--git-dir"])
                .current_dir(from)
                .output()?;

            if git_dir_output.status.success() {
                let git_dir = String::from_utf8_lossy(&git_dir_output.stdout)
                    .trim()
                    .to_string();

                // If git-dir and git-common-dir differ, we're in a worktree
                if git_dir != common_dir && git_dir != ".git" {
                    context.is_worktree = true;

                    // Get worktree path
                    let toplevel = Command::new("git")
                        .args(["rev-parse", "--show-toplevel"])
                        .current_dir(from)
                        .output()?;

                    if toplevel.status.success() {
                        context.worktree_path = Some(PathBuf::from(
                            String::from_utf8_lossy(&toplevel.stdout).trim(),
                        ));
                    }
                }

                context.git_dir = Some(PathBuf::from(common_dir));
            }
        }

        Ok(context)
    }

    /// Get the current branch name
    pub fn current_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check if the repository has any commits
    pub fn has_commits(&self) -> Result<bool> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()?;

        Ok(output.status.success())
    }

    /// Detect the default branch of the repository.
    ///
    /// Uses git's own mechanisms in priority order:
    /// 1. Remote origin HEAD (authoritative if remote exists)
    /// 2. `init.defaultBranch` config
    /// 3. Check common branch names that actually exist as refs
    /// 4. HEAD symref target
    /// 5. "main" as absolute last resort
    pub fn detect_default_branch(&self) -> String {
        // 1. Remote origin HEAD - most authoritative when a remote exists
        if let Ok(output) = Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .current_dir(&self.repo_root)
            .output()
        {
            if output.status.success() {
                let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(branch) = refname.strip_prefix("refs/remotes/origin/") {
                    if !branch.is_empty() {
                        return branch.to_string();
                    }
                }
            }
        }

        // 2. git config init.defaultBranch
        if let Ok(output) = Command::new("git")
            .args(["config", "init.defaultBranch"])
            .current_dir(&self.repo_root)
            .output()
        {
            if output.status.success() {
                let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !branch.is_empty() {
                    return branch;
                }
            }
        }

        // 3. Check common branch names that actually exist as refs. This must
        // precede HEAD so factory branch creation does not accidentally anchor
        // to the supervisor's current feature/epic branch in local-only repos.
        for candidate in &["main", "master", "develop", "trunk"] {
            if self.branch_exists(candidate).unwrap_or(false) {
                return candidate.to_string();
            }
        }

        // 4. HEAD symref - useful in empty repos or repos with an uncommon
        // default branch name and no configured/remote default.
        if let Ok(output) = Command::new("git")
            .args(["symbolic-ref", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
        {
            if output.status.success() {
                let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(branch) = refname.strip_prefix("refs/heads/") {
                    if !branch.is_empty() {
                        return branch.to_string();
                    }
                }
            }
        }

        // 5. Last resort
        "main".to_string()
    }

    /// Check if a branch exists
    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(&self.repo_root)
            .output()?;

        Ok(output.status.success())
    }

    /// Create a worktree with a new branch
    pub fn create_worktree(
        &self,
        path: &Path,
        branch: &str,
        base_branch: Option<&str>,
    ) -> Result<()> {
        // Check if path already exists
        if path.exists() {
            return Err(GitError::WorktreeExists(path.to_path_buf()));
        }

        // Check if branch already exists
        if self.branch_exists(branch)? {
            return Err(GitError::BranchExists(branch.to_string()));
        }

        // Validate base branch is a valid ref (catches empty repos with no commits)
        if let Some(base) = base_branch {
            if !self.branch_exists(base)? {
                return Err(GitError::CommandFailed(format!(
                    "Base branch '{base}' is not a valid reference. Does the repository have any commits? \
                     Try making an initial commit first."
                )));
            }
        }

        // Build command
        let mut args = vec!["worktree", "add"];

        let path_str = path.to_str().ok_or_else(|| {
            GitError::CommandFailed(format!("Path contains invalid UTF-8: {}", path.display()))
        })?;
        if let Some(base) = base_branch {
            args.extend(["-b", branch, path_str, base]);
        } else {
            args.extend(["-b", branch, path_str]);
        }

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        // Initialize submodules in the new worktree
        // This is necessary because git worktree add doesn't copy submodule contents
        self.init_submodules(path)?;

        Ok(())
    }

    /// Initialize git submodules in a directory
    ///
    /// This is necessary for worktrees because `git worktree add` doesn't
    /// automatically populate submodule contents. Without this, builds that
    /// depend on vendored submodules (like ghostty_vt_sys) will fail.
    ///
    /// Note: This function does not fail if submodule init fails, because the
    /// worktree is still usable for many tasks that don't require submodules.
    /// A clear error message will be shown if a build later requires the submodule.
    pub fn init_submodules(&self, path: &Path) -> Result<()> {
        // Check if there are any submodules configured
        let gitmodules = self.repo_root.join(".gitmodules");
        if !gitmodules.exists() {
            return Ok(()); // No submodules to initialize
        }

        tracing::info!("Initializing submodules in worktree: {}", path.display());

        let output = Command::new("git")
            .args(["submodule", "update", "--init", "--recursive"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Log warning but don't fail - submodule init may fail for various reasons
            // (network issues, etc.) but the worktree is still usable for many tasks.
            // If a build later requires the submodule, ghostty_vt_sys/build.rs will
            // provide a clear error message with instructions.
            tracing::warn!(
                "Failed to initialize submodules in {}: {}",
                path.display(),
                stderr
            );
            eprintln!(
                "[CAS] Warning: Failed to initialize git submodules in worktree.\n\
                 [CAS] If you need to build components that depend on vendor/ghostty,\n\
                 [CAS] run: git submodule update --init --recursive\n\
                 [CAS] Error: {}",
                stderr.trim()
            );
        } else {
            tracing::info!("Submodules initialized successfully");
        }

        Ok(())
    }

    /// Get submodule paths from .gitmodules
    ///
    /// Parses the .gitmodules file to extract the paths of all configured submodules.
    pub fn get_submodule_paths(&self) -> Result<Vec<PathBuf>> {
        let gitmodules = self.repo_root.join(".gitmodules");
        if !gitmodules.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&gitmodules)?;
        let mut paths = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("path = ") || trimmed.starts_with("path=") {
                let path = trimmed
                    .trim_start_matches("path = ")
                    .trim_start_matches("path=")
                    .trim();
                paths.push(PathBuf::from(path));
            }
        }

        Ok(paths)
    }

    /// Fix symlinked submodules before merge operations
    ///
    /// Git merge fails when submodule paths are symbolic links with:
    /// "error: expected submodule path 'vendor/...' not to be a symbolic link"
    ///
    /// This function detects symlinked submodules in the given directory and replaces
    /// them with properly initialized submodules.
    pub fn fix_symlinked_submodules(&self, path: &Path) -> Result<()> {
        let submodule_paths = self.get_submodule_paths()?;
        if submodule_paths.is_empty() {
            return Ok(());
        }

        let mut fixed_any = false;
        for submodule in &submodule_paths {
            let full_path = path.join(submodule);
            if full_path.is_symlink() {
                tracing::info!(
                    "Removing symlinked submodule at {} for merge compatibility",
                    full_path.display()
                );

                // Remove the symlink
                if let Err(e) = std::fs::remove_file(&full_path) {
                    tracing::warn!("Failed to remove symlink {}: {}", full_path.display(), e);
                    continue;
                }

                fixed_any = true;
            }
        }

        // Re-initialize submodules if we removed any symlinks
        if fixed_any {
            tracing::info!("Re-initializing submodules after removing symlinks");
            self.init_submodules(path)?;
        }

        Ok(())
    }

    /// List all worktrees
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut worktrees = Vec::new();
        let mut current: Option<WorktreeInfo> = None;

        for line in stdout.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                if let Some(wt) = current.take() {
                    worktrees.push(wt);
                }
                current = Some(WorktreeInfo {
                    path: PathBuf::from(path),
                    branch: None,
                    commit: None,
                    is_bare: false,
                    is_detached: false,
                });
            } else if let Some(ref mut wt) = current {
                if let Some(commit) = line.strip_prefix("HEAD ") {
                    wt.commit = Some(commit.to_string());
                } else if let Some(branch) = line.strip_prefix("branch ") {
                    // Remove refs/heads/ prefix if present
                    wt.branch = Some(
                        branch
                            .strip_prefix("refs/heads/")
                            .unwrap_or(branch)
                            .to_string(),
                    );
                } else if line == "bare" {
                    wt.is_bare = true;
                } else if line == "detached" {
                    wt.is_detached = true;
                }
            }
        }

        if let Some(wt) = current {
            worktrees.push(wt);
        }

        Ok(worktrees)
    }

    /// Remove a worktree
    pub fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        if !path.exists() {
            return Err(GitError::WorktreeNotFound(path.to_path_buf()));
        }

        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        let path_str = path.to_str().ok_or_else(|| {
            GitError::CommandFailed(format!("Path contains invalid UTF-8: {}", path.display()))
        })?;
        args.push(path_str);

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("uncommitted changes") || stderr.contains("untracked files") {
                return Err(GitError::UncommittedChanges);
            }
            return Err(GitError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Delete a branch
    pub fn delete_branch(&self, branch: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };

        let output = Command::new("git")
            .args(["branch", flag, branch])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not found") {
                return Err(GitError::BranchNotFound(branch.to_string()));
            }
            return Err(GitError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Check whether a merge is already in progress in the main checkout
    /// (i.e. `MERGE_HEAD` exists). cas-e18f: a conflicting merge that
    /// wasn't aborted leaves this set, and the *next* merge — for an
    /// unrelated branch — fails with git's generic "you need to resolve
    /// your current index first", which describes the symptom of the
    /// previous failure rather than anything about the current one.
    /// Callers should check this before attempting a merge and report it
    /// distinctly rather than letting git's own error surface unexplained.
    pub fn merge_in_progress(&self) -> bool {
        Command::new("git")
            .args(["rev-parse", "--verify", "-q", "MERGE_HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Describe the in-progress merge for `MergeInProgress`'s error message
    /// (best-effort; falls back to a generic note if git status can't be
    /// read for any reason).
    pub fn describe_merge_in_progress(&self) -> String {
        Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(&self.repo_root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if stdout.is_empty() {
                    "no other changes reported".to_string()
                } else {
                    stdout
                }
            })
            .unwrap_or_else(|| "unable to read repository status".to_string())
    }

    /// Best-effort `git merge --abort`. A failed/conflicting merge must
    /// leave no trace in the shared checkout (cas-e18f) — this is called
    /// unconditionally after any merge failure. The result is intentionally
    /// discarded: if there was nothing to abort (e.g. the failure happened
    /// before git entered a merge state), `--abort` itself fails harmlessly.
    fn abort_merge_best_effort(&self) {
        let _ = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(&self.repo_root)
            .output();
    }

    /// Extract conflicting paths from combined merge output (stdout+stderr).
    /// Matches the common `CONFLICT (<kind>): ... in <path>` shape git
    /// prints for content/add-add conflicts, plus a fallback that keeps the
    /// whole CONFLICT line when a path can't be isolated — some conflict
    /// kinds (e.g. rename/delete) phrase the path differently and callers
    /// still want *something* actionable rather than silence.
    fn extract_conflict_paths(combined: &str) -> Vec<String> {
        let mut paths = Vec::new();
        for line in combined.lines() {
            let line = line.trim();
            if !line.starts_with("CONFLICT") {
                continue;
            }
            if let Some(idx) = line.rfind(" in ") {
                paths.push(line[idx + 4..].trim().to_string());
            } else {
                paths.push(line.to_string());
            }
        }
        paths
    }

    /// Pre-flight a merge without touching the working tree or index
    /// (cas-e18f, fix (b)+(c)). Uses `git merge-tree --write-tree` to
    /// compute the merge result purely in-memory; returns the conflicting
    /// paths (empty if the merge would succeed cleanly). Callers should
    /// refuse to run the real merge when this returns any paths, so the
    /// failing case never enters the working tree at all.
    pub fn preflight_merge_conflicts(&self, target: &str, source: &str) -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["merge-tree", "--write-tree", target, source])
            .current_dir(&self.repo_root)
            .output()?;

        if output.status.success() {
            return Ok(Vec::new());
        }

        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let paths = Self::extract_conflict_paths(&combined);
        if paths.is_empty() {
            // merge-tree failed for a reason other than a content conflict
            // (e.g. one of the refs doesn't exist) — surface it verbatim
            // rather than claiming a conflict with no paths.
            return Err(GitError::CommandFailed(combined.trim().to_string()));
        }
        Ok(paths)
    }

    /// Merge a branch into the current branch
    pub fn merge_branch(&self, branch: &str, no_ff: bool) -> Result<Option<String>> {
        // cas-e18f: a merge left over from a previous, un-aborted failure
        // must be reported distinctly, not surfaced as an opaque failure of
        // *this* (unrelated) merge attempt.
        if self.merge_in_progress() {
            return Err(GitError::MergeInProgress(self.describe_merge_in_progress()));
        }

        // Fix symlinked submodules before merge to avoid:
        // "error: expected submodule path 'vendor/...' not to be a symbolic link"
        self.fix_symlinked_submodules(&self.repo_root)?;

        let mut args = vec!["merge"];
        if no_ff {
            args.push("--no-ff");
        }
        args.push(branch);

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            // cas-e18f: git prints "CONFLICT ..." / "Automatic merge
            // failed" to STDOUT, not stderr — the previous stderr-only
            // check never matched a real conflict, so it fell through to
            // `CommandFailed` with an empty message (stderr is blank on a
            // content conflict). Check both streams.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}\n{stderr}");
            let is_conflict =
                combined.contains("CONFLICT") || combined.contains("Automatic merge failed");

            // cas-e18f fix (a): a failed merge must leave no trace. Abort
            // unconditionally before returning — this is what removes the
            // factory-wide cascade, regardless of which branch below fires.
            self.abort_merge_best_effort();

            if is_conflict {
                let paths = Self::extract_conflict_paths(&combined);
                if !paths.is_empty() {
                    return Err(GitError::MergeConflictPaths(paths));
                }
                return Err(GitError::MergeConflict);
            }
            return Err(GitError::CommandFailed(combined.trim().to_string()));
        }

        // Get the merge commit hash
        let commit_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_root)
            .output()?;

        if commit_output.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&commit_output.stdout)
                    .trim()
                    .to_string(),
            ))
        } else {
            Ok(None)
        }
    }

    /// Check if the worktree has uncommitted changes
    pub fn has_uncommitted_changes(&self, path: &Path) -> Result<bool> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(!output.stdout.is_empty())
    }

    /// Classify a worktree's dirty state for merge/removal gating (cas-006c).
    ///
    /// Splits `git status --porcelain` entries into two buckets:
    ///
    /// - `blocking`: tracked changes (modified/added/deleted) — a force-free
    ///   merge or removal can genuinely lose this work, so callers must
    ///   refuse.
    /// - `warnings`: untracked paths. Nothing git tracks can be destroyed by
    ///   a merge, and a stray untracked file is not lost work, so these are
    ///   surfaced but must never block.
    ///
    /// CAS-generated artifacts (currently the `.husky/_` git-hooks shim the
    /// worker startup hook creates in every worktree) are excluded from
    /// *both* buckets — the tool that creates that artifact must not refuse
    /// to merge or remove because of it.
    ///
    /// Reuses [`porcelain_status`] for parsing rather than a second
    /// porcelain parser.
    pub fn classify_dirty_status(&self, path: &Path) -> Result<DirtyClassification> {
        let entries = porcelain_status(path).ok_or_else(|| {
            GitError::CommandFailed(format!(
                "git status --porcelain=v1 failed in {}",
                path.display()
            ))
        })?;

        let mut classification = DirtyClassification::default();
        for entry in entries {
            if is_cas_generated_artifact(&entry.path) {
                continue;
            }
            if entry.is_untracked() {
                classification.warnings.push(entry);
            } else {
                classification.blocking.push(entry);
            }
        }
        Ok(classification)
    }

    /// Count uncommitted entries (modified, staged, and untracked) in the worktree.
    ///
    /// Treats untracked files the same as modified/staged ones — both block clean
    /// teardown. Returns 0 when the worktree is clean.
    pub fn uncommitted_file_count(&self, path: &Path) -> Result<usize> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let count = output
            .stdout
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .count();
        Ok(count)
    }

    /// Count commits in worktree HEAD that are not in target branch
    ///
    /// Returns the number of commits that exist on the worktree's current branch
    /// but not on the target branch (e.g., epic branch).
    pub fn unmerged_commit_count(
        &self,
        worktree_path: &Path,
        target_branch: &str,
    ) -> Result<usize> {
        let output = Command::new("git")
            .args(["rev-list", "--count", &format!("{target_branch}..HEAD")])
            .current_dir(worktree_path)
            .output()?;

        if !output.status.success() {
            // If branch doesn't exist or other error, return 0
            return Ok(0);
        }

        let count_str = String::from_utf8_lossy(&output.stdout);
        Ok(count_str.trim().parse().unwrap_or(0))
    }

    /// Get detailed dirty status of a worktree
    ///
    /// Returns a summary of uncommitted changes, untracked files, and unmerged commits.
    pub fn get_worktree_dirty_status(
        &self,
        worktree_path: &Path,
        target_branch: Option<&str>,
    ) -> Result<WorktreeDirtyStatus> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(worktree_path)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let status_output = String::from_utf8_lossy(&output.stdout);
        let mut modified_count = 0;
        let mut untracked_count = 0;

        for line in status_output.lines() {
            if line.starts_with("??") {
                untracked_count += 1;
            } else if !line.is_empty() {
                modified_count += 1;
            }
        }

        let unmerged_count = if let Some(branch) = target_branch {
            self.unmerged_commit_count(worktree_path, branch)
                .unwrap_or(0)
        } else {
            0
        };

        Ok(WorktreeDirtyStatus {
            modified_count,
            untracked_count,
            unmerged_count,
            target_branch: target_branch.map(|s| s.to_string()),
        })
    }

    /// Checkout a branch in the main repo
    pub fn checkout(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["checkout", branch])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Prune stale worktree references
    pub fn prune_worktrees(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }
}

/// Information about a git worktree
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Path to the worktree
    pub path: PathBuf,
    /// Branch checked out in the worktree (None if detached)
    pub branch: Option<String>,
    /// Current commit hash
    pub commit: Option<String>,
    /// Whether this is a bare worktree
    pub is_bare: bool,
    /// Whether HEAD is detached
    pub is_detached: bool,
}

#[cfg(test)]
#[path = "git_tests/tests.rs"]
mod tests;
