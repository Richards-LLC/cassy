use crate::mcp::tools::core::imports::*;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct DeliverySupervisorAuthority {
    agent_id: String,
}

fn derive_delivery_supervisor_authority(
    agent: &cas_types::Agent,
) -> Result<DeliverySupervisorAuthority, &'static str> {
    if agent.role != cas_types::AgentRole::Supervisor || !agent.is_alive() {
        return Err("only a live server-registered Supervisor may resume transactional delivery");
    }
    Ok(DeliverySupervisorAuthority {
        agent_id: agent.id.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryMergePreflight {
    Execute,
    Reconcile,
}

fn classify_delivery_merge_preflight(
    repo_binding_matches: bool,
    commit_exists: bool,
    already_merged: bool,
    source_tip_matches: bool,
    target_tip_matches: bool,
    merge_base_matches: bool,
) -> Result<DeliveryMergePreflight, (cas_types::WorkerDeliveryState, &'static str, &'static str)> {
    use cas_types::WorkerDeliveryState;
    if !repo_binding_matches {
        return Err((
            WorkerDeliveryState::RepoMismatch,
            "repo_mismatch",
            "receipt repository/branch binding no longer matches the server-resolved worktree target",
        ));
    }
    if !commit_exists {
        return Err((
            WorkerDeliveryState::Stale,
            "stale_commit",
            "receipt commit no longer resolves to a commit object",
        ));
    }
    if already_merged {
        return Ok(DeliveryMergePreflight::Reconcile);
    }
    if !source_tip_matches {
        return Err((
            WorkerDeliveryState::TipChanged,
            "tip_changed",
            "worker branch tip changed after immutable receipt submission",
        ));
    }
    if !target_tip_matches {
        // cas-0a21: target drift is a *tip* change, not a generic staleness.
        // The typed TipChanged state is what tells a supervisor this is
        // recoverable by re-reviewing against the new tip.
        return Err((
            WorkerDeliveryState::TipChanged,
            "target_tip_changed",
            "target branch tip changed after receipt submission",
        ));
    }
    if !merge_base_matches {
        return Err((
            WorkerDeliveryState::Stale,
            "merge_base_changed",
            "live merge base differs from the immutable receipt",
        ));
    }
    Ok(DeliveryMergePreflight::Execute)
}

/// Check whether `path` looks like a live git worktree (has a `.git` entry
/// — a file for linked worktrees, pointing back at the main repo's
/// worktree admin dir).
///
/// Used to confirm a System B (`spawn_workers isolate=true`) worktree
/// actually exists at its resolved path before `worktree_merge` acts on
/// it (cas-1d11). Returns `false` for a path that doesn't exist or isn't a
/// git worktree — an unknowable worktree is not treated as a false
/// positive.
fn is_git_worktree(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Active statuses that count as "this worker still has work tied to an epic"
/// for merge-target inference when the supervisor omits `task_id` (cas-0b32).
fn assignee_task_is_merge_relevant(status: cas_types::TaskStatus) -> bool {
    use cas_types::TaskStatus::*;
    matches!(
        status,
        Open | InProgress | Blocked | AwaitingMerge | PendingSupervisorReview
    )
}

/// Remediation block shared by merge-target rejections (cas-0b32).
fn merge_target_remediation(assignee: &str) -> String {
    format!(
        "Remediation:\n\
         1. Prefer an explicit task: `coordination action=worktree_merge id={assignee} \
         task_id=<task-id>` (or `id=factory/{assignee}`).\n\
         2. Standalone / trunk merges require explicit intent: pass `allow_trunk=true` \
         (and `task_id` when merging a non-epic task). `force=true` only bypasses dirty \
         worktree protection — it does NOT authorize trunk.\n\
         Session `focus_epic` is a display filter and never authorizes a merge target. \
         CAS never relies on a silent default to main/master/staging."
    )
}

/// Defense-in-depth for both registered (System A) and factory (System B)
/// worktrees: once an epic is closed, its branch must not advance silently and
/// invalidate the close receipt. Task-based System B resolution rejects this
/// earlier; this final target check covers stored worktree records too.
fn reject_closed_epic_merge_target(
    task_store: &dyn cas_store::TaskStore,
    target_branch: &str,
) -> Result<(), McpError> {
    let closed_epics = task_store
        .list(None)
        .map_err(|error| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "Failed to validate merge target {target_branch} against closed epics: {error}"
            )),
            data: None,
        })?
        .into_iter()
        .filter(|task| {
            task.task_type == cas_types::TaskType::Epic
                && task.status == cas_types::TaskStatus::Closed
                && task.branch.as_deref() == Some(target_branch)
        })
        .map(|task| task.id)
        .collect::<Vec<_>>();

    if closed_epics.is_empty() {
        return Ok(());
    }

    Err(McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: Cow::from(format!(
            "merge target {target_branch} belongs to closed epic(s) {} — refusing to \
             advance the branch after its close receipt",
            closed_epics.join(", ")
        )),
        data: None,
    })
}

/// cas-369f: decide whether `worktree_merge` should remove the worktree after
/// merging. Pure — unit-tested.
///
/// Rules:
/// - Explicit `cleanup` request always wins (end-of-lane consume vs preserve).
/// - `force` is **not** consulted here (dirty-tree only; cas-0b32 / cas-369f).
/// - System B (`spawn_workers isolate=true` factory workers): default **preserve**
///   so mid-epic merges do not ENOENT the live worker cwd.
/// - System A: fall back to config `cleanup_on_close`.
pub(crate) fn resolve_worktree_merge_cleanup(
    requested_cleanup: Option<bool>,
    is_system_b: bool,
    config_cleanup_on_close: bool,
) -> bool {
    if let Some(c) = requested_cleanup {
        return c;
    }
    if is_system_b {
        false
    } else {
        config_cleanup_on_close
    }
}

/// Short SHA for receipt prose; passes anything shorter through unchanged.
fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// cas-5ee0 (GH #137): render the merge receipt's push-state line. Pure —
/// unit-tested.
///
/// A `worktree_merge` that moves only the LOCAL target ref is invisible to the
/// task-close merge-state guard, which measures ancestry against BOTH
/// `<target>` and `origin/<target>`. When the local ref moved and origin did
/// not, every close of a task delivered by that merge bounces with
/// "N commit(s) from this task not on <target>" until someone pushes by hand.
/// The receipt must therefore never claim a bare "Merged" without saying
/// whether the merge is actually visible on the remote.
pub(crate) fn describe_target_push_state(
    target_branch: &str,
    outcome: &crate::worktree::git::TargetPushOutcome,
) -> String {
    use crate::worktree::git::TargetPushOutcome;

    match outcome {
        // Local-only repository: there is no remote ref for anything
        // downstream to disagree with, so the local ref IS authoritative.
        // Stating it keeps the receipt unambiguous without crying wolf.
        TargetPushOutcome::NoRemote => format!(
            "\nPush state: no `origin` remote configured — local {target_branch} is authoritative."
        ),
        // A branch that was never published has no remote ref for the close
        // guard to consult — it skips its `origin/<parent>` check when the ref
        // is absent — so the local ref is authoritative and no push is owed.
        TargetPushOutcome::RemoteBranchAbsent => format!(
            "\nPush state: origin/{target_branch} does not exist — {target_branch} is a \
             local-only branch; local ref is authoritative (no remote branch created)."
        ),
        TargetPushOutcome::AlreadyCurrent { sha } => format!(
            "\nPush state: origin/{target_branch} already at {} (nothing to push).",
            short_sha(sha)
        ),
        TargetPushOutcome::Pushed { sha } => format!(
            "\nPush state: pushed {target_branch} -> origin/{target_branch} ({}).",
            short_sha(sha)
        ),
        TargetPushOutcome::NotPushed {
            sha,
            remote_sha,
            reason,
        } => {
            let local = sha.as_deref().map(short_sha).unwrap_or("unresolved");
            let remote = remote_sha
                .as_deref()
                .map(short_sha)
                .unwrap_or("(no origin ref)");
            format!(
                "\n\n⚠️  NOT PUSHED — THIS MERGE IS LOCAL ONLY.\n\
                 Local {target_branch} is at {local}; origin/{target_branch} is at {remote} \
                 and is BEHIND.\n\
                 Task close measures merge evidence against BOTH {target_branch} and \
                 origin/{target_branch}, and other checkouts only ever see origin. Until this \
                 is published, closes for work delivered by this merge can be rejected with \
                 \"commit(s) from this task not on {target_branch}\".\n\
                 REQUIRED NEXT STEP: git push origin {target_branch}\n\
                 Automatic push did not land: {reason}"
            )
        }
    }
}

/// Translate worktree merge failures into supervisor-actionable MCP errors.
///
/// The low-level merge layer owns cleanup and path discovery; this callable
/// surface explains the resulting state and the safe recovery choices.
fn worktree_merge_mcp_error(
    error: crate::worktree::WorktreeError,
    source_branch: &str,
    target_branch: &str,
) -> McpError {
    use crate::worktree::{GitError, WorktreeError};

    let message = match error {
        WorktreeError::Git(GitError::MergeConflictPaths(paths)) => format!(
            "CONTENT CONFLICT: {source_branch} cannot be merged into {target_branch}. \
             Conflicting paths: {}.\n\n\
             The shared checkout was left at or restored to its pre-merge state; this \
             attempt left no MERGE_HEAD or staged conflict.\n\n\
             Manual options:\n\
             1. Resolve on {source_branch} by rebasing or merging {target_branch}, then retry.\n\
             2. Use a temporary worktree for {target_branch}, merge {source_branch} there, \
             resolve and commit the conflicts, then remove the temporary worktree.",
            paths.join(", ")
        ),
        WorktreeError::Git(GitError::MergeConflict) => format!(
            "CONTENT CONFLICT: {source_branch} cannot be merged into {target_branch}; \
             Git did not report the conflicting paths.\n\n\
             The shared checkout was left at or restored to its pre-merge state; this \
             attempt left no MERGE_HEAD or staged conflict.\n\n\
             Manual options:\n\
             1. Resolve on {source_branch} by rebasing or merging {target_branch}, then retry.\n\
             2. Use a temporary worktree for {target_branch}, merge {source_branch} there, \
             resolve and commit the conflicts, then remove the temporary worktree."
        ),
        WorktreeError::Git(GitError::MergeInProgress(details)) => format!(
            "PRE-EXISTING MERGE RESIDUE: the shared target checkout already has \
             MERGE_HEAD and unresolved index state: {details}.\n\n\
             worktree_merge did not attempt {source_branch}. Resolve and commit the \
             existing merge, or run `git merge --abort` in the shared target checkout, \
             before retrying the merge into {target_branch}."
        ),
        WorktreeError::Git(GitError::MergeCheckoutDirty(details)) => format!(
            "PRE-EXISTING TARGET CHECKOUT RESIDUE: the shared target checkout has \
             tracked changes on paths this merge would write: {details}.\n\n\
             worktree_merge did not attempt {source_branch}. Residue on paths the merge \
             does NOT touch is ignored — the merge runs in an ephemeral worktree and \
             never moves the shared checkout's HEAD — so only these intersecting paths \
             block it.\n\n\
             Sanctioned fallback:\n\
             1. Commit, stash, or move just the listed paths, then retry the merge into \
             {target_branch}.\n\
             2. Or merge out of band without touching the shared checkout: \
             `git worktree add --detach /tmp/cas-merge {target_branch}`, merge \
             {source_branch} there, then move the branch ref and \
             `git worktree remove /tmp/cas-merge`.\n\n\
             `force=true` does not bypass shared-checkout residue."
        ),
        WorktreeError::Git(GitError::TargetTipChanged {
            ref branch,
            ref expected,
            ref actual,
        }) => format!(
            "TARGET TIP CHANGED: {branch} moved from {expected} to {actual} while \
             {source_branch} was being merged.\n\n\
             The merge ran in an ephemeral worktree and was discarded rather than \
             published over the concurrent update — {target_branch} still points at the \
             other writer's commit and no shared checkout was touched. Re-run the merge \
             against the new tip."
        ),
        other => format!("Failed to merge worktree: {other}"),
    };

    McpError {
        code: ErrorCode::INTERNAL_ERROR,
        message: Cow::Owned(message),
        data: None,
    }
}

/// True when `token` is the System-B worker name (bare or `factory/<name>`).
fn worker_name_token_matches(token: &str, worker: &str) -> bool {
    token == worker || token.strip_prefix("factory/") == Some(worker)
}

/// Resolve whether an identity token (assignee field or agent id/name) belongs
/// to the System-B worker being merged (cas-bd5f).
fn identity_belongs_to_worker(
    token: &str,
    worker: &str,
    agent_store: &dyn cas_store::AgentStore,
) -> bool {
    if worker_name_token_matches(token, worker) {
        return true;
    }
    // Assignee may be an agent id — resolve and match on name.
    if let Ok(agent) = agent_store.get(token) {
        return worker_name_token_matches(&agent.name, worker) || agent.id == worker;
    }
    // Or a name that maps to a registered agent whose id equals worker
    // (rare; worker is almost always a display name).
    if let Ok(agents) = agent_store.list(None) {
        return agents.iter().any(|a| {
            (a.name == token || a.id == token)
                && (worker_name_token_matches(&a.name, worker) || a.id == worker)
        });
    }
    false
}

/// Authorize that an explicit `task_id` belongs to the System-B worker whose
/// branch is being merged (cas-bd5f).
///
/// Pre-cas-bd5f gap: `resolve_system_b_merge_target` used the task's parent
/// epic whenever `task_id` was supplied, without checking that the task's
/// assignee / active lease matched the worktree worker. A caller could pair
/// worker A with task B and redirect A's branch into B's epic.
///
/// Binding rules:
/// 1. Active valid lease held by the worker → authorize (lease is authoritative).
/// 2. Else task.assignee matches the worker (name, factory/name, or agent id) → ok.
/// 3. Active valid lease held by a *different* agent → reject (incl. cross-session).
/// 4. Assignee set to a different worker → reject.
/// 5. No assignee and no matching valid lease → **conservative reject**.
///
/// Diagnostics are audit-ready: include worker, task id, and the mismatched
/// identity token.
fn authorize_explicit_task_for_system_b_worker(
    task: &cas_types::Task,
    worker: &str,
    agent_store: &dyn cas_store::AgentStore,
) -> Result<(), McpError> {
    let task_id = task.id.as_str();

    // Active lease is authoritative when present and valid.
    let active_lease = match agent_store.get_lease(task_id) {
        Ok(lease) => lease.filter(|l| l.is_valid()),
        Err(e) => {
            return Err(McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to read lease for task {task_id} while authorizing \
                     worktree_merge for worker {worker}: {e} — refusing (fail-closed)."
                )),
                data: None,
            });
        }
    };

    if let Some(lease) = active_lease.as_ref() {
        let holder_matches = identity_belongs_to_worker(&lease.agent_id, worker, agent_store);
        if !holder_matches {
            let holder_desc = agent_store
                .get(&lease.agent_id)
                .map(|a| {
                    format!(
                        "agent id={} name={} session={}",
                        a.id,
                        a.name,
                        a.factory_session.as_deref().unwrap_or("-")
                    )
                })
                .unwrap_or_else(|_| format!("agent id={}", lease.agent_id));
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "worktree_merge authorization failed (cas-bd5f): task {task_id} has an \
                     active lease held by {holder_desc}, which does not match worker \
                     '{worker}'. Refusing to redirect worker '{worker}'s branch into a \
                     foreign task's epic.\n\n{}",
                    merge_target_remediation(worker)
                )),
                data: None,
            });
        }
        // Lease matches worker. If assignee is also set, it must not contradict.
        if let Some(ref assignee) = task.assignee {
            if !identity_belongs_to_worker(assignee, worker, agent_store) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "worktree_merge authorization failed (cas-bd5f): task {task_id} \
                         lease matches worker '{worker}', but assignee '{assignee}' does \
                         not — refusing contradictory ownership.\n\n{}",
                        merge_target_remediation(worker)
                    )),
                    data: None,
                });
            }
        }
        return Ok(());
    }

    // No valid lease — require assignee match.
    match task.assignee.as_deref() {
        Some(assignee) if identity_belongs_to_worker(assignee, worker, agent_store) => Ok(()),
        Some(assignee) => Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "worktree_merge authorization failed (cas-bd5f): task {task_id} is assigned \
                 to '{assignee}', not worker '{worker}'. Refusing to redirect worker \
                 '{worker}'s branch into a foreign task's epic.\n\n{}",
                merge_target_remediation(worker)
            )),
            data: None,
        }),
        None => Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "worktree_merge authorization failed (cas-bd5f): task {task_id} has no \
                 assignee and no active lease belonging to worker '{worker}' — refusing \
                 (conservative rule). Assign the task to '{worker}' or claim a lease \
                 before merging with task_id=.\n\n{}",
                merge_target_remediation(worker)
            )),
            data: None,
        }),
    }
}

/// Path prefix match with canonicalize fallback (symlinks / relative forms).
fn path_is_under(path: &Path, base: &Path) -> bool {
    if path.starts_with(base) {
        return true;
    }
    match (std::fs::canonicalize(path), std::fs::canonicalize(base)) {
        (Ok(p), Ok(b)) => p.starts_with(b),
        _ => false,
    }
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Resolve the configured factory worktree base for this project.
///
/// Matches `spawn_workers isolate=true` / `WorktreeManager` resolution so
/// `worktree_list` does not hardcode `<cas_root>/worktrees` (cas-d1a0).
fn resolve_factory_worktree_base(cas_root: &Path) -> PathBuf {
    use crate::config::Config;

    let Some(project_dir) = cas_root.parent() else {
        return cas_root.join("worktrees");
    };
    let config = Config::load(cas_root).unwrap_or_default();
    config.worktrees().resolve_base_path(project_dir)
}

/// Whether a live git worktree looks CAS-managed (factory, epic, cas/*, or
/// under known CAS worktree roots) and should appear in worktree_list even
/// without a WorktreeStore row (sibling/predecessor sessions).
fn is_cas_pattern_worktree(
    path: &Path,
    branch: Option<&str>,
    cas_root: &Path,
    factory_base: &Path,
    repo_root: &Path,
) -> bool {
    // Main checkout is never listed as a managed worktree entry.
    if paths_equal(path, repo_root) {
        return false;
    }

    if path_is_under(path, factory_base) {
        return true;
    }
    // Default System B layout — still scanned when base_path is customized.
    if path_is_under(path, &cas_root.join("worktrees")) {
        return true;
    }
    // Claude Code agent isolation dirs (also swept by factory cleanup).
    if path_is_under(path, &repo_root.join(".claude").join("worktrees")) {
        return true;
    }

    if let Some(b) = branch {
        if b.starts_with("factory/") || b.starts_with("epic/") || b.starts_with("cas/") {
            return true;
        }
    }

    false
}

fn is_factory_style_worktree(
    path: &Path,
    branch: &str,
    cas_root: &Path,
    factory_base: &Path,
) -> bool {
    branch.starts_with("factory/")
        || path_is_under(path, factory_base)
        || path_is_under(path, &cas_root.join("worktrees"))
}

/// Reconcile live git worktrees that match CAS patterns but are missing from
/// the SQLite WorktreeStore (System B never registers; System A rows are
/// project-scoped but may be absent for sibling-session worktrees).
///
/// Returns transient `Worktree` rows with `git:` id prefix for display.
fn collect_untracked_git_worktrees(
    cas_root: &Path,
    factory_base: &Path,
    tracked_branches: &HashSet<String>,
    tracked_paths: &HashSet<PathBuf>,
) -> Vec<crate::types::Worktree> {
    use crate::types::Worktree;
    use crate::worktree::GitOperations;

    let mut out = Vec::new();
    let Some(project_dir) = cas_root.parent() else {
        return out;
    };
    let Ok(repo_root) = GitOperations::detect_repo_root(project_dir) else {
        return out;
    };
    let git_ops = GitOperations::new(repo_root.clone());
    let Ok(git_worktrees) = git_ops.list_worktrees() else {
        return out;
    };

    for git_wt in git_worktrees {
        if !is_cas_pattern_worktree(
            &git_wt.path,
            git_wt.branch.as_deref(),
            cas_root,
            factory_base,
            &repo_root,
        ) {
            continue;
        }

        let branch = git_wt.branch.clone().unwrap_or_default();
        if !branch.is_empty() && tracked_branches.contains(&branch) {
            continue;
        }
        if tracked_paths.iter().any(|p| paths_equal(p, &git_wt.path)) {
            continue;
        }

        let id_key = if !branch.is_empty() {
            branch.clone()
        } else {
            git_wt.path.display().to_string()
        };
        let display_branch = if branch.is_empty() {
            "(detached)".to_string()
        } else {
            branch
        };

        out.push(Worktree::new(
            format!("git:{id_key}"),
            display_branch,
            "unknown".to_string(),
            git_wt.path,
        ));
    }

    out
}

/// Resolve the parent branch a System B worker's branch should merge into
/// (cas-0938, tightened cas-0b32, authorized cas-bd5f).
///
/// History:
/// - Pre-cas-0938: System-B always merged to trunk → silent wrong-target.
/// - cas-0938: when `task_id` is set, use the task's parent epic branch.
/// - Pre-cas-0b32 residual: **no `task_id` still fell through to trunk** with
///   reason "no task_id given". Live incident 2026-07-11: supervisor merged
///   `hv-director` to main while epic cas-0e22 was focused and the worker's
///   task belonged to that epic.
/// - Pre-cas-bd5f residual: explicit `task_id` resolved parent epic without
///   verifying the task belongs to the worker being merged — a caller could
///   pair worker A with task B and redirect A's branch into B's epic.
///
/// Resolution order (cas-0b32 + cas-bd5f + cas-b86e):
/// 1. Explicit `task_id` → **authorize worker ownership** (assignee/lease), then
///    parent epic branch; if none, **reject** unless `allow_trunk`.
/// 2. Else resolve the assignee's non-closed tasks. A unique epic target wins;
///    a unique standalone task may use trunk only with `allow_trunk`; mixed or
///    multiple task targets reject as ambiguous.
/// 3. Else trunk only when `allow_trunk` explicitly authorizes it.
/// 4. Else reject with remediation — **never** silent trunk default.
///
/// Session `focus_epic` is deliberately absent: it is a TUI attention hint,
/// not merge authority. Any closed parent epic is rejected before git mutation.
///
/// Always returns a human-readable reason on success.
fn resolve_system_b_merge_target(
    task_store: &dyn cas_store::TaskStore,
    agent_store: &dyn cas_store::AgentStore,
    task_id: Option<&str>,
    assignee: &str,
    allow_trunk: bool,
    trunk: impl FnOnce() -> String,
) -> Result<(String, String), McpError> {
    if let Some(task_id) = task_id {
        let task = task_store.get(task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "task_id {task_id} not found — refusing to guess a merge target: {e}"
            )),
            data: None,
        })?;
        // cas-bd5f: bind explicit task context to the worker identity.
        authorize_explicit_task_for_system_b_worker(&task, assignee, agent_store)?;
        let epic = task_store.get_parent_epic(task_id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "Failed to resolve parent epic for task {task_id}: {e}"
            )),
            data: None,
        })?;
        if let Some(epic) = epic {
            if epic.status == cas_types::TaskStatus::Closed {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "task {task_id}'s parent epic {} is Closed — refusing to advance \
                         a branch after the epic's close receipt.\n\n{}",
                        epic.id,
                        merge_target_remediation(assignee)
                    )),
                    data: None,
                });
            }
            if let Some(branch) = epic.branch.clone() {
                return Ok((
                    branch.clone(),
                    format!(
                        "epic branch {branch} (task {task_id}'s parent epic {}; \
                         authorized for worker {assignee})",
                        epic.id
                    ),
                ));
            }
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "task {task_id}'s parent epic {} has no branch field — set the epic \
                     branch before worktree_merge.\n\n{}",
                    epic.id,
                    merge_target_remediation(assignee)
                )),
                data: None,
            });
        }

        // Standalone task (no parent epic): trunk only with allow_trunk=true.
        if allow_trunk {
            let trunk = trunk();
            return Ok((
                trunk.clone(),
                format!(
                    "trunk {trunk} (explicit allow_trunk=true; task {task_id} has no parent epic; \
                     authorized for worker {assignee})"
                ),
            ));
        }
        return Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "task {task_id} has no parent epic — refusing silent trunk merge.\n\n{}",
                merge_target_remediation(assignee)
            )),
            data: None,
        });
    }

    // No task_id: infer only from current assignee tasks. Session focus is a
    // display concern and must never redirect code (cas-b86e).
    let all_tasks = task_store.list(None).map_err(|e| McpError {
        code: ErrorCode::INTERNAL_ERROR,
        message: Cow::from(format!("Failed to list tasks for merge target: {e}")),
        data: None,
    })?;

    let mut assignee_epic_branches: Vec<(String, String, String)> = Vec::new(); // epic, branch, task
    let mut standalone_tasks: Vec<String> = Vec::new();
    let mut branchless_parent_epics: Vec<String> = Vec::new();
    let mut closed_parent_epics: Vec<(String, String)> = Vec::new(); // task, epic
    for task in &all_tasks {
        if task.assignee.as_deref() != Some(assignee) {
            continue;
        }
        if !assignee_task_is_merge_relevant(task.status) {
            continue;
        }
        // P2: surface get_parent_epic errors; reject branchless parents —
        // never silently fall through to trunk/focus (cas-0b32 review).
        match task_store.get_parent_epic(&task.id) {
            Ok(Some(epic)) => {
                if epic.status == cas_types::TaskStatus::Closed {
                    closed_parent_epics.push((task.id.clone(), epic.id.clone()));
                    continue;
                }
                if let Some(branch) = epic.branch.clone() {
                    if !assignee_epic_branches
                        .iter()
                        .any(|(id, b, task_id)| {
                            id == &epic.id && b == &branch && task_id == &task.id
                        })
                    {
                        assignee_epic_branches.push((epic.id.clone(), branch, task.id.clone()));
                    }
                } else if !branchless_parent_epics.contains(&epic.id) {
                    branchless_parent_epics.push(epic.id.clone());
                }
            }
            Ok(None) => standalone_tasks.push(task.id.clone()),
            Err(e) => {
                return Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to resolve parent epic for assignee {assignee}'s task {}: {e}\n\n{}",
                        task.id,
                        merge_target_remediation(assignee)
                    )),
                    data: None,
                });
            }
        }
    }

    if !closed_parent_epics.is_empty() {
        let targets = closed_parent_epics
            .iter()
            .map(|(task, epic)| format!("task {task}→epic {epic}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "assignee {assignee} has merge-relevant task(s) whose parent epic is Closed \
                 ({targets}) — refusing to advance a branch after an epic close receipt.\n\n{}",
                merge_target_remediation(assignee)
            )),
            data: None,
        });
    }

    // Any branchless active parent is a hard reject — even when another
    // parent has a branch (mixed case must not silently pick the branchful
    // one; cas-0b32 second-review residual AC).
    if !branchless_parent_epics.is_empty() {
        let branchful = if assignee_epic_branches.is_empty() {
            String::new()
        } else {
            format!(
                " Also has branchful parent(s): {}.",
                assignee_epic_branches
                    .iter()
                    .map(|(id, b, task)| format!("task {task}→{id}→{b}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "assignee {assignee} has active parent epic(s) without a branch field ({}) — \
                 set epic.branch (or pass task_id= for a branchful epic) before \
                 worktree_merge.{branchful}\n\n{}",
                branchless_parent_epics.join(", "),
                merge_target_remediation(assignee)
            )),
            data: None,
        });
    }

    // Dedup by branch name for uniqueness checks.
    let unique_branches: Vec<String> = {
        let mut seen = std::collections::BTreeSet::new();
        assignee_epic_branches
            .iter()
            .filter_map(|(_, b, _)| seen.insert(b.clone()).then(|| b.clone()))
            .collect()
    };

    if unique_branches.len() == 1 && standalone_tasks.is_empty() {
        let branch = unique_branches[0].clone();
        let epic_id = assignee_epic_branches
            .iter()
            .find(|(_, b, _)| b == &branch)
            .map(|(id, _, _)| id.as_str())
            .unwrap_or("?");
        let task_ids = assignee_epic_branches
            .iter()
            .filter(|(_, b, _)| b == &branch)
            .map(|(_, _, task)| task.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok((
            branch.clone(),
            format!(
                "epic branch {branch} (assignee {assignee}'s current task(s) [{task_ids}] \
                 resolve through parent epic {epic_id}; no task_id given)"
            ),
        ));
    }

    if unique_branches.len() > 1
        || (!unique_branches.is_empty() && !standalone_tasks.is_empty())
        || standalone_tasks.len() > 1
    {
        let list = assignee_epic_branches
            .iter()
            .map(|(id, b, task)| format!("task {task}→{id}→{b}"))
            .chain(
                standalone_tasks
                    .iter()
                    .map(|task| format!("task {task}→trunk")),
            )
            .collect::<Vec<_>>()
            .join(", ");
        return Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!(
                "ambiguous merge target for assignee {assignee}: multiple current task targets \
                 ({list}). Pass task_id= to disambiguate.\n\n{}",
                merge_target_remediation(assignee)
            )),
            data: None,
        });
    }

    if allow_trunk {
        let trunk = trunk();
        let task_context = standalone_tasks
            .first()
            .map(|task| format!("assignee {assignee}'s current standalone task {task}"))
            .unwrap_or_else(|| format!("no current task binding for assignee {assignee}"));
        return Ok((
            trunk.clone(),
            format!(
                "trunk {trunk} (explicit allow_trunk=true; {task_context}; no task_id and \
                 no assignee epic binding; session focus is not merge authority)"
            ),
        ));
    }

    Err(McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: Cow::from(format!(
            "no merge target for worktree assignee {assignee}: no task_id, no assignee \
             epic binding, and allow_trunk was not set — refusing silent trunk default \
             (cas-0b32/cas-b86e). Session focus is not merge authority.\n\n{}",
            merge_target_remediation(assignee)
        )),
        data: None,
    })
}

/// A worker-lane worktree must not disappear while a tracked descendant still
/// has it as its cwd. Reap only the record bound to the worktree owner's
/// factory session; when legacy worktree metadata has no owner, restrict the
/// fallback to sessions whose daemon is already dead.
async fn reap_worker_group_before_worktree_cleanup(
    cas_root: &Path,
    worktree: &crate::types::Worktree,
    agent_store: &dyn cas_store::AgentStore,
) -> Result<(), String> {
    let owner = worktree
        .created_by_agent
        .as_deref()
        .and_then(|agent_id| agent_store.get(agent_id).ok());
    let worker_name = owner
        .as_ref()
        .map(|agent| agent.name.as_str())
        .or_else(|| worktree.branch.strip_prefix("factory/"));
    let Some(worker_name) = worker_name else {
        return Ok(());
    };
    let owner_session = owner
        .as_ref()
        .and_then(|agent| agent.factory_session.as_deref());
    let running_sessions: HashSet<String> = crate::ui::factory::SessionManager::new()
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.is_running)
        .map(|session| session.name)
        .collect();

    for record in crate::ui::factory::process_groups::list(cas_root).unwrap_or_default() {
        if record.worker_name != worker_name {
            continue;
        }
        let belongs_to_lane = owner_session
            .is_some_and(|session| record.factory_session == session)
            || (owner_session.is_none() && !running_sessions.contains(&record.factory_session));
        if !belongs_to_lane {
            continue;
        }
        crate::ui::factory::process_groups::reap(cas_root, &record)
            .await
            .map_err(|error| {
                format!(
                    "worker process group {} could not be reaped before worktree removal: {error}",
                    record.pgid
                )
            })?;
    }
    Ok(())
}

impl CasCore {
    pub async fn worktree_create(&self, epic_id: &str) -> Result<CallToolResult, McpError> {
        use crate::config::Config;
        use crate::store::{open_task_store, open_worktree_store};
        use crate::worktree::{WorktreeConfig, WorktreeManager};

        let cas_root = self.cas_root.clone();
        let config = Config::load(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to load config: {e}")),
            data: None,
        })?;
        let wt_config = config.worktrees();

        if !wt_config.enabled {
            return Ok(Self::success(
                "Worktrees are not enabled. Enable in .cas/config.toml:\n  worktrees:\n    enabled: true",
            ));
        }

        // Verify epic exists
        let task_store = open_task_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open task store: {e}")),
            data: None,
        })?;
        let epic = task_store.get(epic_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Epic/task not found: {e}")),
            data: None,
        })?;

        let cwd = std::env::current_dir().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to get cwd: {e}")),
            data: None,
        })?;

        let manager_config = WorktreeConfig {
            enabled: wt_config.enabled,
            base_path: wt_config.base_path.clone(),
            branch_prefix: wt_config.branch_prefix.clone(),
            auto_merge: wt_config.auto_merge,
            cleanup_on_close: wt_config.cleanup_on_close,
            promote_entries_on_merge: wt_config.promote_entries_on_merge,
        };

        let manager = WorktreeManager::new(&cwd, manager_config).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create worktree manager: {e}")),
            data: None,
        })?;

        // Get agent ID from registered agent (flatten Option<&Option<String>>)
        let agent_id = self.agent_id.get().and_then(|o| o.as_ref());

        let worktree = manager
            .create_for_epic(epic_id, agent_id.map(|s| s.as_str()))
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to create worktree: {e}")),
                data: None,
            })?;

        // Store the worktree record
        let worktree_store = open_worktree_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open worktree store: {e}")),
            data: None,
        })?;
        worktree_store.add(&worktree).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to store worktree: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Created worktree for epic {}:\n  ID: {}\n  Branch: {}\n  Path: {}\n\ncd {} to work in the isolated worktree",
            epic.title,
            worktree.id,
            worktree.branch,
            worktree.path.display(),
            worktree.path.display()
        )))
    }

    /// List worktrees
    ///
    /// Combines the project-scoped WorktreeStore (System A) with a live
    /// `git worktree list` reconcile for CAS-pattern paths/branches that were
    /// never registered (System B factory workers, sibling-session epic
    /// worktrees). Registry rows live in `.cas/cas.db` — shared by every
    /// session in the project; git is the second source of truth when a
    /// session never wrote a row (cas-d1a0).
    pub async fn worktree_list(
        &self,
        all: bool,
        status_filter: Option<&str>,
        orphans_only: bool,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_task_store, open_worktree_store};
        use crate::types::{AgentStatus, TaskStatus, WorktreeStatus};

        let cas_root = self.cas_root.clone();
        let worktree_store = open_worktree_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open worktree store: {e}")),
            data: None,
        })?;
        let task_store = open_task_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open task store: {e}")),
            data: None,
        })?;
        let agent_store = open_agent_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open agent store: {e}")),
            data: None,
        })?;

        let parsed_status: Option<WorktreeStatus> = if let Some(status_str) = status_filter {
            Some(status_str.parse().map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Invalid status: {status_str}")),
                data: None,
            })?)
        } else {
            None
        };

        let mut worktrees = if let Some(status) = parsed_status {
            worktree_store
                .list_by_status(status)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list worktrees: {e}")),
                    data: None,
                })?
        } else if all {
            worktree_store.list().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list worktrees: {e}")),
                data: None,
            })?
        } else {
            worktree_store.list_active().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list worktrees: {e}")),
                data: None,
            })?
        };

        // Live git reconcile only for views that include active worktrees.
        // Non-active status filters (merged/abandoned/…) must not gain
        // transient Active git rows.
        let should_reconcile_git = match parsed_status {
            None => true,
            Some(WorktreeStatus::Active) => true,
            Some(_) => false,
        };

        let factory_base = resolve_factory_worktree_base(&cas_root);
        if should_reconcile_git {
            let tracked_branches: HashSet<String> =
                worktrees.iter().map(|wt| wt.branch.clone()).collect();
            let tracked_paths: HashSet<PathBuf> =
                worktrees.iter().map(|wt| wt.path.clone()).collect();
            worktrees.extend(collect_untracked_git_worktrees(
                &cas_root,
                &factory_base,
                &tracked_branches,
                &tracked_paths,
            ));
        }

        // Filter orphans if requested
        let worktrees: Vec<_> = if orphans_only {
            worktrees
                .into_iter()
                .filter(|wt| {
                    if wt.status != WorktreeStatus::Active {
                        return false;
                    }
                    if !wt.path.exists() {
                        return true;
                    }
                    if let Some(ref epic_id) = wt.epic_id {
                        if let Ok(epic) = task_store.get(epic_id) {
                            if matches!(epic.status, TaskStatus::Closed) {
                                return true;
                            }
                        }
                    }
                    if let Some(ref agent_id) = wt.created_by_agent {
                        if let Ok(agent) = agent_store.get(agent_id) {
                            if matches!(agent.status, AgentStatus::Stale | AgentStatus::Shutdown) {
                                return true;
                            }
                        }
                    }
                    false
                })
                .collect()
        } else {
            worktrees
        };

        if worktrees.is_empty() {
            return Ok(Self::success("No worktrees found."));
        }

        let mut output = format!("WORKTREES ({})\n\n", worktrees.len());
        for wt in &worktrees {
            let status_icon = match wt.status {
                WorktreeStatus::Active => "🟢",
                WorktreeStatus::Merged => "✅",
                WorktreeStatus::Abandoned => "⚠️",
                WorktreeStatus::Conflict => "❌",
                WorktreeStatus::Removed => "🗑️",
            };
            let path_status = if wt.path.exists() { "" } else { " (missing)" };
            // git: prefix = reconciled from live git (not in WorktreeStore).
            // Factory-style vs other CAS patterns get distinct labels so
            // supervisors can tell spawn workers from untracked epic trees.
            let type_indicator = if wt.id.starts_with("git:") {
                if is_factory_style_worktree(&wt.path, &wt.branch, &cas_root, &factory_base) {
                    " [factory]"
                } else {
                    " [untracked]"
                }
            } else {
                ""
            };
            output.push_str(&format!(
                "{} {} - {} {}{}{}\n   Epic: {}\n   Path: {}\n\n",
                status_icon,
                wt.id,
                wt.branch,
                wt.status,
                path_status,
                type_indicator,
                wt.epic_id.as_deref().unwrap_or("-"),
                wt.path.display()
            ));
        }

        Ok(Self::success(output))
    }

    /// Show worktree details
    pub async fn worktree_show(&self, id: &str) -> Result<CallToolResult, McpError> {
        use crate::store::open_worktree_store;

        let cas_root = self.cas_root.clone();
        let worktree_store = open_worktree_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open worktree store: {e}")),
            data: None,
        })?;

        let worktree = match worktree_store.get(id) {
            Ok(wt) => wt,
            Err(_) => worktree_store
                .get_by_branch(id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to get worktree: {e}")),
                    data: None,
                })?
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Worktree not found: {id}")),
                    data: None,
                })?,
        };

        let path_exists = worktree.path.exists();
        Ok(Self::success(format!(
            "Worktree: {}\n\nBranch: {}\nParent: {}\nStatus: {}\nPath: {} {}\nEpic: {}\nCreated by: {}\nCreated: {}",
            worktree.id,
            worktree.branch,
            worktree.parent_branch,
            worktree.status,
            worktree.path.display(),
            if path_exists { "" } else { "(missing)" },
            worktree.epic_id.as_deref().unwrap_or("-"),
            worktree.created_by_agent.as_deref().unwrap_or("-"),
            worktree.created_at.format("%Y-%m-%d %H:%M UTC")
        )))
    }

    /// Cleanup orphaned worktrees
    /// Remove worktrees.
    ///
    /// `id = None` is the historical System-A orphan sweep: every
    /// `WorktreeStore` row whose path is gone, whose epic closed, or whose
    /// creating agent went Stale/Shutdown.
    ///
    /// `id = Some(..)` (cas-f102, GH #140) targets ONE worktree and resolves it
    /// the way [`Self::worktree_merge`] does — System A (`WorktreeStore` by id,
    /// then by branch) first, then the System-B `spawn_workers isolate=true`
    /// convention at `worktree_path_for_worker(assignee)`. cas-1d11 exempted
    /// merge/list/status from the `worktrees.enabled` gate but left cleanup
    /// behind on the premise that it had "no System-B analogue"; a retired
    /// worker's worktree outlives its worker unless `cleanup=true` was passed at
    /// merge time, which is precisely that analogue, and without this path the
    /// only remaining option was a manual `git worktree remove` that bypasses
    /// factory tracking.
    ///
    /// Refusals, in the order they are checked and always before anything is
    /// destroyed:
    /// - the assignee is a live agent (Active/Idle) — `force` does NOT override
    ///   this, exactly as `force` does not stand in for `allow_trunk` in merge;
    /// - the branch's commits exist on no other local branch — removal would
    ///   delete them, since `abandon` deletes the branch with `-D`;
    /// - the worktree is dirty (`abandon`'s own gate).
    ///
    /// Only the dirty and unmerged refusals are bypassable with `force`.
    pub async fn worktree_cleanup(
        &self,
        id: Option<&str>,
        dry_run: bool,
        force: bool,
    ) -> Result<CallToolResult, McpError> {
        if let Some(id) = id {
            return self.worktree_cleanup_target(id, dry_run, force).await;
        }
        use crate::config::Config;
        use crate::store::{open_agent_store, open_task_store, open_worktree_store};
        use crate::types::{AgentStatus, TaskStatus};
        use crate::worktree::{WorktreeConfig, WorktreeManager};

        let cas_root = self.cas_root.clone();
        let config = Config::load(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to load config: {e}")),
            data: None,
        })?;
        let wt_config = config.worktrees();

        let worktree_store = open_worktree_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open worktree store: {e}")),
            data: None,
        })?;
        let task_store = open_task_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open task store: {e}")),
            data: None,
        })?;
        let agent_store = open_agent_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open agent store: {e}")),
            data: None,
        })?;

        let active_worktrees = worktree_store.list_active().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list worktrees: {e}")),
            data: None,
        })?;

        // Find orphans
        let orphans: Vec<_> = active_worktrees
            .into_iter()
            .filter(|wt| {
                if !wt.path.exists() {
                    return true;
                }
                if let Some(ref epic_id) = wt.epic_id {
                    if let Ok(epic) = task_store.get(epic_id) {
                        if matches!(epic.status, TaskStatus::Closed) {
                            return true;
                        }
                    }
                }
                if let Some(ref agent_id) = wt.created_by_agent {
                    if let Ok(agent) = agent_store.get(agent_id) {
                        if matches!(agent.status, AgentStatus::Stale | AgentStatus::Shutdown) {
                            return true;
                        }
                    }
                }
                false
            })
            .collect();

        if orphans.is_empty() {
            return Ok(Self::success("No orphaned worktrees to clean up."));
        }

        if dry_run {
            let mut output = format!("Would clean up {} worktree(s):\n\n", orphans.len());
            for wt in &orphans {
                output.push_str(&format!("  {} - {}\n", wt.id, wt.branch));
            }
            output.push_str("\nRun with dry_run=false to actually clean up.");
            return Ok(Self::success(output));
        }

        let cwd = std::env::current_dir().unwrap_or_default();
        let manager_config = WorktreeConfig {
            enabled: wt_config.enabled,
            base_path: wt_config.base_path.clone(),
            branch_prefix: wt_config.branch_prefix.clone(),
            auto_merge: wt_config.auto_merge,
            cleanup_on_close: wt_config.cleanup_on_close,
            promote_entries_on_merge: wt_config.promote_entries_on_merge,
        };

        let mut cleaned = 0;
        let mut errors = Vec::new();

        for mut wt in orphans {
            if let Err(error) =
                reap_worker_group_before_worktree_cleanup(&cas_root, &wt, agent_store.as_ref())
                    .await
            {
                errors.push(format!("{} ({error})", wt.id));
                continue;
            }
            if wt.path.exists() {
                if let Ok(manager) = WorktreeManager::new(&cwd, manager_config.clone()) {
                    if manager.abandon(&mut wt, force).is_ok() {
                        wt.mark_abandoned();
                        wt.mark_removed();
                        let _ = worktree_store.update(&wt);
                        cleaned += 1;
                        continue;
                    }
                }
            }
            // Just mark in store if physical cleanup failed
            wt.mark_abandoned();
            wt.mark_removed();
            if worktree_store.update(&wt).is_ok() {
                cleaned += 1;
            } else {
                errors.push(wt.id.clone());
            }
        }

        if errors.is_empty() {
            Ok(Self::success(format!("Cleaned up {cleaned} worktree(s).")))
        } else {
            Ok(Self::success(format!(
                "Cleaned up {} worktree(s), {} error(s): {}",
                cleaned,
                errors.len(),
                errors.join(", ")
            )))
        }
    }

    /// cas-f102 (GH #140): remove ONE worktree, resolved System A then System B.
    async fn worktree_cleanup_target(
        &self,
        id: &str,
        dry_run: bool,
        force: bool,
    ) -> Result<CallToolResult, McpError> {
        use crate::config::Config;
        use crate::store::{open_agent_store, open_worktree_store};
        use crate::worktree::{WorktreeConfig, WorktreeManager};

        let cas_root = self.cas_root.clone();
        let config = Config::load(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to load config: {e}")),
            data: None,
        })?;
        let wt_config = config.worktrees();

        let worktree_store = open_worktree_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open worktree store: {e}")),
            data: None,
        })?;
        let agent_store = open_agent_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open agent store: {e}")),
            data: None,
        })?;

        let cwd = cas_root.parent().unwrap_or(&cas_root).to_path_buf();
        let manager_config = WorktreeConfig {
            enabled: wt_config.enabled,
            base_path: wt_config.base_path.clone(),
            branch_prefix: wt_config.branch_prefix.clone(),
            auto_merge: wt_config.auto_merge,
            cleanup_on_close: wt_config.cleanup_on_close,
            promote_entries_on_merge: wt_config.promote_entries_on_merge,
        };
        let manager = WorktreeManager::new(&cwd, manager_config).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create worktree manager: {e}")),
            data: None,
        })?;

        // Same resolution order as worktree_merge: System A by id, then by
        // branch, then the System-B convention.
        let system_a = match worktree_store.get(id) {
            Ok(wt) => Some(wt),
            Err(_) => worktree_store.get_by_branch(id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get worktree: {e}")),
                data: None,
            })?,
        };

        let (mut worktree, is_system_b) = match system_a {
            Some(wt) => (wt, false),
            None => {
                let assignee = id.strip_prefix("factory/").unwrap_or(id);
                let path = manager.worktree_path_for_worker(assignee);
                if !is_git_worktree(&path) {
                    // Accurate not-found, never the "disabled" text (AC2).
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Worktree not found: {id} (checked System A worktree store and \
                             the System B path {})",
                            path.display()
                        )),
                        data: None,
                    });
                }
                (
                    crate::types::Worktree::new(
                        format!("system-b-{assignee}"),
                        format!("factory/{assignee}"),
                        String::new(),
                        path,
                    ),
                    true,
                )
            }
        };

        let worker_name = worktree
            .created_by_agent
            .as_deref()
            .and_then(|agent_id| agent_store.get(agent_id).ok())
            .map(|agent| agent.name)
            .or_else(|| worktree.branch.strip_prefix("factory/").map(str::to_string));

        // Refusal 1: the assignee is still alive. `force` deliberately does not
        // override this — it would yank the working directory out from under a
        // running worker mid-turn. force stays a dirty-tree bypass only.
        if let Some(name) = worker_name.as_deref()
            && let Ok(agents) = agent_store.list(None)
            && let Some(agent) = agents.iter().find(|a| a.name == name)
            && matches!(
                agent.status,
                cas_types::AgentStatus::Active | cas_types::AgentStatus::Idle
            )
        {
            return Ok(Self::success(format!(
                "Refused: {name} is still a live agent (status {:?}), and {} is its working \
                 directory.\n\nShut the worker down first (`coordination action=shutdown_workers \
                 worker_names={name}`), then retry. `force=true` does NOT override this — it only \
                 bypasses the dirty-worktree check.",
                agent.status,
                worktree.path.display()
            )));
        }

        // Refusal 2: the branch's commits live nowhere else. `abandon` deletes
        // the branch with `-D`, so removing this worktree would destroy them.
        let containers = manager.git().branches_containing(&worktree.branch);
        let branch_is_reachable = !containers.is_empty();
        if !branch_is_reachable && !force {
            return Ok(Self::success(format!(
                "Refused: {} has commits that exist on no other branch, and cleanup deletes the \
                 branch.\n\nMerge it first (`coordination action=worktree_merge id={id}`), or pass \
                 force=true to discard the commits.\n\nWorktree: {}",
                worktree.branch,
                worktree.path.display()
            )));
        }

        let system = if is_system_b { "System B" } else { "System A" };
        let reachable_via = if branch_is_reachable {
            format!("merged — reachable from {}", containers.join(", "))
        } else {
            "UNMERGED (force=true supplied)".to_string()
        };

        if dry_run {
            return Ok(Self::success(format!(
                "Would remove {system} worktree:\n\n  path:   {}\n  branch: {} ({reachable_via})\n\
                 \nRun with dry_run=false to actually remove it.",
                worktree.path.display(),
                worktree.branch
            )));
        }

        // Reap any surviving process group before pulling the directory out
        // from under it — same guard the sweep uses.
        if let Err(error) =
            reap_worker_group_before_worktree_cleanup(&cas_root, &worktree, agent_store.as_ref())
                .await
        {
            return Ok(Self::success(format!(
                "Refused: {error}\n\nWorktree {} was left in place.",
                worktree.path.display()
            )));
        }

        // Refusal 3 (dirty without force) is `abandon`'s own gate.
        if let Err(error) = manager.abandon(&mut worktree, force) {
            return Ok(Self::success(format!(
                "Could not remove {system} worktree {}: {error}\n\nNothing was changed. Pass \
                 force=true to remove a dirty worktree.",
                worktree.path.display()
            )));
        }

        if !is_system_b {
            let _ = worktree_store.update(&worktree);
        }

        Ok(Self::success(format!(
            "Removed {system} worktree.\n\n  path:   {}\n  branch: {} (deleted; was {reachable_via})\n\
             {}",
            worktree.path.display(),
            worktree.branch,
            if is_system_b {
                "\nSystem-B worktrees carry no store row, so nothing further was updated."
            } else {
                "\nStore row marked abandoned + removed."
            }
        )))
    }

    /// Merge worktree back to parent
    ///
    /// Resolves `id` against System A first (the `WorktreeStore`-tracked,
    /// `worktrees.enabled`-gated worktrees created by `worktree_create`).
    /// When that lookup misses, falls back to System B — the
    /// `spawn_workers isolate=true` convention (branch `factory/<assignee>`,
    /// path resolved via `WorktreeManager::worktree_path_for_worker` so a
    /// customized `worktrees.base_path` still resolves correctly), which is
    /// never registered in the store and doesn't check `worktrees.enabled`
    /// at all (cas-1d11). Without this fallback, spawn happily created
    /// isolated worktrees while the only supervisor-callable merge action
    /// refused every one of them — forcing a manual `git worktree add` +
    /// merge + push that bypassed factory tracking/lease/cleanup entirely.
    ///
    /// A System-B merge target is resolved via `task_id`, then the assignee's
    /// current task binding (cas-0938 + cas-0b32 + cas-b86e). Session epic
    /// focus is never merge authority. Never silently defaults a factory
    /// worker merge to trunk — trunk requires explicit `allow_trunk=true`
    /// (independent of `force`, which only bypasses dirty worktree protection).
    /// The resolved task/target is always surfaced.
    ///
    /// `cleanup` (cas-369f) is independent of `force`:
    /// - `force` only allows merging a dirty worktree
    /// - `cleanup=true` removes the worktree + deletes the branch after merge
    /// - System-B default is **preserve** (mid-session merges leave the
    ///   worker cwd intact); System-A uses `worktrees.cleanup_on_close`
    pub async fn worktree_merge(
        &self,
        id: &str,
        force: bool,
        task_id: Option<&str>,
        allow_trunk: bool,
        cleanup: Option<bool>,
    ) -> Result<CallToolResult, McpError> {
        use crate::config::Config;
        use crate::store::open_worktree_store;
        use crate::worktree::{WorktreeConfig, WorktreeManager};

        let cas_root = self.cas_root.clone();
        let config = Config::load(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to load config: {e}")),
            data: None,
        })?;
        let wt_config = config.worktrees();
        let transactional_delivery =
            match task_id {
                Some(task_id) => cas_store::get_latest_worker_delivery(&cas_root, task_id)
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to load transactional worker delivery for {task_id}: {error}"
                        )),
                        data: None,
                    })?,
                None => None,
            };
        let delivery_authority = if transactional_delivery.is_some() {
            let caller_id = self.get_agent_id().map_err(|_| McpError {
                code: ErrorCode::INVALID_REQUEST,
                message: Cow::from(
                    "Transactional delivery merge requires an authenticated registered supervisor.",
                ),
                data: None,
            })?;
            let caller = self
                .open_agent_store()?
                .get(&caller_id)
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(
                        "Transactional delivery merge requires a live registered supervisor session.",
                    ),
                    data: None,
                })?;
            Some(
                derive_delivery_supervisor_authority(&caller).map_err(|reason| McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(format!(
                        "Transactional delivery merge authority rejected: {reason}; request flags, environment role labels, and task ownership do not grant authority."
                    )),
                    data: None,
                })?,
            )
        } else {
            None
        };
        if let Some((_, transaction)) = transactional_delivery.as_ref()
            && transaction.state == cas_types::WorkerDeliveryState::Delivered
        {
            return Ok(Self::success(format!(
                "Transactional delivery {} is already delivered; no Git merge, event, or close was repeated.",
                transaction.id
            )));
        }

        let worktree_store = open_worktree_store(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open worktree store: {e}")),
            data: None,
        })?;

        // Resolve an explicit task target before constructing any git-bound
        // object. The manager owns preflight, checkout, merge, and cleanup,
        // so constructing it from the spawn repo would mutate the wrong
        // checkout even if a later identity-only validation succeeded.
        let declared_repo_context = match task_id {
            Some(task_id) => {
                let task_store = self.open_task_store()?;
                let task = task_store.get(task_id).map_err(|error| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "worktree_merge cannot load task {task_id} for repository binding: {error}"
                    )),
                    data: None,
                })?;
                match task.deliverables.work_target.as_ref() {
                    Some(target) => Some(
                        crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                            &self.cas_root,
                            target,
                        )
                        .map_err(|message| McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(message),
                            data: None,
                        })?,
                    ),
                    None => None,
                }
            }
            None => None,
        };
        let cwd = declared_repo_context
            .as_ref()
            .map(|context| context.repo_root.clone())
            .unwrap_or_else(|| cas_root.parent().unwrap_or(&cas_root).to_path_buf());

        let manager_config = WorktreeConfig {
            enabled: wt_config.enabled,
            base_path: wt_config.base_path.clone(),
            branch_prefix: wt_config.branch_prefix.clone(),
            auto_merge: true, // Force merge for this operation
            cleanup_on_close: wt_config.cleanup_on_close,
            promote_entries_on_merge: wt_config.promote_entries_on_merge,
        };

        let manager = WorktreeManager::new(&cwd, manager_config).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create worktree manager: {e}")),
            data: None,
        })?;

        let system_a = match worktree_store.get(id) {
            Ok(wt) => Some(wt),
            Err(_) => worktree_store.get_by_branch(id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get worktree: {e}")),
                data: None,
            })?,
        };

        let (mut worktree, is_system_b, source_worktree_live, target_reason) = match system_a {
            Some(wt) => {
                let source_worktree_live = is_git_worktree(&wt.path);
                (wt, false, source_worktree_live, String::new())
            }
            None => {
                let assignee = id.strip_prefix("factory/").unwrap_or(id);
                let path = manager.worktree_path_for_worker(assignee);
                let source_worktree_live = is_git_worktree(&path);
                if !source_worktree_live && transactional_delivery.is_none() {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Worktree not found: {id} (checked System A worktree store and \
                             the System B path {})",
                            path.display()
                        )),
                        data: None,
                    });
                }
                let task_store = self.open_task_store()?;
                // cas-bd5f: agent store needed to bind explicit task_id to the
                // System-B worker (assignee name and/or active lease holder).
                let agent_store =
                    crate::store::open_agent_store(&cas_root).map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to open agent store for worktree_merge authorization: {e}"
                        )),
                        data: None,
                    })?;
                let (resolved_parent_branch, mut target_reason) = resolve_system_b_merge_target(
                    task_store.as_ref(),
                    agent_store.as_ref(),
                    task_id,
                    assignee,
                    allow_trunk, // NOT force — dirty bypass stays separate (cas-0b32 review P1)
                    || {
                        Config::configured_epic_base_branch(&cwd)
                            .unwrap_or_else(|| manager.git().detect_default_branch())
                    },
                )?;
                let parent_branch = match declared_repo_context.as_ref() {
                    Some(context) => {
                        target_reason = format!(
                            "task WorkTarget {} branch {}",
                            context.repo_selector, context.target_branch
                        );
                        context.target_branch.clone()
                    }
                    None => resolved_parent_branch,
                };
                (
                    crate::types::Worktree::new(
                        format!("system-b-{assignee}"),
                        format!("factory/{assignee}"),
                        parent_branch,
                        path,
                    ),
                    true,
                    source_worktree_live,
                    target_reason,
                )
            }
        };

        // Bind this mutation to the task's declared work repository before
        // merge/reachability checks. A cleaned-up source is the sole
        // exception: immutable receipt and target ancestry are authenticated
        // below before source-less reconciliation can proceed.
        if source_worktree_live
            && let (Some(task_id), Some(expected)) = (task_id, declared_repo_context.as_ref())
        {
            let actual = crate::mcp::tools::core::task::repo_context::resolve_path_context(
                &worktree.path,
                &worktree.parent_branch,
            )
            .map_err(|reason| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "⚠️ WORKTREE REPOSITORY MISMATCH\n\n\
                     Cannot resolve repository identity for {}: {reason}. \
                     Refusing before merge/reachability checks.",
                    worktree.path.display()
                )),
                data: None,
            })?;
            crate::mcp::tools::core::task::repo_context::validate_worktree_binding(
                task_id,
                expected,
                &actual,
                &worktree.parent_branch,
                &worktree.path,
            )
            .map_err(|message| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(message),
                data: None,
            })?;
        }

        // A stale System-A row is not a live worktree. Preserve the legacy
        // no-receipt failure while transactional delivery continues into the
        // immutable receipt/ancestry reconciliation below.
        if !source_worktree_live && transactional_delivery.is_none() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Worktree not found: {id} (checked System A worktree store and \
                     the System B path {})",
                    worktree.path.display()
                )),
                data: None,
            });
        }

        let mut reconciled_delivery = false;
        let mut delivery_close_result = None;
        // cas-0a21: held from before the target tip is first read until after
        // the post-merge delivery state is durable, so no other CAS-mediated
        // merge can move this repository's target ref inside the window.
        // Bound to the function scope so it releases on every exit path.
        let _delivery_target_lock;
        if let (Some((receipt, transaction)), Some(authority)) =
            (transactional_delivery.as_ref(), delivery_authority.as_ref())
        {
            let canonical_repo =
                manager
                    .git()
                    .canonical_repo_key()
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Transactional delivery cannot resolve canonical repository \
                             identity for the merge target: {error}"
                        )),
                        data: None,
                    })?;
            _delivery_target_lock = crate::worktree::target_lock::lock_delivery_target(
                &cas_root,
                &canonical_repo,
                &receipt.target_branch,
            )
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Transactional delivery could not acquire the target-ref lock: {error}"
                )),
                data: None,
            })?;

            let fail = |state: cas_types::WorkerDeliveryState,
                        code: &'static str,
                        detail: String|
             -> Result<CallToolResult, McpError> {
                cas_store::transition_worker_delivery(
                    &cas_root,
                    &transaction.id,
                    &[
                        cas_types::WorkerDeliveryState::AwaitingMerge,
                        cas_types::WorkerDeliveryState::MergeAuthorized,
                    ],
                    state,
                    &authority.agent_id,
                    Some(&authority.agent_id),
                    None,
                    None,
                    Some((code, &detail)),
                )
                .map_err(|error| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to persist delivery failure state: {error}"
                    )),
                    data: None,
                })?;
                Ok(Self::tool_error(format!(
                    "TRANSACTIONAL DELIVERY {state}: {detail}\n\nNo merge was attempted. A registered supervisor may recover after correcting the exact cause."
                )))
            };
            if !matches!(
                transaction.state,
                cas_types::WorkerDeliveryState::AwaitingMerge
                    | cas_types::WorkerDeliveryState::MergeAuthorized
                    | cas_types::WorkerDeliveryState::Merged
                    | cas_types::WorkerDeliveryState::CloseReady
            ) {
                return Ok(Self::tool_error(format!(
                    "Transactional delivery {} is in state {}; merge/resume is not authorized from this state.",
                    transaction.id, transaction.state
                )));
            }
            let Some(expected) = declared_repo_context.as_ref() else {
                return fail(
                    cas_types::WorkerDeliveryState::RepoMismatch,
                    "repo_mismatch",
                    "receipt requires a declared task RepoContext, but none resolved".to_string(),
                );
            };
            let repo_binding_matches = receipt.repo_selector == expected.repo_selector
                && receipt.target_branch == expected.target_branch
                && receipt.source_branch == worktree.branch
                && receipt.target_branch == worktree.parent_branch;
            let commit_exists =
                crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                    &cwd,
                    &receipt.commit_sha,
                )
                .is_some();
            let target_tip =
                crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                    &cwd,
                    &receipt.target_branch,
                );
            let already_merged = target_tip.as_deref().is_some_and(|target| {
                crate::mcp::tools::core::task::lifecycle::close_ops::git_commit_is_ancestor(
                    &cwd,
                    &receipt.commit_sha,
                    target,
                )
            });
            let source_tip =
                crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                    &cwd,
                    &receipt.source_branch,
                );
            let live_base = crate::mcp::tools::core::task::lifecycle::close_ops::git_merge_base(
                &cwd,
                &receipt.source_branch,
                &receipt.target_branch,
            );
            let preflight = classify_delivery_merge_preflight(
                repo_binding_matches,
                commit_exists,
                already_merged,
                source_tip.as_deref() == Some(receipt.commit_sha.as_str()),
                target_tip.as_deref() == Some(receipt.target_sha.as_str()),
                live_base.as_deref() == Some(receipt.merge_base_sha.as_str()),
            );
            let preflight = match preflight {
                Ok(preflight) => preflight,
                Err((state, code, detail)) => {
                    return fail(state, code, detail.to_string());
                }
            };
            if preflight == DeliveryMergePreflight::Execute {
                cas_store::transition_worker_delivery(
                    &cas_root,
                    &transaction.id,
                    &[cas_types::WorkerDeliveryState::AwaitingMerge],
                    cas_types::WorkerDeliveryState::MergeAuthorized,
                    &authority.agent_id,
                    Some(&authority.agent_id),
                    None,
                    None,
                    None,
                )
                .map_err(|error| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to persist delivery merge intent: {error}")),
                    data: None,
                })?;
            } else {
                reconciled_delivery = true;
            }
        }

        if !source_worktree_live && !reconciled_delivery {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Worktree not found: {id} (checked System A worktree store and \
                     the System B path {}). Transactional delivery cannot continue \
                     without a live source unless the authenticated receipt commit \
                     is already an ancestor of its declared target.",
                    worktree.path.display()
                )),
                data: None,
            });
        }

        // cas-369f: force (dirty) ≠ cleanup (remove). System-B factory
        // workers default to preserving the worktree mid-session.
        let do_cleanup =
            resolve_worktree_merge_cleanup(cleanup, is_system_b, wt_config.cleanup_on_close);

        let merge_commit = if reconciled_delivery {
            crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                &cwd,
                &worktree.parent_branch,
            )
        } else {
            reject_closed_epic_merge_target(
                self.open_task_store()?.as_ref(),
                &worktree.parent_branch,
            )?;

            // cas-0a21: final compare-and-swap read of the target ref, taken
            // under the target lock immediately before Git runs. Any drift
            // that landed since preflight is refused here, so the common case
            // never reaches Git at all and needs no rollback.
            if let (Some((receipt, transaction)), Some(authority)) =
                (transactional_delivery.as_ref(), delivery_authority.as_ref())
            {
                let live_target = manager.git().resolve_commit(&receipt.target_branch);
                if live_target.as_deref() != Some(receipt.target_sha.as_str()) {
                    let _ = cas_store::transition_worker_delivery(
                        &cas_root,
                        &transaction.id,
                        &[
                            cas_types::WorkerDeliveryState::AwaitingMerge,
                            cas_types::WorkerDeliveryState::MergeAuthorized,
                        ],
                        cas_types::WorkerDeliveryState::TipChanged,
                        &authority.agent_id,
                        Some(&authority.agent_id),
                        None,
                        None,
                        Some((
                            "target_tip_changed",
                            "target ref moved after preflight; no merge was attempted",
                        )),
                    );
                    return Ok(Self::tool_error(
                        "TRANSACTIONAL DELIVERY tip_changed: the target ref moved between \
                         preflight and merge.\n\nNo merge was attempted and no delivery was \
                         recorded. Re-review the worker commit against the new target tip, \
                         then retry."
                            .to_string(),
                    ));
                }
            }

            let merge_result = if transactional_delivery.is_some() {
                // Persisted delivery state must separate a successful Git
                // merge from destructive cleanup.
                manager.merge_preserving_worktree(&mut worktree, force, do_cleanup)
            } else {
                manager.merge_and_cleanup(&mut worktree, force, do_cleanup)
            };
            match merge_result {
                Ok(commit) => commit,
                // cas-4702: the ephemeral-worktree merge lost its
                // compare-and-swap because the target ref moved while Git was
                // running. That is target drift, not a content conflict — it
                // must reach the supervisor as the recoverable TipChanged
                // state, exactly like drift caught before the merge.
                Err(crate::worktree::WorktreeError::Git(
                    crate::worktree::GitError::TargetTipChanged { .. },
                )) => {
                    if let (Some((_, transaction)), Some(authority)) =
                        (transactional_delivery.as_ref(), delivery_authority.as_ref())
                    {
                        let _ = cas_store::transition_worker_delivery(
                            &cas_root,
                            &transaction.id,
                            &[
                                cas_types::WorkerDeliveryState::AwaitingMerge,
                                cas_types::WorkerDeliveryState::MergeAuthorized,
                            ],
                            cas_types::WorkerDeliveryState::TipChanged,
                            &authority.agent_id,
                            Some(&authority.agent_id),
                            None,
                            None,
                            Some((
                                "target_tip_changed",
                                "target ref moved while the merge ran; the merge was discarded",
                            )),
                        );
                    }
                    return Ok(Self::tool_error(
                        "TRANSACTIONAL DELIVERY tip_changed: the target ref moved while the \
                         merge was running.\n\nThe merge was computed in an ephemeral worktree \
                         and discarded rather than published over the concurrent update, so no \
                         delivery was recorded and the target still carries the other writer's \
                         commit. Re-review the worker commit against the new target tip, then \
                         retry."
                            .to_string(),
                    ));
                }
                Err(error) => {
                    if let (Some((_, transaction)), Some(authority)) =
                        (transactional_delivery.as_ref(), delivery_authority.as_ref())
                    {
                        // Keep durable delivery events portable and diagnostic-only:
                        // the user-facing MCP error below may name local paths, but
                        // SQLite stores no path-bearing Git error payload.
                        let detail =
                            "Git merge did not complete; explicit conflict/recovery is required.";
                        let _ = cas_store::transition_worker_delivery(
                            &cas_root,
                            &transaction.id,
                            &[cas_types::WorkerDeliveryState::MergeAuthorized],
                            cas_types::WorkerDeliveryState::Conflict,
                            &authority.agent_id,
                            Some(&authority.agent_id),
                            None,
                            None,
                            Some(("merge_conflict", detail)),
                        );
                    }
                    return Err(worktree_merge_mcp_error(
                        error,
                        &worktree.branch,
                        &worktree.parent_branch,
                    ));
                }
            }
        };

        // cas-5ee0 (GH #137): the merge receipt must tell the truth about push
        // state. Filled in by the transactional branch below (which has to
        // publish *before* its internal close runs, or that close measures a
        // target the rest of the world cannot see); otherwise resolved once in
        // the shared tail.
        let mut target_push: Option<(String, crate::worktree::git::TargetPushOutcome)> = None;

        if let (Some((receipt, transaction)), Some(authority)) =
            (transactional_delivery.as_ref(), delivery_authority.as_ref())
        {
            let target_tip =
                crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                    &cwd,
                    &receipt.target_branch,
                )
                .ok_or_else(|| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(
                        "Transactional delivery cannot resolve target tip after merge.",
                    ),
                    data: None,
                })?;
            if !crate::mcp::tools::core::task::lifecycle::close_ops::git_commit_is_ancestor(
                &cwd,
                &receipt.commit_sha,
                &target_tip,
            ) {
                return Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(
                        "Transactional delivery refused to mark merged: exact receipt commit is not an ancestor of the target after Git returned.",
                    ),
                    data: None,
                });
            }

            // cas-0a21: ancestry alone cannot prove the merge was rooted at
            // the *reviewed* target — a merge that swept in a concurrent
            // commit still leaves the receipt commit an ancestor of the new
            // tip. Only first-parent identity pins the topology. This is the
            // check that catches drift landing inside the merge itself, which
            // the target lock cannot prevent (a non-CAS actor can always move
            // the ref).
            //
            // `reconciled_delivery` resumes are exempt: they intentionally
            // observe an already-merged history rather than creating one.
            if !reconciled_delivery {
                let first_parent = manager.git().first_parent(&target_tip);
                if first_parent.as_deref() != Some(receipt.target_sha.as_str()) {
                    // Undo exactly our merge and nothing else: reset the
                    // target to the merge's own first parent via git's
                    // compare-and-swap. That preserves the concurrent actor's
                    // commit — resetting to receipt.target_sha would destroy
                    // it. If the CAS fails, the ref moved again; leave it
                    // alone and report rather than clobber a third writer.
                    let rollback = first_parent.as_deref().map(|parent| {
                        manager.git().rollback_branch_to(
                            &receipt.target_branch,
                            parent,
                            &target_tip,
                        )
                    });
                    let rolled_back = matches!(rollback, Some(Ok(())));
                    let _ = cas_store::transition_worker_delivery(
                        &cas_root,
                        &transaction.id,
                        &[
                            cas_types::WorkerDeliveryState::AwaitingMerge,
                            cas_types::WorkerDeliveryState::MergeAuthorized,
                        ],
                        cas_types::WorkerDeliveryState::TipChanged,
                        &authority.agent_id,
                        Some(&authority.agent_id),
                        None,
                        None,
                        Some((
                            "target_tip_changed",
                            if rolled_back {
                                "target ref moved during the merge; the merge was rolled back"
                            } else {
                                "target ref moved during the merge; automatic rollback was declined"
                            },
                        )),
                    );
                    return Ok(Self::tool_error(format!(
                        "TRANSACTIONAL DELIVERY tip_changed: the target ref moved during the \
                         merge, so the resulting merge was not rooted at the reviewed target.\n\n\
                         {}\n\nNo delivery was recorded. Re-review the worker commit against \
                         the new target tip, then retry.",
                        if rolled_back {
                            "The merge has been rolled back; the concurrent commit was preserved."
                        } else {
                            "The merge could NOT be rolled back automatically because the target \
                             moved again. Inspect the target branch before retrying."
                        }
                    )));
                }
            }
            // cas-5ee0 (GH #137): publish the target ref HERE — after every
            // drift/rollback check has cleared, and before the internal close
            // below. The close merge-state guard measures ancestry against
            // both `<target>` and `origin/<target>`; a merge that moved only
            // the local ref is invisible to any other checkout and produced a
            // guaranteed close-rejection loop that only a manual
            // `git push origin <target>` could break. Publishing before the
            // close is what makes the receipt and the guard agree.
            target_push = Some((
                receipt.target_branch.clone(),
                manager
                    .git()
                    .publish_branch_to_origin(&receipt.target_branch),
            ));

            if transaction.state != cas_types::WorkerDeliveryState::CloseReady {
                cas_store::transition_worker_delivery(
                    &cas_root,
                    &transaction.id,
                    &[
                        cas_types::WorkerDeliveryState::MergeAuthorized,
                        cas_types::WorkerDeliveryState::AwaitingMerge,
                        cas_types::WorkerDeliveryState::Merged,
                    ],
                    cas_types::WorkerDeliveryState::Merged,
                    &authority.agent_id,
                    Some(&authority.agent_id),
                    None,
                    Some(&target_tip),
                    None,
                )
                .and_then(|_| {
                    cas_store::transition_worker_delivery(
                        &cas_root,
                        &transaction.id,
                        &[cas_types::WorkerDeliveryState::Merged],
                        cas_types::WorkerDeliveryState::CloseReady,
                        &authority.agent_id,
                        Some(&authority.agent_id),
                        None,
                        Some(&target_tip),
                        None,
                    )
                })
                .map_err(|error| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to persist post-merge delivery state: {error}"
                    )),
                    data: None,
                })?;
            }

            // Cleanup is destructive and follows the durable Merged →
            // CloseReady transition. If the process stops after removal, the
            // next call can reconcile the exact receipt from target ancestry;
            // if it stops before removal, the next call safely retries cleanup.
            if do_cleanup && source_worktree_live {
                manager
                    .cleanup_merged_worktree(&mut worktree)
                    .map_err(|error| {
                        worktree_merge_mcp_error(
                            error,
                            &worktree.branch,
                            &worktree.parent_branch,
                        )
                    })?;
            }

            if let Some(task_id) = task_id {
                let close_result = self
                    .cas_task_close(Parameters(TaskCloseRequest {
                        id: task_id.to_string(),
                        reason: Some(receipt.scope_summary.clone()),
                        bypass_code_review: None,
                        code_review_findings: None,
                        search_manifest: None,
                        commit_receipt: Some(receipt.commit_sha.clone()),
                    }))
                    .await?;
                if self
                    .open_task_store()?
                    .get(task_id)
                    .map(|task| task.status == cas_types::TaskStatus::Closed)
                    .unwrap_or(false)
                {
                    cas_store::transition_worker_delivery(
                        &cas_root,
                        &transaction.id,
                        &[cas_types::WorkerDeliveryState::CloseReady],
                        cas_types::WorkerDeliveryState::Delivered,
                        &authority.agent_id,
                        Some(&authority.agent_id),
                        None,
                        Some(&target_tip),
                        None,
                    )
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to mark worker delivery complete: {error}"
                        )),
                        data: None,
                    })?;
                } else {
                    delivery_close_result = Some(close_result);
                }
            }
        }

        // Update store — System B worktrees were never registered there, so
        // there's no row to update (and nothing worth persisting: the
        // git-level merge + optional cleanup above already happened).
        if !is_system_b {
            worktree_store.update(&worktree).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update worktree: {e}")),
                data: None,
            })?;
        }

        // Always surface the resolved target for System-B merges — the
        // wrong-target-to-trunk defect (cas-0938) was invisible precisely
        // because the tool didn't say which branch it actually used.
        let target_suffix = if is_system_b {
            format!(" [resolved via: {target_reason}]")
        } else {
            String::new()
        };

        let cleanup_note = if do_cleanup {
            " Worktree removed (cleanup=true)."
        } else {
            " Worktree preserved (mid-session merge; pass cleanup=true to remove)."
        };

        // cas-5ee0 (GH #137): resolve push state for the non-transactional
        // path (the transactional one already published, before its close).
        // Every success receipt below carries this line — "Merged" alone is
        // not a truthful receipt when origin never saw the merge.
        let (push_branch, push_outcome) = target_push.unwrap_or_else(|| {
            let branch = worktree.parent_branch.clone();
            let outcome = manager.git().publish_branch_to_origin(&branch);
            (branch, outcome)
        });
        let push_note = describe_target_push_state(&push_branch, &push_outcome);

        if !reconciled_delivery {
            let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
                &self.cas_root,
                "worktree_merged",
                &[
                    ("worktree_id", &worktree.id),
                    ("branch", &worktree.branch),
                    ("target_branch", &worktree.parent_branch),
                    ("commit", merge_commit.as_deref().unwrap_or("none")),
                    ("cleanup", if do_cleanup { "true" } else { "false" }),
                ],
            );
        }

        if let Some(mut close_result) = delivery_close_result {
            // Git and durable delivery state reached CloseReady, but the
            // internal close surfaced another exact gate. Preserve that
            // result verbatim so the caller receives the required next action
            // instead of a misleading generic merge success.
            //
            // cas-5ee0: if the target never reached origin, that is very
            // likely *why* the close gate fired — append the diagnosis rather
            // than letting the caller re-derive it. Appended as an extra
            // content block so the gate's own text stays verbatim.
            if !push_outcome.is_published() {
                close_result.content.push(Content::text(push_note));
            }
            return Ok(close_result);
        }

        // Promote entries if configured
        if wt_config.promote_entries_on_merge {
            if let Ok(count) = self.promote_branch_entries(&worktree.branch) {
                if count > 0 {
                    return Ok(Self::success(format!(
                        "Merged worktree {} to {}.{} Commit: {}{}{}\nPromoted {} entries from branch scope.",
                        worktree.id,
                        worktree.parent_branch,
                        target_suffix,
                        merge_commit.as_deref().unwrap_or("none"),
                        cleanup_note,
                        push_note,
                        count
                    )));
                }
            }
        }

        Ok(Self::success(format!(
            "Merged worktree {} to {}.{} Commit: {}{}{}",
            worktree.id,
            worktree.parent_branch,
            target_suffix,
            merge_commit.as_deref().unwrap_or("none"),
            cleanup_note,
            push_note
        )))
    }

    /// Get current worktree status
    pub async fn worktree_status(&self) -> Result<CallToolResult, McpError> {
        use crate::config::Config;
        use crate::store::open_worktree_store;
        use crate::worktree::GitOperations;

        let cas_root = self.cas_root.clone();
        let config = Config::load(&cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to load config: {e}")),
            data: None,
        })?;
        let wt_config = config.worktrees();

        let cwd = std::env::current_dir().unwrap_or_default();
        let git_context = GitOperations::get_context(&cwd).ok();

        let mut output = String::from("WORKTREE STATUS\n\n");

        // Current git context (caller's working directory)
        if let Some(ctx) = git_context {
            output.push_str(&format!("In worktree: {}\n", ctx.is_worktree));
            if let Some(branch) = ctx.branch {
                output.push_str(&format!("Current branch: {branch}\n"));
            }
            output.push('\n');
        }

        // System A — CAS experimental worktrees (config-gated).
        // Explicitly labeled to avoid confusion with System B (factory isolation).
        output.push_str("System A (CAS experimental worktrees):\n");
        output.push_str(&format!("  Enabled:        {}\n", wt_config.enabled));
        output.push_str(&format!("  Base path:      {}\n", wt_config.base_path));
        output.push_str(&format!("  Branch prefix:  {}\n", wt_config.branch_prefix));
        output.push_str(&format!("  Auto-merge:     {}\n", wt_config.auto_merge));
        output.push_str(&format!(
            "  Cleanup:        {}\n",
            wt_config.cleanup_on_close
        ));

        // Query worktree store for active worktrees
        let mut stored_branches: HashSet<String> = HashSet::new();
        let mut stored_paths: HashSet<PathBuf> = HashSet::new();
        let mut active_count = 0usize;
        let mut branch_names: Vec<String> = Vec::new();

        if let Ok(worktree_store) = open_worktree_store(&cas_root) {
            if let Ok(active_worktrees) = worktree_store.list_active() {
                active_count = active_worktrees.len();
                for wt in &active_worktrees {
                    stored_branches.insert(wt.branch.clone());
                    stored_paths.insert(wt.path.clone());
                    branch_names.push(wt.branch.clone());
                }
            }
        }

        // Live git reconcile — same CAS-pattern rules as worktree_list (cas-d1a0).
        let factory_base = resolve_factory_worktree_base(&cas_root);
        let untracked = collect_untracked_git_worktrees(
            &cas_root,
            &factory_base,
            &stored_branches,
            &stored_paths,
        );
        let mut factory_entries: Vec<(String, PathBuf)> = Vec::new();
        let mut other_untracked: Vec<(String, PathBuf)> = Vec::new();
        for wt in untracked {
            if is_factory_style_worktree(&wt.path, &wt.branch, &cas_root, &factory_base) {
                factory_entries.push((wt.branch, wt.path));
            } else {
                other_untracked.push((wt.branch, wt.path));
            }
        }

        // System B summary — always shown so callers can see isolation state
        // regardless of the System A flag.
        output.push_str("\nSystem B (factory isolation worktrees):\n");
        let b_active = factory_entries.len();
        if b_active == 0 {
            output.push_str("  Active: none\n");
        } else {
            output.push_str(&format!("  Active: {b_active}\n"));
            for (branch, path) in &factory_entries {
                output.push_str(&format!("    {} ({})\n", branch, path.display()));
            }
        }

        // Untracked CAS-pattern worktrees (e.g. epic/* outside factory base)
        // from sibling sessions — visible for management without a store row.
        if !other_untracked.is_empty() {
            output.push_str("\nUntracked CAS-pattern worktrees:\n");
            for (branch, path) in &other_untracked {
                output.push_str(&format!("    {} ({})\n", branch, path.display()));
            }
        }

        // System A active worktrees (if any)
        if active_count > 0 {
            output.push_str(&format!(
                "\nSystem A tracked worktrees: {} ({})\n",
                active_count,
                branch_names.join(", ")
            ));
        }

        Ok(Self::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeliveryMergePreflight, authorize_explicit_task_for_system_b_worker,
        classify_delivery_merge_preflight, derive_delivery_supervisor_authority,
        describe_target_push_state, is_cas_pattern_worktree, is_factory_style_worktree,
        is_git_worktree, path_is_under, resolve_worktree_merge_cleanup, worktree_merge_mcp_error,
    };
    use crate::worktree::git::TargetPushOutcome;
    use crate::worktree::{GitError, WorktreeError};
    use std::path::Path;
    use tempfile::TempDir;

    // -----------------------------------------------------------------
    // cas-5ee0 (GH #137): the merge receipt must state push state.
    // -----------------------------------------------------------------

    #[test]
    fn merge_receipt_shouts_when_the_merge_is_local_only() {
        let note = describe_target_push_state(
            "main",
            &TargetPushOutcome::NotPushed {
                sha: Some("42d81bd9aaaabbbbccccddddeeeeffff00001111".to_string()),
                remote_sha: Some("c2698df216cd9abe7db530ec1d035f6a378d1d06".to_string()),
                reason: "Permission denied (publickey)".to_string(),
            },
        );

        // The three things the supervisor could not learn from the old
        // bare "Merged ..." receipt: that origin is behind, that closes
        // will bounce because of it, and the exact command that fixes it.
        assert!(note.contains("NOT PUSHED"), "{note}");
        assert!(note.contains("origin/main"), "{note}");
        assert!(note.contains("BEHIND"), "{note}");
        assert!(note.contains("git push origin main"), "{note}");
        assert!(
            note.contains("Permission denied (publickey)"),
            "the real git reason must survive: {note}"
        );
        // Short SHAs on both sides so the two tips are visibly different.
        assert!(note.contains("42d81bd9aaaa"), "{note}");
        assert!(note.contains("c2698df216cd"), "{note}");
    }

    #[test]
    fn merge_receipt_states_push_state_without_crying_wolf() {
        let pushed = describe_target_push_state(
            "epic/thing",
            &TargetPushOutcome::Pushed {
                sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
        );
        assert!(
            pushed.contains("pushed epic/thing -> origin/epic/thing"),
            "{pushed}"
        );
        assert!(pushed.contains("0123456789ab"), "{pushed}");
        assert!(!pushed.contains("NOT PUSHED"), "{pushed}");

        // The three benign states must never render the loud warning: a
        // receipt that warns on every local-only repo trains supervisors
        // to ignore the one time it matters.
        for outcome in [
            TargetPushOutcome::NoRemote,
            TargetPushOutcome::RemoteBranchAbsent,
            TargetPushOutcome::AlreadyCurrent {
                sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
        ] {
            let note = describe_target_push_state("main", &outcome);
            assert!(!note.contains("NOT PUSHED"), "{outcome:?} -> {note}");
            assert!(note.contains("Push state:"), "{outcome:?} -> {note}");
            assert!(outcome.is_published());
        }
    }

    #[test]
    fn merge_receipt_push_state_survives_a_short_or_odd_sha() {
        // Defensive: receipt prose must not panic on a sha shorter than the
        // display width (or a non-ASCII boundary) — it is rendered on the
        // failure path, where inputs are least trustworthy.
        let note = describe_target_push_state(
            "main",
            &TargetPushOutcome::NotPushed {
                sha: None,
                remote_sha: None,
                reason: "local ref `main` did not resolve to a commit".to_string(),
            },
        );
        assert!(note.contains("unresolved"), "{note}");
        assert!(note.contains("(no origin ref)"), "{note}");

        let short = describe_target_push_state(
            "main",
            &TargetPushOutcome::Pushed {
                sha: "abc".to_string(),
            },
        );
        assert!(short.contains("abc"), "{short}");
    }

    #[test]
    fn delivery_resume_authority_is_derived_only_from_live_registered_supervisor() {
        let mut supervisor =
            cas_types::Agent::new("sup-session".to_string(), "supervisor".to_string());
        supervisor.role = cas_types::AgentRole::Supervisor;
        assert_eq!(
            derive_delivery_supervisor_authority(&supervisor)
                .unwrap()
                .agent_id,
            "sup-session"
        );

        let mut worker = cas_types::Agent::new("worker-session".to_string(), "worker".to_string());
        worker.role = cas_types::AgentRole::Worker;
        assert!(derive_delivery_supervisor_authority(&worker).is_err());

        supervisor.status = cas_types::AgentStatus::Shutdown;
        assert!(derive_delivery_supervisor_authority(&supervisor).is_err());
    }

    #[test]
    fn delivery_preflight_classifies_happy_retry_and_explicit_failures() {
        assert_eq!(
            classify_delivery_merge_preflight(true, true, false, true, true, true).unwrap(),
            DeliveryMergePreflight::Execute
        );
        // Interrupted resume reconciles exact ancestry before checking drift.
        assert_eq!(
            classify_delivery_merge_preflight(true, true, true, false, false, false).unwrap(),
            DeliveryMergePreflight::Reconcile
        );
        assert_eq!(
            classify_delivery_merge_preflight(false, true, false, true, true, true)
                .unwrap_err()
                .0,
            cas_types::WorkerDeliveryState::RepoMismatch
        );
        assert_eq!(
            classify_delivery_merge_preflight(true, false, false, true, true, true)
                .unwrap_err()
                .0,
            cas_types::WorkerDeliveryState::Stale
        );
        assert_eq!(
            classify_delivery_merge_preflight(true, true, false, false, true, true)
                .unwrap_err()
                .0,
            cas_types::WorkerDeliveryState::TipChanged
        );
        // cas-0a21: target drift is typed as a recoverable tip change.
        let target_drift = classify_delivery_merge_preflight(true, true, false, true, false, true)
            .unwrap_err();
        assert_eq!(target_drift.0, cas_types::WorkerDeliveryState::TipChanged);
        assert!(target_drift.0.is_recoverable_failure());
        assert_eq!(target_drift.1, "target_tip_changed");
        assert_eq!(
            classify_delivery_merge_preflight(true, true, false, true, true, false)
                .unwrap_err()
                .1,
            "merge_base_changed"
        );
    }

    #[test]
    fn is_git_worktree_true_when_git_entry_present() {
        let temp = TempDir::new().unwrap();
        let wt_path = temp.path().join("alice");
        std::fs::create_dir_all(wt_path.join(".git")).unwrap();

        assert!(is_git_worktree(&wt_path));
    }

    #[test]
    fn is_git_worktree_false_when_path_missing() {
        let temp = TempDir::new().unwrap();
        assert!(!is_git_worktree(&temp.path().join("ghost")));
    }

    #[test]
    fn is_git_worktree_false_when_directory_exists_but_not_a_git_worktree() {
        // A stray non-git directory (e.g. leftover cruft) must not be
        // mistaken for a live factory worktree.
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("bob");
        std::fs::create_dir_all(&path).unwrap();

        assert!(!is_git_worktree(&path));
    }

    #[test]
    fn cas_pattern_matches_factory_branch_outside_cas_dir() {
        let repo = Path::new("/repo");
        let cas = Path::new("/repo/.cas");
        let factory_base = Path::new("/repo/.cas/worktrees");
        let path = Path::new("/tmp/elsewhere/worker");
        assert!(is_cas_pattern_worktree(
            path,
            Some("factory/hv-food-qa"),
            cas,
            factory_base,
            repo,
        ));
    }

    #[test]
    fn cas_pattern_matches_epic_branch_outside_cas_dir() {
        let repo = Path::new("/repo");
        let cas = Path::new("/repo/.cas");
        let factory_base = Path::new("/repo/.cas/worktrees");
        let path = Path::new("/tmp/ozer-epic-ea3e-hv");
        assert!(is_cas_pattern_worktree(
            path,
            Some("epic/integrate-cas-ea3e"),
            cas,
            factory_base,
            repo,
        ));
        assert!(!is_factory_style_worktree(
            path,
            "epic/integrate-cas-ea3e",
            cas,
            factory_base,
        ));
    }

    #[test]
    fn cas_pattern_rejects_main_checkout_and_unrelated_branches() {
        let repo = Path::new("/repo");
        let cas = Path::new("/repo/.cas");
        let factory_base = Path::new("/repo/.cas/worktrees");
        assert!(!is_cas_pattern_worktree(
            repo,
            Some("staging"),
            cas,
            factory_base,
            repo,
        ));
        assert!(!is_cas_pattern_worktree(
            Path::new("/tmp/hand-made"),
            Some("feature/hand-made"),
            cas,
            factory_base,
            repo,
        ));
    }

    #[test]
    fn path_is_under_matches_prefix() {
        let base = Path::new("/proj/.cas/worktrees");
        assert!(path_is_under(Path::new("/proj/.cas/worktrees/alice"), base));
        assert!(!path_is_under(Path::new("/proj/other"), base));
    }

    // --- cas-369f: force ≠ cleanup; System-B default preserve -------------

    #[test]
    fn resolve_merge_cleanup_system_b_defaults_to_preserve() {
        assert!(
            !resolve_worktree_merge_cleanup(None, true, true),
            "System-B mid-session default must preserve even if config cleanup_on_close=true"
        );
        assert!(!resolve_worktree_merge_cleanup(None, true, false));
    }

    #[test]
    fn resolve_merge_cleanup_explicit_true_wins() {
        assert!(resolve_worktree_merge_cleanup(Some(true), true, false));
        assert!(resolve_worktree_merge_cleanup(Some(true), false, false));
    }

    #[test]
    fn resolve_merge_cleanup_explicit_false_wins() {
        assert!(!resolve_worktree_merge_cleanup(Some(false), true, true));
        assert!(!resolve_worktree_merge_cleanup(Some(false), false, true));
    }

    #[test]
    fn resolve_merge_cleanup_system_a_uses_config() {
        assert!(resolve_worktree_merge_cleanup(None, false, true));
        assert!(!resolve_worktree_merge_cleanup(None, false, false));
    }

    #[test]
    fn conflict_error_names_paths_restored_checkout_and_manual_options() {
        let error = worktree_merge_mcp_error(
            WorktreeError::Git(GitError::MergeConflictPaths(vec![
                "src/one.rs".to_string(),
                "src/two.rs".to_string(),
            ])),
            "factory/worker",
            "epic/example",
        );
        let message = error.message.as_ref();

        assert!(message.contains("CONTENT CONFLICT"));
        assert!(message.contains("src/one.rs"));
        assert!(message.contains("src/two.rs"));
        assert!(message.contains("restored"));
        assert!(message.contains("temporary worktree"));
        assert!(message.contains("factory/worker"));
        assert!(message.contains("epic/example"));
    }

    #[test]
    fn pre_existing_checkout_residue_error_names_original_state() {
        let error = worktree_merge_mcp_error(
            WorktreeError::Git(GitError::MergeInProgress("UU src/leftover.rs".to_string())),
            "factory/unrelated",
            "epic/example",
        );
        let message = error.message.as_ref();

        assert!(message.contains("PRE-EXISTING MERGE RESIDUE"));
        assert!(message.contains("MERGE_HEAD"));
        assert!(message.contains("src/leftover.rs"));
        assert!(message.contains("did not attempt"));
        assert!(message.contains("factory/unrelated"));

        let dirty_error = worktree_merge_mcp_error(
            WorktreeError::Git(GitError::MergeCheckoutDirty(
                "added src/staged.rs".to_string(),
            )),
            "factory/unrelated",
            "epic/example",
        );
        let dirty_message = dirty_error.message.as_ref();
        assert!(dirty_message.contains("PRE-EXISTING TARGET CHECKOUT RESIDUE"));
        assert!(dirty_message.contains("src/staged.rs"));
        assert!(dirty_message.contains("force=true"));
        // cas-4702 / GH #73: the refusal must say it is scoped to the
        // intersecting paths and name the sanctioned fallback.
        assert!(dirty_message.contains("paths this merge would write"));
        assert!(dirty_message.contains("does NOT touch is ignored"));
        assert!(dirty_message.contains("git worktree add --detach"));
    }

    // -----------------------------------------------------------------------
    // cas-f8bc (GH #106): the deadlock's exit depends on rule 2 — an assignee
    // match authorizes the merge with NO lease. The behindness fix is only a
    // real exit if that stays true, so pin it.
    // -----------------------------------------------------------------------

    fn agent_store_with_worker(cas_dir: &Path, worker: &str) -> std::sync::Arc<dyn cas_store::AgentStore> {
        let store = crate::store::open_agent_store(cas_dir).expect("open agent store");
        let mut agent = cas_types::Agent::new(format!("{worker}-session"), worker.to_string());
        agent.role = cas_types::AgentRole::Worker;
        store.register(&agent).expect("register worker");
        store
    }

    fn task_assigned_to(assignee: Option<&str>) -> cas_types::Task {
        let mut task = cas_types::Task::new("cas-b001".to_string(), "standalone fix".to_string());
        task.assignee = assignee.map(str::to_string);
        task
    }

    #[test]
    fn assignee_match_authorizes_system_b_merge_without_any_lease_cas_f8bc() {
        let temp = TempDir::new().unwrap();
        let store = agent_store_with_worker(temp.path(), "wolf");

        // No lease was ever taken — this is the post-assignment state the
        // behindness fix unblocks.
        assert!(
            store.get_lease("cas-b001").expect("lease read").is_none(),
            "precondition: no lease exists for the task"
        );

        authorize_explicit_task_for_system_b_worker(
            &task_assigned_to(Some("wolf")),
            "wolf",
            store.as_ref(),
        )
        .expect("assignee match must authorize the merge with no lease (GH #106 exit)");
    }

    #[test]
    fn unassigned_leaseless_task_is_still_refused_cas_f8bc() {
        let temp = TempDir::new().unwrap();
        let store = agent_store_with_worker(temp.path(), "wolf");

        let error = authorize_explicit_task_for_system_b_worker(
            &task_assigned_to(None),
            "wolf",
            store.as_ref(),
        )
        .expect_err("the conservative rule must survive: this is what asks for the assignment");
        assert!(
            error.message.contains("no assignee and no active lease"),
            "refusal must state why: {}",
            error.message
        );
    }

    #[test]
    fn foreign_assignee_is_still_refused_cas_f8bc() {
        let temp = TempDir::new().unwrap();
        let store = agent_store_with_worker(temp.path(), "wolf");

        let error = authorize_explicit_task_for_system_b_worker(
            &task_assigned_to(Some("other-worker")),
            "wolf",
            store.as_ref(),
        )
        .expect_err("cas-bd5f must keep refusing a foreign task's epic");
        assert!(
            error.message.contains("is assigned to"),
            "refusal must name the mismatch: {}",
            error.message
        );
    }
}
