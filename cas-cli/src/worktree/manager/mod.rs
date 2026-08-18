//! High-level worktree manager that integrates git operations with Cassy storage
//!
//! This module coordinates between git worktree operations and Cassy's epic/task system.
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

    /// cas-df97: live external symlinks (outside the worktree) resolve
    /// into it — removing the worktree would leave them dangling. Names
    /// every offending link so it's fixable without spelunking.
    #[error(
        "Worktree has live external symlinks pointing into it: {}",
        describe_external_symlinks(&.0.links)
    )]
    ExternalSymlinksDetected(worker_ops::ExternalSymlinkWarning),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// "link -> target" listing for [`WorktreeError::ExternalSymlinksDetected`].
fn describe_external_symlinks(links: &[crate::worktree::external_symlinks::ExternalSymlink]) -> String {
    links
        .iter()
        .map(|link| format!("{} -> {}", link.link.display(), link.target.display()))
        .collect::<Vec<_>>()
        .join(", ")
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

pub use worker_ops::{CleanupReport, DirtyWorktreeWarning, ExternalSymlinkWarning, RemoveOutcome};

/// Where a merge into a target branch is executed (cas-4702 / GH #68).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeVenue {
    /// The main checkout is already on the target branch — merge in place.
    /// HEAD does not move, and the working tree stays consistent with the
    /// branch it tracks.
    SharedCheckout,
    /// The main checkout is on some other branch (or detached) — merge in an
    /// ephemeral detached worktree and advance the branch ref, so the main
    /// checkout's HEAD is never touched.
    TempWorktree,
}

/// Pure venue decision — unit-tested. `main_head_branch` is the main
/// checkout's current branch (`None` when detached or unresolvable).
pub(crate) fn decide_merge_venue(
    main_head_branch: Option<&str>,
    target_branch: &str,
) -> MergeVenue {
    match main_head_branch {
        Some(branch) if branch == target_branch => MergeVenue::SharedCheckout,
        _ => MergeVenue::TempWorktree,
    }
}

/// True when a dirty shared-checkout path is one this merge would write
/// (cas-4702 / GH #73).
///
/// Matches exact paths and directory containment in either direction: a
/// residue entry for `src/` covers a merge touching `src/lib.rs`, and residue
/// on `src/lib.rs` conflicts with a merge that rewrites `src/`.
fn path_intersects(residue_path: &str, merge_path: &str) -> bool {
    let residue = residue_path.trim_end_matches('/');
    let merge = merge_path.trim_end_matches('/');
    residue == merge
        || merge.starts_with(&format!("{residue}/"))
        || residue.starts_with(&format!("{merge}/"))
}

/// Subset of shared-checkout residue that intersects the merge's touched
/// paths. Empty means the merge and the residue are disjoint and the merge is
/// safe to run (cas-4702 / GH #73).
pub(crate) fn residue_overlapping_merge(
    residue: &[crate::hooks::handlers::session_hygiene::PorcelainEntry],
    merge_paths: &[String],
) -> Vec<crate::hooks::handlers::session_hygiene::PorcelainEntry> {
    residue
        .iter()
        .filter(|entry| {
            merge_paths
                .iter()
                .any(|merge_path| path_intersects(&entry.path, merge_path))
        })
        .cloned()
        .collect()
}

impl WorktreeManager {
    fn worker_ref(&self, worker_name: &str) -> WorktreeResult<&Worktree> {
        self.workers
            .get(worker_name)
            .ok_or_else(|| WorktreeError::NotFound(worker_name.to_string()))
    }

    /// Resolve the merge venue for `target_branch` against the main
    /// checkout's live HEAD (cas-4702 / GH #68). A detached or unresolvable
    /// HEAD, or one on any other branch, means the merge must not run here.
    pub(crate) fn merge_venue(&self, target_branch: &str) -> MergeVenue {
        let head = self
            .git
            .current_branch()
            .ok()
            .filter(|branch| branch != "HEAD");
        decide_merge_venue(head.as_deref(), target_branch)
    }

    /// Shared dirty-check gate for force-free merge/removal (cas-006c).
    ///
    /// Always blocks on tracked modified/added/deleted paths, naming each
    /// with its status. Cassy-generated artifacts (`.husky/_/`) never block.
    ///
    /// Untracked paths are handled according to `will_remove`, per a
    /// supervisor review finding on the first cut of this fix: a merge that
    /// leaves the worktree directory in place cannot lose data git never
    /// tracked, so untracked-only dirt is warning-only (`tracing::warn!`,
    /// named, non-blocking). But `git worktree remove` deletes the
    /// directory outright — an uncommitted, untracked file exists nowhere
    /// else, so removal must block on it exactly like a tracked change.
    /// Pass `will_remove = true` from any caller that is about to delete
    /// the worktree directory (merge_and_cleanup with cleanup=true,
    /// abandon, remove_worker); pass `false` only when the worktree is
    /// merged but preserved (cleanup=false).
    fn reject_or_warn_on_dirty(&self, path: &Path, will_remove: bool) -> WorktreeResult<()> {
        let dirty = self.git.classify_dirty_status(path)?;

        if will_remove && !dirty.warnings.is_empty() {
            let mut message = dirty.describe_blocking();
            if !message.is_empty() {
                message.push_str(", ");
            }
            message.push_str(&dirty.describe_warnings());
            return Err(WorktreeError::UncommittedChanges(message));
        }

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
        // accidentally commit Cassy-synced changes (rules, skills, settings).
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
        let merge_commit = self.merge_preserving_worktree(worktree, force, cleanup)?;
        if cleanup {
            self.cleanup_merged_worktree(worktree)?;
        }
        Ok(merge_commit)
    }

    /// Merge while deliberately preserving the source worktree.
    ///
    /// `will_cleanup` controls dirty-tree validation exactly as it does for
    /// [`Self::merge_and_cleanup`], but removal is deferred to
    /// [`Self::cleanup_merged_worktree`]. Transactional delivery uses this
    /// split so its ancestry-gated post-merge state can be durable before the
    /// source worktree is removed.
    pub(crate) fn merge_preserving_worktree(
        &self,
        worktree: &mut Worktree,
        force: bool,
        will_cleanup: bool,
    ) -> WorktreeResult<Option<String>> {
        // cas-4702 / GH #68: where the merge runs decides everything below.
        // `SharedCheckout` (the main checkout is already on the target branch)
        // writes the shared working tree; `TempWorktree` runs in an ephemeral
        // detached worktree and only moves the branch ref, so the shared
        // checkout's HEAD, index and working tree are never touched — and its
        // residue is therefore irrelevant.
        let venue = self.merge_venue(&worktree.parent_branch);

        if self.config.auto_merge && venue == MergeVenue::SharedCheckout {
            // cas-e18f/cas-09f2: inspect the shared merge point before even
            // evaluating the requested source worktree. Residue from an
            // earlier operation is the primary failure and must never be
            // misreported as belonging to the branch requested now.
            if self.git.merge_in_progress() {
                return Err(WorktreeError::Git(GitError::MergeInProgress(
                    self.git.describe_merge_in_progress(),
                )));
            }

            // MERGE_HEAD is not the only residue that can poison the shared
            // checkout. A staged or modified tracked path makes the next
            // merge fail for reasons belonging to an earlier operation.
            // `force` intentionally does not bypass this gate; it applies
            // only to the source worktree.
            //
            // cas-4702 / GH #73: the gate is scoped to the paths this merge
            // would actually touch. Operator residue elsewhere in a shared
            // checkout (stray `.claude/` edits, unrelated scripts) is none
            // of this merge's business and must not refuse it.
            let target_dirty = self.git.classify_dirty_status(&self.repo_root)?;
            if target_dirty.is_blocked() {
                let merge_paths = self
                    .git
                    .merge_touched_paths(&worktree.parent_branch, &worktree.branch);
                let blocking = target_dirty.blocking.clone();
                let conflicting = match merge_paths {
                    Ok(paths) => residue_overlapping_merge(&blocking, &paths),
                    // Safe fallback: if the touched-path set can't be
                    // computed (unresolvable refs, git failure), fall back to
                    // the historical conservative behaviour and treat all
                    // residue as conflicting rather than merging blind.
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "could not compute merge-touched paths; falling back to \
                             unscoped shared-checkout residue refusal"
                        );
                        blocking.clone()
                    }
                };

                if !conflicting.is_empty() {
                    return Err(WorktreeError::Git(GitError::MergeCheckoutDirty(
                        crate::worktree::git::DirtyClassification {
                            blocking: conflicting,
                            warnings: Vec::new(),
                        }
                        .describe_blocking(),
                    )));
                }

                tracing::warn!(
                    residue = %target_dirty.describe_blocking(),
                    "shared checkout has tracked residue that does not intersect the \
                     merge; proceeding without touching it"
                );
            }
        }

        // Check for uncommitted changes (cas-006c: named-path classification,
        // not a raw "any porcelain output" check — see
        // GitOperations::classify_dirty_status). will_remove=will_cleanup: when
        // this merge also removes the worktree directory, untracked files
        // must block too (removal destroys them outright); when the
        // worktree survives (cleanup=false) they only warn.
        if !force {
            self.reject_or_warn_on_dirty(&worktree.path, will_cleanup)?;
        }

        let merge_commit = if self.config.auto_merge {
            // cas-e18f (fix b+c): pre-flight with `git merge-tree
            // --write-tree`, which computes the merge purely in-memory —
            // it never touches the working tree or index. A conflicting
            // merge is refused here, before `checkout`/`merge` run at all,
            // so the failing case never puts the shared checkout in a
            // mid-merge state in the first place.
            let conflicts = self
                .git
                .preflight_merge_conflicts(&worktree.parent_branch, &worktree.branch)
                .map_err(WorktreeError::Git)?;
            if !conflicts.is_empty() {
                worktree.mark_conflict();
                return Err(WorktreeError::Git(GitError::MergeConflictPaths(conflicts)));
            }

            // cas-4702 / GH #68: never move the main checkout's HEAD. When
            // the checkout already happens to be on the target branch the
            // merge runs in place (no checkout needed, working tree stays in
            // sync); otherwise it runs in an ephemeral detached worktree and
            // the branch ref is advanced by compare-and-swap, so the
            // supervisor's next commit still lands where they were.
            let merge_result = match self.merge_venue(&worktree.parent_branch) {
                MergeVenue::SharedCheckout => {
                    self.git
                        .merge_branch(&worktree.parent_branch, &worktree.branch, true)
                }
                MergeVenue::TempWorktree => self.git.merge_branch_via_temp_worktree(
                    &worktree.parent_branch,
                    &worktree.branch,
                    true,
                ),
            };

            // The pre-flight above should make the conflicting case
            // unreachable, but the merge itself still aborts-on-failure
            // (cas-e18f fix a) as a safety net — e.g. a conflict introduced
            // by a concurrent change between pre-flight and this call, or
            // anything merge-tree doesn't model identically to a real merge.
            match merge_result {
                Ok(commit) => {
                    worktree.mark_merged(commit.clone());
                    commit
                }
                Err(e @ (GitError::MergeConflict | GitError::MergeConflictPaths(_))) => {
                    worktree.mark_conflict();
                    return Err(WorktreeError::Git(e));
                }
                // cas-4702: git's own working-tree guard fired in the shared
                // checkout — residue the scoped gate let through (e.g. a
                // staged add of a path the merge result does not contain)
                // still cannot be checked out over. Report it as residue,
                // with git's own path list, not as an opaque command failure.
                Err(GitError::CommandFailed(details))
                    if details.contains("would be overwritten by merge") =>
                {
                    return Err(WorktreeError::Git(GitError::MergeCheckoutDirty(details)));
                }
                Err(e) => return Err(WorktreeError::Git(e)),
            }
        } else {
            worktree.mark_abandoned();
            None
        };

        Ok(merge_commit)
    }

    /// Remove a successfully merged worktree after any caller-owned durable
    /// state has been committed.
    ///
    /// The dirty-tree decision belongs to `merge_preserving_worktree`; this
    /// method is intentionally only the destructive half of the existing
    /// merge-and-cleanup operation.
    pub(crate) fn cleanup_merged_worktree(&self, worktree: &mut Worktree) -> WorktreeResult<()> {
        // cas-006c: pass force=true to the low-level git removal here. The
        // merge preflight already vetted the tree with `will_cleanup=true`,
        // or the caller explicitly forced past that gate.
        self.git.remove_worktree(&worktree.path, true)?;

        // Delete the branch.
        let _ = self.git.delete_branch(&worktree.branch, true);

        worktree.mark_removed();
        Ok(())
    }

    /// Abandon a worktree without merging
    pub fn abandon(&self, worktree: &mut Worktree, force: bool) -> WorktreeResult<()> {
        // Check for uncommitted changes (cas-006c: same named-path
        // classification as merge_and_cleanup). will_remove=true always —
        // abandon unconditionally deletes the worktree directory, so
        // untracked files must block exactly like tracked ones.
        if !force {
            self.reject_or_warn_on_dirty(&worktree.path, true)?;
        }

        // Remove the worktree. cas-006c: pass force=true unconditionally —
        // by this point `reject_or_warn_on_dirty(path, true)` already
        // vetted the tree as safe to remove (blocking on untracked too),
        // or the caller explicitly forced past it. The low-level
        // `git worktree remove` therefore has nothing left to independently
        // catch that our gate hasn't already covered.
        self.git.remove_worktree(&worktree.path, true)?;

        // Delete the branch
        let _ = self.git.delete_branch(&worktree.branch, true);

        worktree.mark_abandoned();
        worktree.mark_removed();

        Ok(())
    }

    /// List all worktrees (git + Cassy context)
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

/// Symlink gitignored project support files from the main project into a worktree.
///
/// These files are typically gitignored (`.mcp.json` contains API keys, `.claude/`
/// and `.codex/` contain local settings), so `git worktree add` doesn't check
/// them out. The pinned `.context/zig/` toolchain is likewise gitignored. Without
/// these links, workers lose Cassy configuration, Codex's native hook policy, or
/// the compiler required by the vendored Ghostty build.
///
/// Safe to call on worktrees where the files are already present (tracked in git):
/// existing paths are silently skipped.
pub fn symlink_project_config(repo_root: &Path, worktree_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        // .mcp.json — MCP server definitions (Cassy, Context7, etc.)
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

        // .codex/ — project MCP config plus Cassy-native hooks. This must be the
        // same source path as the project copy: Codex records hook trust by the
        // absolute hooks.json source path, so copying would create an untrusted
        // second hook identity for every factory worktree.
        let codex_src = repo_root.join(".codex");
        let codex_dst = worktree_path.join(".codex");
        if codex_src.is_dir() && !codex_dst.exists() {
            let _ = symlink(&codex_src, &codex_dst);
        }

        // .context/zig/ — the pinned Zig toolchain used by ghostty_vt_sys.
        // Link only the toolchain rather than all of .context/, whose other
        // contents are not necessarily safe or useful to share with workers.
        let zig_src = repo_root.join(".context").join("zig");
        let zig_dst = worktree_path.join(".context").join("zig");
        if zig_src.is_dir() && !zig_dst.exists() {
            if let Some(context_dst) = zig_dst.parent() {
                let _ = std::fs::create_dir_all(context_dst);
            }
            let _ = symlink(&zig_src, &zig_dst);
        }
    }
}

/// Return the dependency setup instruction an isolated Node worktree needs
/// before its first JS/TS command.
///
/// `node_modules` deliberately is not linked from the primary checkout: unlike
/// the pinned `.context/zig` compiler, dependency trees vary with the branch's
/// lockfile and may contain native artifacts tied to the install path. Running
/// an install during worktree creation would also put network and package-cache
/// contention on the concurrent spawn path. Instead, the worker receives this
/// branch-local command in its task brief before it starts work.
pub fn node_modules_setup_instruction(worktree_path: &Path) -> Option<String> {
    if !worktree_path.join("package.json").is_file() || worktree_path.join("node_modules").exists()
    {
        return None;
    }

    let command = if worktree_path.join("package-lock.json").is_file()
        || worktree_path.join("npm-shrinkwrap.json").is_file()
    {
        "npm ci"
    } else if worktree_path.join("pnpm-lock.yaml").is_file() {
        "pnpm install --frozen-lockfile"
    } else if worktree_path.join("yarn.lock").is_file() {
        if worktree_path.join(".yarnrc.yml").is_file() {
            "yarn install --immutable"
        } else {
            "yarn install --frozen-lockfile"
        }
    } else if worktree_path.join("bun.lock").is_file() || worktree_path.join("bun.lockb").is_file()
    {
        "bun install --frozen-lockfile"
    } else {
        let manager = std::fs::read_to_string(worktree_path.join("package.json"))
            .ok()
            .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
            .and_then(|manifest| {
                manifest
                    .get("packageManager")
                    .and_then(serde_json::Value::as_str)
                    .map(|value| value.split('@').next().unwrap_or_default().to_string())
            });
        match manager.as_deref() {
            Some("pnpm") => "pnpm install",
            Some("yarn") => "yarn install",
            Some("bun") => "bun install",
            _ => "npm install",
        }
    };

    Some(format!(
        "Isolated worktree dependency prerequisite: this worktree has package.json but no \
         node_modules. Before running any JS/TS test or build command, run `{command}` in this \
         worktree. Cassy intentionally does not share node_modules across worktrees because each \
         branch may require lockfile- and path-specific dependencies."
    ))
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
