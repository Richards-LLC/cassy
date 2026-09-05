//! Low-level git operations for worktree management
//!
//! This module provides a safe wrapper around git commands for worktree operations.
//! It's independent of Cassy storage - purely git operations.

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

use crate::hooks::handlers::session_hygiene::{PorcelainEntry, porcelain_status};
use crate::types::GitContext;

pub(crate) mod branch_ops;

pub use branch_ops::{TargetPushOutcome, TargetReconcile};

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

    /// cas-09f2: tracked changes already present in the shared target
    /// checkout. This is distinct from source-worktree dirt and cannot be
    /// bypassed with `force`; attempting a merge on top would either fail
    /// opaquely or risk mixing unrelated staged work into the merge.
    #[error("The shared target checkout has pre-existing tracked changes and was not touched: {0}")]
    MergeCheckoutDirty(String),

    /// cas-9415: an in-place merge must name and validate its destination.
    /// Without this guard, `git merge <source>` commits to implicit HEAD, so
    /// another process switching the shared checkout after venue selection
    /// can contaminate an unrelated branch.
    #[error(
        "Refusing merge in {checkout}: checkout is on branch '{actual}', but the resolved merge target is '{expected}'"
    )]
    MergeTargetMismatch {
        checkout: PathBuf,
        expected: String,
        actual: String,
    },

    /// cas-4702: the ephemeral-worktree merge completed, but the target
    /// branch had moved since it was read, so the compare-and-swap that would
    /// have published the merge declined. A concurrent writer is never
    /// clobbered — the merge is simply discarded with the temp worktree.
    #[error(
        "target branch {branch} moved during the merge (expected {expected}, found {actual}) — \
         the merge was discarded rather than clobbering the concurrent update"
    )]
    TargetTipChanged {
        branch: String,
        expected: String,
        actual: String,
    },

    /// cas-0f04: the target branch is checked out in another linked worktree
    /// that holds uncommitted work.
    ///
    /// Advancing the ref would leave that checkout describing an ancestor of
    /// its own HEAD — `git status` there reports the merged content as staged
    /// deletions, and every later merge refuses. Git blocks `checkout` of one
    /// branch in two worktrees, but `update-ref` bypasses that protection and
    /// notifies nobody, so this refusal is the notification. It fires before
    /// the merge, so the uncommitted work is untouched.
    #[error(
        "target branch {branch} is checked out at {checkout} with uncommitted changes \
         ({change_count} path(s), first: {first_change}) — NO MERGE WAS ATTEMPTED. \
         Advancing the ref would strand that checkout at its current commit and turn the \
         merged content into phantom staged deletions there. That work exists nowhere \
         else: commit or stash it in {checkout} — do NOT reset or check it out over — then \
         retry. Target is still at {tip}."
    )]
    TargetCheckedOutDirty {
        branch: String,
        checkout: PathBuf,
        change_count: usize,
        first_change: String,
        tip: String,
    },

    /// cas-0f04: git could not be asked which worktrees hold the target
    /// branch. Without that inventory there is no basis for claiming the
    /// advance is safe for them, so the merge refuses instead of proceeding on
    /// an assumption.
    #[error(
        "cannot determine which checkouts hold {branch} ({reason}) — NO MERGE WAS ATTEMPTED. \
         Advancing the ref without that inventory could strand a checkout holding \
         uncommitted work. Resolve the git failure above, then retry."
    )]
    CheckoutInventoryUnavailable { branch: String, reason: String },

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

/// True for paths Cassy itself generates inside every worktree that must
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

/// Outcome of choosing the start point for a NEW epic branch when the
/// checkout may already be sitting on a prior epic branch (cas-a85e / GH #99).
///
/// Basing every epic branch on trunk (cas-dc28) is right when HEAD is some
/// incidental branch, but it strands work when HEAD is the *previous epic*
/// branch carrying commits trunk has never seen: the follow-on epic starts
/// empty and a worker checking it out can re-create or overwrite deliverables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicBaseChoice {
    /// Ref the new epic branch should actually be created from.
    pub base_ref: String,
    /// The checkout's branch at decision time, when it is a named branch
    /// other than the trunk itself (`None` for detached HEAD or trunk).
    pub head_branch: Option<String>,
    /// Commits reachable from HEAD but not from the trunk base.
    pub head_ahead: u32,
    /// Commits reachable from the trunk base but not from HEAD.
    pub head_behind: u32,
    /// Whether `base_ref` is HEAD's branch rather than the trunk base.
    pub used_head: bool,
    /// Unlanded `epic/*` branches already contained in the chosen base,
    /// trunk-first (cas-aae6 / GH #110). Empty unless the base is an epic
    /// branch that is itself stacked. The new epic sits on top of all of them,
    /// so this is the order they must land in.
    pub stacked_on: Vec<String>,
    /// Operator-facing sentence describing the decision, when there is
    /// anything to say. Always populated when HEAD is ahead of the base.
    pub notice: Option<String>,
}

impl EpicBaseChoice {
    /// Trunk base with nothing noteworthy about HEAD.
    pub(crate) fn plain(base_ref: impl Into<String>) -> Self {
        Self {
            base_ref: base_ref.into(),
            head_branch: None,
            head_ahead: 0,
            head_behind: 0,
            used_head: false,
            stacked_on: Vec::new(),
            notice: None,
        }
    }
}

/// What happened to a linked checkout of the target after the ref moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckoutRefresh {
    /// Index and working tree were advanced to the new tip.
    Updated,
    /// Left exactly as it was, because advancing it would have overwritten
    /// something. Carries git's own reason.
    LeftStale { reason: String },
}

/// The branch a checkout currently has out, if it is on one at all.
fn head_branch_of(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

/// Twelve characters of a commit id — enough to identify it in a receipt.
fn short_tip(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// A worktree that currently has a given branch checked out (cas-0f04).
#[derive(Debug, Clone)]
struct LinkedCheckout {
    path: PathBuf,
}

/// Uncommitted work in `dir`, as `(count, first path)`.
///
/// Untracked files count: this decides whether advancing a ref would strand
/// somebody's work, and an untracked file exists nowhere else.
fn dirty_summary(dir: &Path) -> Option<(usize, String)> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines().filter(|line| !line.trim().is_empty()).peekable();
    let first = (*lines.peek()?).trim().to_string();
    Some((text.lines().filter(|l| !l.trim().is_empty()).count(), first))
}

/// Monotonic suffix so two ephemeral merge worktrees created inside the same
/// nanosecond still get distinct paths.
static TEMP_WORKTREE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// RAII handle for the ephemeral detached worktree used to merge without
/// touching the main checkout (cas-4702). Removal is best-effort on drop:
/// a leaked directory is recoverable with `git worktree prune`, whereas
/// failing the merge because cleanup hiccuped is not what the caller asked
/// for.
struct TempWorktree {
    repo_root: PathBuf,
    path: PathBuf,
}

impl TempWorktree {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        let removed = Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ])
            .current_dir(&self.repo_root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !removed {
            let _ = std::fs::remove_dir_all(&self.path);
            let _ = Command::new("git")
                .args(["worktree", "prune"])
                .current_dir(&self.repo_root)
                .output();
        }
    }
}

/// Git operations wrapper
pub struct GitOperations {
    /// Path to the main repository root
    repo_root: PathBuf,
    /// cas-0f04: linked checkouts of a merge target that were deliberately
    /// left untouched, recorded so the operator's receipt can say so.
    ///
    /// A `tracing::warn!` is not delivery: the MCP caller sees a receipt, not
    /// this process's logs, and reporting an ordinary success while a
    /// checkout is stranded is the original defect at the user boundary.
    /// Drained by [`Self::take_stale_checkout_notes`] when the receipt is
    /// assembled.
    stale_checkout_notes: std::sync::Mutex<Vec<String>>,
}

impl GitOperations {
    /// Create a new GitOperations instance for a repository
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            stale_checkout_notes: std::sync::Mutex::new(Vec::new()),
        }
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

    /// Canonical identity of the repository this instance operates on
    /// (cas-0a21).
    ///
    /// Resolves the git *common* directory — shared by the main checkout and
    /// every linked worktree — and canonicalizes it so symlinked or
    /// differently-spelled paths to one repository collapse to one identity.
    /// Two `GitOperations` pointed at the same repository through different
    /// paths therefore serialize against each other, while genuinely distinct
    /// repositories never contend.
    pub fn canonical_repo_key(&self) -> Result<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .current_dir(&self.repo_root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if raw.is_empty() {
            return Err(GitError::NotAGitRepo);
        }
        let path = PathBuf::from(raw);
        // Canonicalize when possible; a non-canonicalizable path is still a
        // usable (if less aggressive) identity, so don't fail the merge.
        Ok(path.canonicalize().unwrap_or(path))
    }

    /// Resolve `refname` to a full commit SHA, or `None` when it does not
    /// resolve. Used as the compare-and-swap read of the delivery target.
    pub fn resolve_commit(&self, refname: &str) -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("{refname}^{{commit}}")])
            .current_dir(&self.repo_root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!sha.is_empty()).then_some(sha)
    }

    /// First parent of `commit`, i.e. the commit the merge was actually
    /// rooted at. `None` for a root commit or an unresolvable ref.
    ///
    /// Ancestry is not sufficient to prove a delivery merged the reviewed
    /// target: a merge that swept in concurrent commits still leaves the
    /// receipt commit an ancestor of the new tip. First-parent identity is
    /// the invariant that actually pins the topology (cas-0a21).
    pub fn first_parent(&self, commit: &str) -> Option<String> {
        self.resolve_commit(&format!("{commit}^1"))
    }

    /// Expand `refname` to its fully-qualified form (`main` ->
    /// `refs/heads/main`).
    ///
    /// `git update-ref` performs **no** DWIM expansion: passing a bare branch
    /// name creates a literal `.git/<name>` ref instead of updating the
    /// branch. Every compare-and-swap must therefore qualify the ref first.
    pub fn full_ref_name(&self, refname: &str) -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--symbolic-full-name", refname])
            .current_dir(&self.repo_root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let full = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!full.is_empty()).then_some(full)
    }

    /// Atomically move `refname` from `expected_old` to `new_value` using
    /// git's own compare-and-swap (`update-ref <ref> <new> <old>`).
    ///
    /// Git rejects the update if the ref is not exactly at `expected_old`, so
    /// this can never clobber a concurrent writer. `refname` is qualified
    /// first — see [`Self::full_ref_name`].
    pub fn compare_and_swap_ref(
        &self,
        refname: &str,
        new_value: &str,
        expected_old: &str,
    ) -> Result<()> {
        let qualified = self
            .full_ref_name(refname)
            .unwrap_or_else(|| refname.to_string());
        let output = Command::new("git")
            .args(["update-ref", &qualified, new_value, expected_old])
            .current_dir(&self.repo_root)
            .output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    /// Roll `branch` back from `expected_tip` to `new_tip`, atomically and
    /// without leaving the shared checkout dirty (cas-0a21).
    ///
    /// Used to undo a delivery merge that Git completed but that turned out
    /// not to be rooted at the reviewed target. The ref move is a
    /// compare-and-swap, so a third writer that moved the ref again is never
    /// clobbered — the rollback simply declines.
    ///
    /// When HEAD is attached to `branch` (it is, right after the merge
    /// checkout), moving the ref alone would leave the index and working tree
    /// describing the discarded merge and poison the *next* merge's
    /// dirty-tree gate. The index/worktree are therefore realigned to the
    /// rolled-back HEAD.
    pub fn rollback_branch_to(
        &self,
        branch: &str,
        new_tip: &str,
        expected_tip: &str,
    ) -> Result<()> {
        self.compare_and_swap_ref(branch, new_tip, expected_tip)?;

        let head_ref = self.full_ref_name("HEAD");
        let branch_ref = self.full_ref_name(branch);
        let head_is_on_branch = matches!(
            (head_ref.as_deref(), branch_ref.as_deref()),
            (Some(head), Some(target)) if head == target
        );
        if head_is_on_branch {
            let output = Command::new("git")
                .args(["reset", "--hard", "HEAD"])
                .current_dir(&self.repo_root)
                .output()?;
            if !output.status.success() {
                return Err(GitError::CommandFailed(
                    String::from_utf8_lossy(&output.stderr).trim().to_string(),
                ));
            }
        }
        Ok(())
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

        // Resolve before `git worktree add -b` writes refs/heads/<branch>.
        // This is the worktree-creation sibling of `create_branch_from`: an
        // unresolved or corrupt name must never reach a ref-writing command.
        let start_point = base_branch.unwrap_or("HEAD");
        let start_sha = self.resolve_commit(start_point).ok_or_else(|| {
            GitError::CommandFailed(format!(
                "Refusing to create worktree branch '{branch}': start point '{start_point}' does not resolve to a commit"
            ))
        })?;

        // Build command
        let mut args = vec!["worktree", "add"];

        let path_str = path.to_str().ok_or_else(|| {
            GitError::CommandFailed(format!("Path contains invalid UTF-8: {}", path.display()))
        })?;
        args.extend(["-b", branch, path_str, &start_sha]);

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let branch_ref = format!("refs/heads/{branch}");
        let written_sha = self.resolve_commit(&branch_ref).ok_or_else(|| {
            GitError::CommandFailed(format!(
                "Created worktree branch '{branch}', but its ref '{branch_ref}' does not resolve to a commit"
            ))
        })?;
        if written_sha != start_sha {
            return Err(GitError::CommandFailed(format!(
                "Created worktree branch '{branch}', but post-verification found {written_sha} instead of expected {start_sha}"
            )));
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
                "[Cassy] Warning: Failed to initialize git submodules in worktree.\n\
                 [Cassy] If you need to build components that depend on vendor/ghostty,\n\
                 [Cassy] run: git submodule update --init --recursive\n\
                 [Cassy] Error: {}",
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
        Self::merge_in_progress_in(&self.repo_root)
    }

    /// `merge_in_progress` for an arbitrary checkout directory (main
    /// checkout, linked worktree, or the ephemeral merge worktree used by
    /// [`Self::merge_branch_via_temp_worktree`]).
    fn merge_in_progress_in(dir: &Path) -> bool {
        Command::new("git")
            .args(["rev-parse", "--verify", "-q", "MERGE_HEAD"])
            .current_dir(dir)
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
    fn abort_merge_best_effort_in(dir: &Path) {
        let _ = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(dir)
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

    /// Merge `source_branch` into `target_branch` in the main checkout.
    ///
    /// Git's merge command always writes implicit HEAD. Validate the live
    /// symbolic HEAD at the last helper boundary before invoking it, rather
    /// than trusting an earlier venue decision that another checkout process
    /// can invalidate (cas-9415).
    pub fn merge_branch(
        &self,
        target_branch: &str,
        source_branch: &str,
        no_ff: bool,
    ) -> Result<Option<String>> {
        let repo_root = self.repo_root.clone();
        self.merge_branch_in_dir(&repo_root, Some(target_branch), source_branch, no_ff)
    }

    /// Refuse an in-place merge unless `dir`'s symbolic HEAD is exactly the
    /// resolved target branch. Detached HEAD is reported as a mismatch too.
    fn ensure_merge_target_checked_out(&self, dir: &Path, expected: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(dir)
            .output()?;
        let actual = if output.status.success() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            "<detached HEAD>".to_string()
        };

        if actual != expected {
            return Err(GitError::MergeTargetMismatch {
                checkout: dir.to_path_buf(),
                expected: expected.to_string(),
                actual,
            });
        }
        Ok(())
    }

    /// Merge `branch` into whatever `dir`'s HEAD points at.
    ///
    /// `dir` is the main checkout for the in-place path and the ephemeral
    /// detached worktree for [`Self::merge_branch_via_temp_worktree`]
    /// (cas-4702 / GH #68) — the merge mechanics, conflict detection and
    /// abort-on-failure guarantees are identical either way.
    fn merge_branch_in_dir(
        &self,
        dir: &Path,
        expected_target: Option<&str>,
        branch: &str,
        no_ff: bool,
    ) -> Result<Option<String>> {
        // cas-e18f: a merge left over from a previous, un-aborted failure
        // must be reported distinctly, not surfaced as an opaque failure of
        // *this* (unrelated) merge attempt.
        if Self::merge_in_progress_in(dir) {
            return Err(GitError::MergeInProgress(self.describe_merge_in_progress()));
        }

        // Fix symlinked submodules before merge to avoid:
        // "error: expected submodule path 'vendor/...' not to be a symbolic link"
        self.fix_symlinked_submodules(dir)?;

        // This is intentionally the final operation before `git merge`.
        // Shared-checkout callers must revalidate live symbolic HEAD here;
        // dedicated detached merge worktrees pass no branch expectation.
        if let Some(target) = expected_target {
            self.ensure_merge_target_checked_out(dir, target)?;
        }

        let mut args = vec!["merge"];
        if no_ff {
            args.push("--no-ff");
        }
        args.push(branch);

        let output = Command::new("git").args(&args).current_dir(dir).output()?;

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
            Self::abort_merge_best_effort_in(dir);

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
            .current_dir(dir)
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

    /// Paths a merge of `source` into `target` would actually touch
    /// (cas-4702 / GH #73).
    ///
    /// Uses the three-dot diff — everything changed on `source` since the
    /// merge base with `target` — which is exactly the set of paths the
    /// merge can write. Callers scope shared-checkout residue checks to this
    /// set instead of refusing on any dirty path at all.
    pub fn merge_touched_paths(&self, target: &str, source: &str) -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["diff", "--name-only", &format!("{target}...{source}"), "--"])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Merge `source_branch` into `target_branch` **without touching the
    /// main checkout** (cas-4702 / GH #68, #73).
    ///
    /// The merge runs in an ephemeral detached worktree created at the
    /// target branch's current tip, and the branch ref is then advanced with
    /// a compare-and-swap (`update-ref <ref> <new> <old>`), so:
    ///
    /// - the main checkout's HEAD, index and working tree are never moved —
    ///   a supervisor's next commit still lands on the branch they were on;
    /// - residue in the shared checkout cannot block or contaminate the
    ///   merge;
    /// - a concurrent writer that moved the target branch is never
    ///   clobbered (the Cassy declines instead).
    ///
    /// Detaching at the resolved SHA (rather than checking out the branch in
    /// the temp worktree) also means this works when the target branch is
    /// already checked out somewhere else.
    ///
    /// Returns the resulting tip of `target_branch`, or `None` when the
    /// merge produced no commit.
    pub fn merge_branch_via_temp_worktree(
        &self,
        target_branch: &str,
        source_branch: &str,
        no_ff: bool,
    ) -> Result<Option<String>> {
        let old_tip = self
            .resolve_commit(target_branch)
            .ok_or_else(|| GitError::BranchNotFound(target_branch.to_string()))?;

        // cas-0f04: a compare-and-swap on the ref is invisible to any OTHER
        // worktree that has this branch checked out. Decide what to do about
        // those checkouts BEFORE the merge — a dirty one refuses here, while
        // the ref is still where that checkout expects it.
        let checkouts = self.linked_checkouts_of(target_branch)?;
        for checkout in &checkouts {
            if let Some((change_count, first_change)) = dirty_summary(&checkout.path) {
                return Err(GitError::TargetCheckedOutDirty {
                    branch: target_branch.to_string(),
                    checkout: checkout.path.clone(),
                    change_count,
                    first_change,
                    tip: old_tip.clone(),
                });
            }
        }

        let guard = self.add_temp_worktree(&old_tip)?;
        let merged = self.merge_branch_in_dir(guard.path(), None, source_branch, no_ff)?;

        let new_tip = match merged {
            Some(ref tip) if *tip != old_tip => tip.clone(),
            // "Already up to date" — nothing to advance, and nothing to
            // compare-and-swap.
            _ => return Ok(Some(old_tip)),
        };

        if self
            .compare_and_swap_ref(target_branch, &new_tip, &old_tip)
            .is_err()
        {
            // The target moved under us. Report it as the typed tip change it
            // is — the merge commit stays unreferenced and dies with the temp
            // worktree, and the concurrent writer's ref is untouched.
            return Err(GitError::TargetTipChanged {
                branch: target_branch.to_string(),
                expected: old_tip,
                actual: self
                    .resolve_commit(target_branch)
                    .unwrap_or_else(|| "<unresolvable>".to_string()),
            });
        }

        // The ref moved. Every checkout listed above was clean when we looked
        // and holds this branch, so realigning it discards nothing and is the
        // difference between a usable checkout and one stranded on an
        // ancestor of its own HEAD.
        for checkout in &checkouts {
            if let CheckoutRefresh::LeftStale { reason } =
                self.refresh_linked_checkout(&checkout.path, target_branch, &old_tip, &new_tip)
            {
                self.record_stale_checkout(
                    target_branch,
                    &checkout.path,
                    &old_tip,
                    &new_tip,
                    &reason,
                );
            }
        }
        Ok(Some(new_tip))
    }

    /// Worktrees other than the ephemeral merge worktree that currently have
    /// `branch` checked out, as git itself reports them.
    ///
    /// The ephemeral worktree is detached, so it carries no `branch` line and
    /// never appears here.
    fn linked_checkouts_of(&self, branch: &str) -> Result<Vec<LinkedCheckout>> {
        // An unknown inventory must never authorize the ref advance: if we
        // cannot say which checkouts hold this branch, we cannot say the merge
        // is safe for them. Fail closed, before anything is written.
        #[cfg(test)]
        if std::env::var_os("CAS_TEST_FAIL_WORKTREE_LIST").is_some() {
            return Err(GitError::CheckoutInventoryUnavailable {
                branch: branch.to_string(),
                reason: "injected failure (CAS_TEST_FAIL_WORKTREE_LIST)".to_string(),
            });
        }

        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|error| GitError::CheckoutInventoryUnavailable {
                branch: branch.to_string(),
                reason: error.to_string(),
            })?;
        if !output.status.success() {
            return Err(GitError::CheckoutInventoryUnavailable {
                branch: branch.to_string(),
                reason: branch_ops::first_line(&String::from_utf8_lossy(&output.stderr)),
            });
        }

        let wanted = format!("refs/heads/{branch}");
        let text = String::from_utf8_lossy(&output.stdout);
        let mut found = Vec::new();
        let mut current: Option<PathBuf> = None;
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                current = Some(PathBuf::from(path));
            } else if let Some(reference) = line.strip_prefix("branch ") {
                if reference.trim() == wanted {
                    if let Some(path) = current.clone() {
                        found.push(LinkedCheckout { path });
                    }
                }
            }
        }
        Ok(found)
    }

    /// Record a checkout the merge deliberately did not touch.
    fn record_stale_checkout(
        &self,
        branch: &str,
        path: &Path,
        old_tip: &str,
        new_tip: &str,
        reason: &str,
    ) {
        // Deliberately does not claim what that checkout now contains: it may
        // have been edited, staged into, or switched to another branch inside
        // the window. The only facts we can state are that the ref moved, the
        // refresh declined, and why.
        let note = format!(
            "\n⚠️  Stale checkout: {branch} advanced {} -> {}, and the checkout at {} was left \
             untouched because the refresh declined ({reason}). Nothing there was overwritten. \
             Inspect it, preserve whatever it holds (commit or stash), then reconcile it \
             deliberately.",
            short_tip(old_tip),
            short_tip(new_tip),
            path.display(),
        );
        tracing::warn!("{}", note.trim());
        // A poisoned lock must not drop a safety warning: recover the
        // contained notes rather than discarding them.
        let mut notes = self
            .stale_checkout_notes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        notes.push(note);
    }

    /// Test seam for the receipt contract: records a note exactly as a real
    /// refusal does, without needing to drive a whole merge.
    #[cfg(test)]
    pub(crate) fn record_stale_checkout_for_test(
        &self,
        branch: &str,
        path: &Path,
        old_tip: &str,
        new_tip: &str,
        reason: &str,
    ) {
        self.record_stale_checkout(branch, path, old_tip, new_tip, reason);
    }

    /// Drain the stale-checkout notes recorded since the last call.
    ///
    /// The receipt assembler calls this after a merge so the operator reads
    /// about a stranded checkout in the response, not only in a log file.
    pub fn take_stale_checkout_notes(&self) -> Vec<String> {
        let mut notes = self
            .stale_checkout_notes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *notes)
    }

    /// Advance a linked checkout from `old_tip` to `new_tip`, refusing rather
    /// than overwriting if anything there changed.
    ///
    /// The cleanliness check happens before the merge; the refresh happens
    /// after the compare-and-swap. Someone can edit that checkout in between,
    /// so a `--reset` justified by the earlier observation would silently
    /// destroy work that appeared during the window. This uses git's two-tree
    /// `read-tree -m -u`, which performs the same update but refuses when an
    /// entry is not up to date — the safety is git's, evaluated at the moment
    /// of the write, not ours from an earlier glance.
    ///
    /// Refusal is not an error: the merge has already published. The checkout
    /// is simply left exactly as the operator has it, and the caller says so.
    pub(crate) fn refresh_linked_checkout(
        &self,
        path: &Path,
        branch: &str,
        old_tip: &str,
        new_tip: &str,
    ) -> CheckoutRefresh {
        if !path.is_dir() {
            return CheckoutRefresh::LeftStale {
                reason: "checkout directory no longer exists".to_string(),
            };
        }
        // The checkout may have been switched to a different branch inside the
        // window. Advancing it to this target's tip would then be a silent
        // checkout of a branch nobody asked for, so re-verify identity here
        // rather than trusting the pre-merge listing.
        match head_branch_of(path) {
            Some(current) if current == branch => {}
            Some(current) => {
                return CheckoutRefresh::LeftStale {
                    reason: format!("checkout moved to branch {current} during the merge"),
                };
            }
            None => {
                return CheckoutRefresh::LeftStale {
                    reason: "checkout is no longer on a branch (detached HEAD)".to_string(),
                };
            }
        }
        let output = Command::new("git")
            .args(["read-tree", "-m", "-u", old_tip, new_tip])
            .current_dir(path)
            .output();
        match output {
            Ok(output) if output.status.success() => CheckoutRefresh::Updated,
            Ok(output) => CheckoutRefresh::LeftStale {
                reason: branch_ops::first_line(&String::from_utf8_lossy(&output.stderr)),
            },
            Err(error) => CheckoutRefresh::LeftStale {
                reason: error.to_string(),
            },
        }
    }

    /// Absolute path of the repository's *common* git dir — shared by the
    /// main checkout and every linked worktree.
    fn git_common_dir(&self) -> Option<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .current_dir(&self.repo_root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!dir.is_empty()).then(|| PathBuf::from(dir))
    }

    /// Create an ephemeral detached worktree at `start_point`. The returned
    /// guard removes it (and prunes the admin entry) on drop.
    ///
    /// The worktree is created under the repository's git dir, not the system
    /// temp dir: `/tmp` is tmpfs (RAM) on many hosts, and checking out a large
    /// repository there for the duration of a merge can wedge the machine.
    /// Inside `.git/` it also stays invisible to `git status` in every
    /// checkout.
    fn add_temp_worktree(&self, start_point: &str) -> Result<TempWorktree> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = self
            .git_common_dir()
            .map(|dir| dir.join("cas-merge"))
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base)?;
        let path = base.join(format!(
            "cas-merge-{}-{}-{}",
            std::process::id(),
            unique,
            TEMP_WORKTREE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));

        let path_str = path.to_str().ok_or_else(|| {
            GitError::CommandFailed(format!("Path contains invalid UTF-8: {}", path.display()))
        })?;

        let output = Command::new("git")
            .args(["worktree", "add", "--detach", path_str, start_point])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "failed to create ephemeral merge worktree at {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        Ok(TempWorktree {
            repo_root: self.repo_root.clone(),
            path,
        })
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
    /// Cassy-generated artifacts (currently the `.husky/_` git-hooks shim the
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
