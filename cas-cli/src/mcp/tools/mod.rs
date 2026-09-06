//! MCP Tools for Cassy
//!
//! This module contains MCP tools organized by category:
//! - Memory tools (12): Entry management
//! - Task tools (15): Task and dependency management
//! - Rule tools (10): Rule management
//! - Skill tools (10): Skill management
//! - Search tools (1): Unified search with doc_type filter
//! - System tools (7): Context, stats, diagnostics, and utilities

use crate::hooks::{
    HookInput, build_context, build_context_with_token_budget, handle_session_end,
    handle_session_start,
};
use crate::types::{
    BeliefType, ClaimResult, DEFAULT_LEASE_DURATION_SECS, Dependency, DependencyType, Entry,
    EntryType, LeaseStatus, MemoryTier, ObservationType, Priority, Rule, RuleStatus, Scope, Skill,
    SkillStatus, SkillType, Task, TaskStatus, TaskType, Verification, VerificationIssue,
    VerificationStatus, VerificationType, WorktreeStatus,
};
use crate::hybrid_search::{DocType, SearchIndex, SearchOptions};

// Include all request types
mod types;
pub use types::*;

// CAS MCP service (7 meta-tools)
pub mod service;
pub use service::CasService;

// ============================================================================
// Tool Implementations - All in one impl block to satisfy the macro
// ============================================================================

// ============================================================================
// Sort Helper Functions
// ============================================================================

/// Sort any slice by task sort options, using a key function to extract the Task
fn sort_by_task_opts<T>(items: &mut [T], opts: &cas_types::TaskSortOptions, key: impl Fn(&T) -> &Task) {
    use cas_types::{SortOrder, TaskSortField};

    items.sort_by(|a, b| {
        let (a, b) = (key(a), key(b));
        let cmp = match opts.field {
            TaskSortField::Created => a.created_at.cmp(&b.created_at),
            TaskSortField::Updated => a.updated_at.cmp(&b.updated_at),
            TaskSortField::Priority => a.priority.0.cmp(&b.priority.0),
            TaskSortField::Title => a.title.cmp(&b.title),
        };
        let cmp = match opts.effective_order() {
            SortOrder::Asc => cmp,
            SortOrder::Desc => cmp.reverse(),
        };
        // cas-06f9 (GH #104): break ties deterministically. `list_ready` /
        // `list_blocked` carry an ORDER BY, but `get_subtasks` (the epic-
        // filtered path) does not, so equal-priority rows arrived in
        // SQLite-plan order — two identical calls could show different tasks
        // inside a capped window, which is precisely the kind of "it moved and
        // I don't know why" that truncation honesty is meant to remove.
        cmp.then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Sort a vector of tasks based on sort options
pub(super) fn sort_tasks(tasks: &mut [Task], opts: &cas_types::TaskSortOptions) {
    sort_by_task_opts(tasks, opts, |t| t);
}

/// cas-06f9 (GH #104): default the "what can I work on" queries to
/// priority order.
///
/// `TaskSortOptions`' own default is `Created`, which is incidental ordering
/// for this question — and combined with a silent 10-row cap it hid thirteen
/// ready P0 tasks behind P2/P3 follow-ups while a supervisor assigned work
/// from the visible window. An explicit `sort=` from the caller still wins;
/// this only changes what "unspecified" means.
pub(super) fn ready_blocked_sort_options(
    sort: Option<&str>,
    order: Option<&str>,
) -> cas_types::TaskSortOptions {
    use cas_types::TaskSortField;
    // An unparseable `sort=` must NOT silently fall back to created/desc —
    // that is the exact pre-fix ordering, so `sort=p0` or `sort=highest`
    // (neither is a valid field) would hand the caller the incident behaviour
    // back with no error. Unrecognised means unspecified, and unspecified means
    // priority here.
    cas_types::TaskSortOptions::new(
        sort.and_then(|s| s.parse().ok())
            .unwrap_or(TaskSortField::Priority),
        order.and_then(|o| o.parse().ok()),
    )
}

/// Human-readable name for the ordering actually applied, so the header can
/// never imply an order the rows are not in (cas-06f9).
pub(super) fn sort_order_label(opts: &cas_types::TaskSortOptions) -> &'static str {
    use cas_types::{SortOrder, TaskSortField};
    let ascending = matches!(opts.effective_order(), SortOrder::Asc);
    match (opts.field, ascending) {
        (TaskSortField::Priority, true) => "P0 first",
        (TaskSortField::Priority, false) => "lowest priority first",
        (TaskSortField::Created, true) => "oldest first",
        (TaskSortField::Created, false) => "newest first",
        (TaskSortField::Updated, true) => "least recently updated first",
        (TaskSortField::Updated, false) => "most recently updated first",
        (TaskSortField::Title, true) => "title A-Z",
        (TaskSortField::Title, false) => "title Z-A",
    }
}

/// Header that states the true total whenever the list is capped (cas-06f9).
///
/// `Ready tasks (3, P0 first):` when everything fits;
/// `Ready tasks (showing 10 of 30, P0 first):` when it does not. The previous
/// header printed only the shown count, so a capped list was indistinguishable
/// from a drained queue.
pub(super) fn truncated_list_header(
    noun: &str,
    total: usize,
    shown: usize,
    opts: &cas_types::TaskSortOptions,
) -> String {
    let order = sort_order_label(opts);
    if shown < total {
        format!("{noun} (showing {shown} of {total}, {order}):\n\n")
    } else {
        format!("{noun} ({total}, {order}):\n\n")
    }
}

/// Footer naming what was withheld and how to see it (cas-06f9).
pub(super) fn truncated_list_footer(total: usize, shown: usize) -> String {
    if shown >= total {
        return String::new();
    }
    let hidden = total - shown;
    format!("\n... and {hidden} more not shown — pass limit={total} to see all of them.\n")
}

/// Sort a slice of task references (cas-61d3 / GH #111).
///
/// `tasks_available` filters `list_ready`'s output into a `Vec<&Task>`, so it
/// cannot use `sort_tasks` without cloning every row it is about to print.
pub(super) fn sort_task_refs(tasks: &mut [&Task], opts: &cas_types::TaskSortOptions) {
    sort_by_task_opts(tasks, opts, |t| *t);
}

/// Sort a vector of blocked tasks (task, blockers) tuples based on sort options
pub(super) fn sort_blocked_tasks(
    blocked: &mut [(Task, Vec<Task>)],
    opts: &cas_types::TaskSortOptions,
) {
    sort_by_task_opts(blocked, opts, |(t, _)| t);
}

// ============================================================================
// Branch Name Helper
// ============================================================================

/// Convert a title to a branch-safe slug for epic branches
///
/// Creates branch names like `epic/add-user-authentication` from titles.
fn slugify_for_branch(title: &str) -> String {
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

/// Return the canonical branch name for a newly-created epic.
///
/// Keep the task id outside the truncated title slug so two long titles cannot
/// silently collide, and so every creator/consumer agrees on the same ref.
pub(crate) fn epic_branch_name(title: &str, epic_id: &str) -> String {
    let slugified = slugify_for_branch(title);
    let slug = slugified.trim_end_matches('-');
    format!("epic/{slug}-{}", epic_id.trim())
}

// ============================================================================
// Epic Merge Check Helper
// ============================================================================

/// Check for unmerged worker branches for an epic
///
/// In factory mode, workers may push branches in format `{epic-id}/{worker-name}` to a remote.
/// This function checks remote branches first, then falls back to local branches when no
/// matching remote branches exist.
///
/// Returns a list of unmerged branch names, or empty if all merged.
fn check_unmerged_epic_branches(
    repo_path: &std::path::Path,
    epic_id: &str,
    target_branch: &str,
) -> Vec<String> {
    use std::collections::HashSet;

    let remote_branches = list_git_branches(
        Some(repo_path),
        &["branch", "-r", "--list", &format!("origin/{epic_id}/*")],
    );
    if !remote_branches.is_empty() {
        let mut merged: HashSet<String> = list_git_branches(
            Some(repo_path),
            &["branch", "-r", "--merged", target_branch],
        )
        .into_iter()
        .collect();

        if merged.is_empty() && !target_branch.starts_with("origin/") {
            let fallback_branch = format!("origin/{target_branch}");
            merged = list_git_branches(
                Some(repo_path),
                &["branch", "-r", "--merged", &fallback_branch],
            )
            .into_iter()
            .collect();
        }

        return remote_branches
            .into_iter()
            .filter(|b| !merged.contains(b))
            .collect();
    }

    let local_branches = list_git_branches(
        Some(repo_path),
        &["branch", "--list", &format!("{epic_id}/*")],
    );
    if local_branches.is_empty() {
        return vec![];
    }

    let merged_local: HashSet<String> =
        list_git_branches(Some(repo_path), &["branch", "--merged", target_branch])
            .into_iter()
            .collect();

    local_branches
        .into_iter()
        .filter(|b| !merged_local.contains(b))
        .collect()
}

/// Resolve which git ref a worktree should be compared against for assignment
/// freshness (cas-44e9).
///
/// Order:
/// 1. `preferred` — task's parent epic branch, or session focus_epic pin (caller-resolved)
/// 2. upstream tracking ref of HEAD (when set)
/// 3. current branch when it is already `epic/*`
/// 4. repository default / base branch
///
/// **Never** picks an arbitrary `epic/*` from `git branch --list` — that is the
/// multi-epic bug where concurrent epic B contaminated assignments for epic A.
pub(crate) fn resolve_staleness_sync_ref(
    preferred: Option<&str>,
    current_branch: &str,
    upstream: Option<&str>,
    default_branch: &str,
) -> String {
    if let Some(p) = preferred.map(str::trim).filter(|s| !s.is_empty()) {
        return p.to_string();
    }
    if let Some(u) = upstream.map(str::trim).filter(|s| !s.is_empty()) {
        return u.to_string();
    }
    if current_branch.starts_with("epic/") {
        return current_branch.to_string();
    }
    // factory/* and all other local branches: base/main — not "last epic/*".
    if default_branch.trim().is_empty() {
        "main".to_string()
    } else {
        default_branch.to_string()
    }
}

/// Check how many commits behind its sync target a worktree is.
///
/// `preferred_sync_ref` is the task-scoped target (parent epic branch / focus pin).
/// When set, it always wins so multi-epic factories compare against the correct epic.
///
/// Returns (commits_behind, sync_ref) or None if check fails (missing path / git error).
pub(crate) fn check_worktree_staleness(
    clone_path: &str,
    preferred_sync_ref: Option<&str>,
) -> Option<(u32, String)> {
    check_worktree_staleness_with_fetch(clone_path, preferred_sync_ref, true)
}

fn check_worktree_staleness_with_fetch(
    clone_path: &str,
    preferred_sync_ref: Option<&str>,
    fetch: bool,
) -> Option<(u32, String)> {
    use crate::worktree::GitOperations;
    use std::path::Path;
    use std::process::Command;

    let path = Path::new(clone_path);
    if !path.exists() {
        return None;
    }

    // Auto-detect target branch by checking current branch
    let branch_output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;

    let current_branch = if branch_output.status.success() {
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string()
    } else {
        return None;
    };

    let default_branch = GitOperations::detect_repo_root(path)
        .ok()
        .map(GitOperations::new)
        .map(|git| git.detect_default_branch())
        .unwrap_or_else(|| "main".to_string());

    let sync_ref = resolve_staleness_sync_ref(
        preferred_sync_ref,
        &current_branch,
        current_upstream(path).as_deref(),
        &default_branch,
    );

    // Fetch latest refs when sync target is a remote-tracking ref.
    if fetch && let Some(remote) = remote_for_ref(path, &sync_ref) {
        let _ = Command::new("git")
            .args(["fetch", &remote])
            .current_dir(path)
            .status();
    }

    let behind_count = count_unheld_behind(path, &sync_ref)?;

    Some((behind_count, sync_ref))
}

/// Count commits on `sync_ref` whose **content** the worktree does not already
/// have (cas-f8bc / GH #106).
///
/// The naive `git rev-list --count HEAD..<epic>` counts every commit reachable
/// from the epic and not from HEAD — which includes the merge commit of the
/// worker's *own* just-merged branch. That produced a circular deadlock: a
/// worker's completed lane is merged, the merge itself makes the worker read
/// as "1 commit behind epic", assignment is refused for staleness, and the
/// refused assignment was the prerequisite for the merge that would clear it.
/// The worker had nothing to gain from syncing: its work IS the epic's tip.
///
/// Two adjustments, both verified against real git rather than assumed:
/// - `--no-merges` drops the merge *node* itself. Content is not lost: a merge
///   of another worker's lane still contributes that worker's own non-merge
///   commits, which are counted individually.
/// - `--cherry-pick --right-only A...B` drops commits whose patch-id already
///   exists on HEAD. This covers the supervisor rebasing/cherry-picking a
///   worker's lane onto the epic instead of merging it, where the worker's own
///   commits reappear under new SHAs and `--no-merges` alone still counts them.
///
/// Genuine staleness is unaffected: another worker's merged commits are absent
/// from HEAD by both reachability and patch-id, so they still count.
///
/// Returns `None` only when git cannot answer at all.
pub(crate) fn count_unheld_behind(path: &std::path::Path, sync_ref: &str) -> Option<u32> {
    use std::process::Command;

    // Content check first, and it is the authoritative one: if the worktree's
    // tree is identical to the sync target's, there is by definition nothing
    // to sync, whatever the commit topology says.
    //
    // This is what catches a SQUASH-merged lane. A squash collapses N of the
    // worker's commits into one new commit whose patch-id matches none of the
    // originals, so the commit-level rules below still count it and the GH #106
    // deadlock returns. Comparing trees is immune to how the lane was landed —
    // merge, rebase, cherry-pick or squash.
    match Command::new("git")
        .args(["diff", "--quiet", "HEAD", sync_ref, "--"])
        .current_dir(path)
        .status()
    {
        Ok(status) if status.code() == Some(0) => return Some(0),
        // code 1 = trees differ (expected); anything else = git could not
        // answer, so fall through to the commit count rather than trusting it.
        _ => {}
    }

    let output = Command::new("git")
        .args([
            "rev-list",
            "--count",
            "--no-merges",
            "--cherry-pick",
            "--right-only",
            &format!("HEAD...{sync_ref}"),
        ])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u32>()
            .unwrap_or(0),
    )
}

fn list_git_branches(path: Option<&std::path::Path>, args: &[&str]) -> Vec<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(path) = path {
        cmd.current_dir(path);
    }

    match cmd.output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(normalize_branch_line)
            .collect(),
        _ => vec![],
    }
}

fn normalize_branch_line(line: &str) -> Option<String> {
    let trimmed = line
        .trim()
        .trim_start_matches('*')
        .trim_start_matches('+')
        .trim();
    if trimmed.is_empty() || trimmed.contains(" -> ") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn current_upstream(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if upstream.is_empty() {
        None
    } else {
        Some(upstream)
    }
}

fn remote_for_ref(path: &std::path::Path, reference: &str) -> Option<String> {
    let candidate = reference.split('/').next()?;
    if candidate.is_empty() {
        return None;
    }

    let output = std::process::Command::new("git")
        .args(["remote"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let remotes = String::from_utf8_lossy(&output.stdout);
    remotes
        .lines()
        .map(str::trim)
        .find(|name| *name == candidate)
        .map(str::to_string)
}

// NOTE: #[tool_router] removed - CasService (service/mod.rs) is the actual MCP service.
// CasCore methods are called directly, not through tool routing.
// This reduces compile time by avoiding proc-macro expansion of ~77 tools.

/// Helper to truncate strings for display (shared by core and service modules)
pub(crate) fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

pub(crate) mod core;

#[cfg(test)]
mod mod_tests;
