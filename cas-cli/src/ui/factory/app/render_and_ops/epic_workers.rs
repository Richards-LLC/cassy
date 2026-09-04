use crate::store::{open_agent_store, open_task_store};
use crate::ui::factory::app::imports::*;
use crate::worktree::RemoveOutcome;

fn validate_live_spawn_repo_context(
    manager: &WorktreeManager,
    project_path: &std::path::Path,
) -> anyhow::Result<()> {
    use crate::worktree::GitOperations;

    let live_root = GitOperations::detect_repo_root(project_path).map_err(|error| {
        anyhow::anyhow!(
            "Repository is not available at spawn time: {error}. If git was initialized after \
             the factory started, restart the factory daemon before spawning isolated workers."
        )
    })?;
    if live_root != manager.repo_root() {
        anyhow::bail!(
            "Repository context changed after the factory daemon started (cached root: {}, \
             live root: {}). Restart the factory daemon so worker isolation uses the new repository.",
            manager.repo_root().display(),
            live_root.display(),
        );
    }
    Ok(())
}

fn bool_prop(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn worker_registry_rows_for_shutdown<'a>(
    agents: &'a [cas_types::Agent],
    name: &str,
    factory_session: Option<&str>,
) -> Vec<&'a cas_types::Agent> {
    let mut matching: Vec<_> = agents
        .iter()
        .filter(|agent| {
            agent.name == name
                && agent.role == cas_types::AgentRole::Worker
                && factory_session
                    .is_none_or(|session| agent.factory_session.as_deref() == Some(session))
        })
        .collect();
    matching.sort_by_key(|agent| agent.registered_at);
    matching
}

/// Resolve the current worker harness from the live on-disk `LlmConfig`.
///
/// Exists as a standalone function so unit tests can verify the on-disk
/// config-read path directly, without needing a full `FactoryApp`.
///
/// Falls back to `SupervisorCli::Claude` only when the config file is
/// *unparseable* (`Config::load` returns `Err`) or the persisted harness
/// string doesn't parse as a known `SupervisorCli` — degraded but not
/// broken. An *absent* config file is NOT degraded: `Config::load` returns
/// `Ok(Config::default())` for a missing file, so it flows through
/// `harness_for_role("worker")` like any other empty config and resolves to
/// the worker-only stock floor, `STOCK_WORKER_HARNESS` = `"codex"` (cas-fbac).
///
/// In production code the equivalent logic lives inside
/// `FactoryApp::sync_worker_config_from_live_settings`, which also
/// re-reads model and effort in the same config load.
#[cfg(test)]
pub(super) fn resolve_live_worker_harness(cas_dir: &std::path::Path) -> cas_mux::SupervisorCli {
    use std::str::FromStr;
    Config::load(cas_dir)
        .ok()
        .map(|c| c.llm())
        .as_ref()
        .and_then(|llm| cas_mux::SupervisorCli::from_str(llm.harness_for_role("worker")).ok())
        .unwrap_or(cas_mux::SupervisorCli::Claude)
}

/// Metadata key written on the agent record when shutdown preserved a dirty
/// worktree. The daemon reaper (Unit 3) reads this to drive TTL-based salvage.
const DIRTY_ON_SHUTDOWN_KEY: &str = "dirty_on_shutdown";

/// Resolve the branch a dynamically-spawned worker should be cut from: the
/// active epic branch when pinned, else the configured epic base
/// (`.cas/config.toml` `[factory] epic_base_branch`, cas-b082), else the
/// repo's detected default branch.
fn worker_base_for_spawn(epic_branch: Option<&str>, manager: &WorktreeManager) -> String {
    epic_branch.map(ToOwned::to_owned).unwrap_or_else(|| {
        Config::configured_epic_base_branch(manager.repo_root())
            .unwrap_or_else(|| manager.git().detect_default_branch())
    })
}

/// cas-7587 (GH #122): the epic branch a pre-assigned task belongs to,
/// resolved from the task store at spawn time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskEpicBase {
    /// The task the spawn was requested for (`spawn_workers task_id=...`).
    pub task_id: String,
    /// The epic that owns that task (or the task itself when it *is* an epic).
    pub epic_id: String,
    /// Branch recorded on that epic (or, for a legacy epic, derived from its
    /// title only as a last-resort fallback).
    pub branch: String,
    /// Whether `branch` currently resolves to a commit in the repository.
    pub branch_exists: bool,
    /// `true` only when `branch` was synthesized from the title because the
    /// legacy `epic.branch` field is absent. A declared epic WorkTarget must
    /// outrank this cosmetic fallback; it must not be mistaken for a live
    /// coordination branch.
    pub branch_is_title_slug_fallback: bool,
    /// The WorkTarget explicitly declared by the task or its owning epic.
    /// It is delivery authority for both worker spawn and worktree merge.
    pub work_target: Option<SpawnWorkTarget>,
}

/// The concrete delivery branch a `spawn_workers task_id=...` request must
/// check out. This carries the owner so the receipt can say whether the task
/// itself or its parent epic provided the authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnWorkTarget {
    pub task_id: String,
    /// Portable selector for the repository whose target branch owns this
    /// spawn. A task's store remains in the factory session repository, but
    /// the worker's Git worktree may live in this sibling checkout.
    pub repo_selector: String,
    pub target_branch: String,
    pub owner: WorkTargetOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkTargetOwner {
    Task,
    Epic { epic_id: String },
}

/// cas-d897 (GH #146): what the task store could tell us about the base for a
/// spawn that named a task.
///
/// The distinction that matters is between "this task provably has no epic"
/// (→ trunk, never the operator's pinned focus) and "we could not find out"
/// (→ keep the legacy focus fallback). Collapsing both into `None` is what
/// let an epic-less task get cut from a stale focus branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskBase {
    /// The task resolved to an epic (possibly one whose branch is missing).
    Epic(TaskEpicBase),
    /// The task exists and provably belongs to no epic.
    NoEpic {
        task_id: String,
        work_target: Option<SpawnWorkTarget>,
    },
    /// No task named, or the store could not answer (missing task, store
    /// error). Legacy focus-based behaviour applies.
    Unresolved,
}

impl TaskBase {
    /// The resolved epic, when there is one.
    pub(crate) fn epic(&self) -> Option<&TaskEpicBase> {
        match self {
            TaskBase::Epic(epic) => Some(epic),
            _ => None,
        }
    }

    /// The declared WorkTarget for this task, or (when the task has none) its
    /// owning epic. This is the same explicit authority the System-B merge
    /// path honors for a directly targeted task.
    pub(crate) fn work_target(&self) -> Option<&SpawnWorkTarget> {
        match self {
            TaskBase::Epic(epic) => epic.work_target.as_ref(),
            TaskBase::NoEpic { work_target, .. } => work_target.as_ref(),
            TaskBase::Unresolved => None,
        }
    }

    /// The declared integration branch, when a WorkTarget exists.
    pub(crate) fn target_branch(&self) -> Option<&str> {
        self.work_target()
            .map(|target| target.target_branch.as_str())
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
    }
}

/// cas-7587 (GH #122): where a worker's spawn base came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpawnBaseSource {
    /// The task (or its owning epic) declared the integration branch to use.
    WorkTarget {
        task_id: String,
        owner: WorkTargetOwner,
    },
    /// Base is the branch of the epic that owns the pre-assigned task.
    /// This outranks the pinned focus — the task, not the operator's last
    /// `focus_epic`, decides which history the worker needs.
    TaskEpic { task_id: String, epic_id: String },
    /// cas-d897 (GH #146): the spawn named a task that belongs to no epic, so
    /// the base is trunk. The pinned focus is deliberately *not* used: an
    /// epic-less task has no business inheriting an unrelated epic's history.
    TaskWithoutEpic { task_id: String },
    /// No task context (or the task's epic has no branch on disk): the
    /// session's pinned epic focus.
    PinnedFocus,
    /// Neither task epic nor focus: configured trunk / detected default.
    Trunk,
}

/// cas-7587 (GH #122): resolve the branch a spawning worker must be cut from.
///
/// ROOT CAUSE this fixes: `prepare_worker_spawn` resolved the base purely from
/// `self.epic_branch` (the session's *pinned focus*), while `spawn_workers`
/// carried a `task_id` that could belong to a completely different epic. With
/// focus pinned to epic A, `spawn_workers task_id=<task of epic B>` cut the
/// worktree from epic A's branch even though epic B's branch existed — the
/// worker then built on history its task had nothing to do with (observed
/// twice on 2026-08-06, in both directions).
///
/// cas-d897 (GH #146) extends it: a spawn that names a task with *no* epic
/// must fall through to trunk, not to the pinned focus. Observed 2026-08-07 —
/// an epic-less task's worktree was cut from the pinned epic's branch, 71
/// commits behind trunk.
///
/// Precedence, highest first:
///   1. the task's declared WorkTarget;
///   2. the task epic's recorded live branch;
///   3. the task epic's declared WorkTarget;
///   4. a legacy title-derived epic branch, only when no declared target
///      exists and that branch resolves in the repo;
///   5. trunk, when the spawn names a task that provably belongs to no epic
///      (cas-d897 / GH #146);
///   6. the pinned epic focus (taskless spawns, tasks the store could not
///      resolve, and tasks whose epic has no branch yet — falling back to
///      focus there preserves pre-fix behavior rather than silently dropping
///      a worker onto trunk);
///   7. configured trunk / detected default branch.
///
/// Pure so the precedence itself is unit-testable without a factory app.
pub(crate) fn resolve_spawn_base(
    task_base: &TaskBase,
    focused_epic_branch: Option<&str>,
    trunk: &str,
) -> (String, SpawnBaseSource) {
    if let Some(target) = task_base
        .work_target()
        .filter(|target| matches!(target.owner, WorkTargetOwner::Task))
    {
        return (
            target.target_branch.clone(),
            SpawnBaseSource::WorkTarget {
                task_id: target.task_id.clone(),
                owner: target.owner.clone(),
            },
        );
    }
    // A live epic branch carries its children's sibling integration history.
    // An epic-owned WorkTarget is its final delivery destination, not a reason
    // to discard that branch while it exists.
    if let Some(task_epic) = task_base
        .epic()
        .filter(|t| t.branch_exists && !t.branch_is_title_slug_fallback)
    {
        return (
            task_epic.branch.clone(),
            SpawnBaseSource::TaskEpic {
                task_id: task_epic.task_id.clone(),
                epic_id: task_epic.epic_id.clone(),
            },
        );
    }
    if let Some(target) = task_base.work_target() {
        return (
            target.target_branch.clone(),
            SpawnBaseSource::WorkTarget {
                task_id: target.task_id.clone(),
                owner: target.owner.clone(),
            },
        );
    }
    // Preserve legacy behaviour only after all declared delivery authority
    // has been exhausted. A title slug is merely a guessed old branch name;
    // it must never override a WorkTarget that MCP can actually maintain.
    if let Some(task_epic) = task_base
        .epic()
        .filter(|t| t.branch_exists && t.branch_is_title_slug_fallback)
    {
        return (
            task_epic.branch.clone(),
            SpawnBaseSource::TaskEpic {
                task_id: task_epic.task_id.clone(),
                epic_id: task_epic.epic_id.clone(),
            },
        );
    }
    if let TaskBase::NoEpic { task_id, .. } = task_base {
        // cas-d897 (GH #146): no epic means no epic history to inherit. The
        // operator's pinned focus is about *their* current attention, not
        // this task, and it is routinely a stale branch.
        return (
            trunk.to_string(),
            SpawnBaseSource::TaskWithoutEpic {
                task_id: task_id.clone(),
            },
        );
    }
    match focused_epic_branch {
        Some(branch) => (branch.to_string(), SpawnBaseSource::PinnedFocus),
        None => (trunk.to_string(), SpawnBaseSource::Trunk),
    }
}

/// cas-7587 (GH #122): one line naming the resolved base *and why it won*, so
/// a supervisor reading spawn output never has to guess which epic's history a
/// worker actually got. When the task's epic differs from the pinned focus the
/// divergence is spelled out explicitly — that silent divergence was the bug.
pub(crate) fn spawn_base_provenance_notice(
    base: &str,
    source: &SpawnBaseSource,
    focused_epic_branch: Option<&str>,
) -> String {
    match source {
        SpawnBaseSource::WorkTarget { task_id, owner } => {
            let authority = match owner {
                WorkTargetOwner::Task => format!("task {task_id}'s WorkTarget"),
                WorkTargetOwner::Epic { epic_id } => {
                    format!("parent epic {epic_id}'s WorkTarget for task {task_id}")
                }
            };
            let mut line = format!(
                "SPAWN BASE: '{base}' — {authority}; declared delivery branch wins over ambient trunk and pinned focus."
            );
            if let Some(focus) = focused_epic_branch.filter(|focus| *focus != base) {
                line.push_str(&format!(
                    " NOTE: pinned focus branch '{focus}' was not used because the declared WorkTarget is authoritative."
                ));
            }
            line
        }
        SpawnBaseSource::TaskEpic { task_id, epic_id } => {
            let mut line = format!(
                "SPAWN BASE: '{base}' — branch of epic {epic_id}, which owns pre-assigned task {task_id}."
            );
            match focused_epic_branch {
                Some(focus) if focus != base => {
                    line.push_str(&format!(
                        " NOTE: this differs from the pinned focus branch '{focus}'; the task's \
                         epic wins so the worker starts on the history its task belongs to."
                    ));
                }
                Some(_) => line.push_str(" (Same branch as the pinned epic focus.)"),
                None => line.push_str(" (No epic focus pinned.)"),
            }
            line
        }
        SpawnBaseSource::TaskWithoutEpic { task_id } => {
            let mut line = format!(
                "SPAWN BASE: '{base}' — integration trunk fallback: pre-assigned task {task_id} \
                 has no WorkTarget and belongs to no epic, so there is no declared delivery or \
                 epic history to inherit."
            );
            match focused_epic_branch {
                Some(focus) if focus != base => {
                    line.push_str(&format!(
                        " NOTE: the pinned focus branch '{focus}' was deliberately NOT used \
                         (cas-d897 / GH #146) — an epic-less task must not inherit an unrelated \
                         epic's history."
                    ));
                }
                _ => {}
            }
            line
        }
        SpawnBaseSource::PinnedFocus => {
            format!("SPAWN BASE: '{base}' — pinned epic focus (no task epic branch to prefer).")
        }
        SpawnBaseSource::Trunk => {
            format!("SPAWN BASE: '{base}' — integration trunk (no task epic, no pinned focus).")
        }
    }
}

/// Surface the one hazardous legacy shape: an epic with no persisted branch
/// has both a title-derived branch and a different declared WorkTarget. The
/// declared target wins, but the old branch is visible evidence that an
/// operator may otherwise mistake it for the integration lane.
fn stale_legacy_slug_notice(
    task_epic: Option<&TaskEpicBase>,
    base: &str,
    source: &SpawnBaseSource,
) -> Option<String> {
    let SpawnBaseSource::WorkTarget {
        owner: WorkTargetOwner::Epic { .. },
        ..
    } = source
    else {
        return None;
    };
    let legacy_slug = task_epic.filter(|epic| {
        epic.branch_is_title_slug_fallback && epic.branch_exists && epic.branch != base
    })?;
    Some(format!(
        "SPAWN BASE: declared epic WorkTarget '{base}' won over legacy title-derived branch '{}' for epic {}. The legacy slug is stale and was not used.",
        legacy_slug.branch, legacy_slug.epic_id
    ))
}

/// cas-7587 (GH #122): `true` when the task's epic decided the base *and* that
/// base is not the pinned focus branch. That divergence is exactly what used to
/// happen silently (and wrongly, in the other direction), so it is escalated to
/// the supervisor rather than only written to the spawn audit trail.
///
/// cas-d897 (GH #146): the same applies when an epic-less task sends the spawn
/// to trunk while a focus is pinned — the operator expected their focus branch
/// and must be told it was intentionally overridden.
pub(crate) fn base_diverges_from_focus(
    base: &str,
    source: &SpawnBaseSource,
    focused_epic_branch: Option<&str>,
) -> bool {
    matches!(
        source,
        SpawnBaseSource::WorkTarget { .. }
            | SpawnBaseSource::TaskEpic { .. }
            | SpawnBaseSource::TaskWithoutEpic { .. }
    ) && focused_epic_branch.is_some_and(|focus| focus != base)
}

/// Resolve the repository in which an isolated worker worktree must be
/// provisioned.
///
/// The task store is deliberately not moved: `cas_dir` remains the spawning
/// session's `.cas` directory and is passed to the worker as `CAS_ROOT`. A
/// declared WorkTarget can, however, identify a sibling checkout. Resolve
/// that selector through the host repository bindings before any branch or
/// worktree operation so a branch that exists only there is never looked up in
/// the session repository.
pub(crate) fn resolve_spawn_worktree_repo(
    cas_dir: &std::path::Path,
    session_repo_root: &std::path::Path,
    target: Option<&SpawnWorkTarget>,
) -> anyhow::Result<std::path::PathBuf> {
    let Some(target) = target else {
        return Ok(session_repo_root.to_path_buf());
    };

    if crate::mcp::tools::core::task::repo_context::repo_answers_to(
        session_repo_root,
        &target.repo_selector,
    ) {
        return Ok(session_repo_root.to_path_buf());
    }

    let work_target = cas_types::WorkTarget {
        repo_selector: target.repo_selector.clone(),
        target_branch: target.target_branch.clone(),
    };
    let context = crate::mcp::tools::core::task::repo_context::resolve_repo_context(
        cas_dir,
        &work_target,
    )
    .map_err(|reason| {
        anyhow::anyhow!(
            "cross-repo spawn: target_repo {} — {}",
            target.repo_selector,
            reason
        )
    })?;
    let target_root = context.repo_root;
    let target_git = crate::worktree::GitOperations::new(target_root.clone());
    if !target_git.has_commits().unwrap_or(false) {
        anyhow::bail!(
            "cross-repo spawn: target_repo {} — repository has no commits",
            target_root.display()
        );
    }
    if !ref_exists(&target_root, &target.target_branch) {
        anyhow::bail!(
            "cross-repo spawn: target_repo {} — target branch '{}' does not resolve to a commit",
            target_root.display(),
            target.target_branch
        );
    }
    Ok(target_root)
}

/// Human-readable provision receipt. Keep the session store implicit in this
/// line: `Worktree repository` is the Git venue, while the worker's `CAS_ROOT`
/// continues to be the spawning session's store.
pub(crate) fn spawn_provision_receipt(prep: &crate::ui::factory::app::WorkerSpawnPrep) -> String {
    match prep.worktree_info.as_ref() {
        Some(worktree) => format!(
            "Preparing worker filesystem and worktree. Worktree repository: {} (CAS_ROOT remains the factory session store).",
            worktree.repo_root.display()
        ),
        None => "Preparing worker filesystem (no isolated worktree).".to_string(),
    }
}

/// cas-7587: resolve `task_id` → its epic → that epic's branch.
///
/// A task that *is* an epic resolves to itself. The branch is the one persisted
/// on the epic task, falling back to the title-derived name only for legacy
/// epics. `branch_exists` records
/// whether that branch is actually present in `repo_root` — the caller uses it
/// to decide whether the task epic may outrank the pinned focus.
///
/// Returns `TaskBase::Unresolved` when the task/epic cannot be resolved at all;
/// the caller then keeps the pre-cas-7587 focus-based behavior. cas-d897
/// (GH #146): a task that provably has *no* epic returns `TaskBase::NoEpic`
/// instead, which routes the spawn to trunk rather than the pinned focus.
pub(crate) fn task_epic_base(
    cas_dir: &std::path::Path,
    repo_root: &std::path::Path,
    task_id: &str,
) -> TaskBase {
    let store = match open_task_store(cas_dir) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                task_id, %error,
                "cas-7587: could not open task store to resolve spawn base from the task's epic"
            );
            return TaskBase::Unresolved;
        }
    };

    let task = match store.get(task_id) {
        Ok(task) => task,
        Err(error) => {
            tracing::warn!(
                task_id, %error,
                "cas-7587: pre-assigned task not found while resolving spawn base"
            );
            return TaskBase::Unresolved;
        }
    };

    let task_work_target = task
        .deliverables
        .work_target
        .as_ref()
        .map(|target| SpawnWorkTarget {
            task_id: task_id.to_string(),
            repo_selector: target.repo_selector.clone(),
            target_branch: target.target_branch.clone(),
            owner: if task.task_type == cas_types::TaskType::Epic {
                WorkTargetOwner::Epic {
                    epic_id: task.id.clone(),
                }
            } else {
                WorkTargetOwner::Task
            },
        });
    let task_is_epic = task.task_type == cas_types::TaskType::Epic;
    let epic = if task_is_epic {
        task.clone()
    } else {
        match store.get_parent_epic(task_id) {
            Ok(Some(epic)) => epic,
            Ok(None) => {
                tracing::debug!(
                    task_id,
                    "cas-d897: task has no parent epic; spawn base falls through to trunk"
                );
                return TaskBase::NoEpic {
                    task_id: task_id.to_string(),
                    work_target: task_work_target,
                };
            }
            Err(error) => {
                tracing::warn!(
                    task_id, %error,
                    "cas-7587: parent-epic lookup failed while resolving spawn base"
                );
                return TaskBase::Unresolved;
            }
        }
    };

    let recorded_branch = epic.branch.clone().filter(|b| !b.trim().is_empty());
    let branch_is_title_slug_fallback = recorded_branch.is_none();
    let branch =
        recorded_branch.unwrap_or_else(|| crate::ui::factory::app::epic_branch_name(&epic.title));
    let branch_exists = ref_exists(repo_root, &branch);
    if !branch_exists {
        tracing::warn!(
            task_id,
            epic_id = %epic.id,
            branch = %branch,
            "cas-7587: task's epic branch does not exist in this repo; keeping focus-based spawn base"
        );
    }

    // cas-d22d (GH #625): a child created before the task-create inheritance
    // fix can still carry the parent's default target (usually `main`). That
    // is implicit epic scope, not an explicit task lane; let the live epic
    // branch win while preserving distinct task targets below.
    let child_target_is_epic_default = !task_is_epic
        && crate::mcp::tools::core::task::repo_context::default_child_work_target_from_epic(
            &task, &epic,
        )
        .is_some();

    TaskBase::Epic(TaskEpicBase {
        task_id: task_id.to_string(),
        epic_id: epic.id.clone(),
        branch,
        branch_exists,
        branch_is_title_slug_fallback,
        work_target: (!child_target_is_epic_default)
            .then_some(task_work_target)
            .flatten()
            .or_else(|| {
                epic.deliverables
                    .work_target
                    .as_ref()
                    .map(|target| SpawnWorkTarget {
                        task_id: task_id.to_string(),
                        repo_selector: target.repo_selector.clone(),
                        target_branch: target.target_branch.clone(),
                        owner: WorkTargetOwner::Epic {
                            epic_id: epic.id.clone(),
                        },
                    })
            }),
    })
}

/// Return a supervisor-visible warning when a resolved spawn base cannot be
/// proven to contain the focused epic's current tip.
fn worker_base_mismatch_notice(
    repo_root: &std::path::Path,
    worker_base: &str,
    epic_branch: &str,
) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", epic_branch, worker_base])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(output) if output.status.success() => None,
        Ok(_) => {
            let git = crate::worktree::GitOperations::new(repo_root.to_path_buf());
            let epic_tip = git
                .ref_sha(epic_branch)
                .unwrap_or_else(|_| "unknown".to_string());
            let base_tip = git
                .ref_sha(worker_base)
                .unwrap_or_else(|_| "unknown".to_string());
            Some(format!(
                "WORKER BASE MISMATCH: resolved base '{worker_base}' ({}) does not contain \
                 focused epic branch '{epic_branch}' tip ({}). Worker spawn may be missing \
                 epic changes.",
                &base_tip[..base_tip.len().min(8)],
                &epic_tip[..epic_tip.len().min(8)],
            ))
        }
        Err(error) => Some(format!(
            "WORKER BASE MISMATCH: could not verify resolved base '{worker_base}' contains \
             focused epic branch '{epic_branch}': {error}. Worker spawn may be missing epic changes."
        )),
    }
}

/// `true` when `candidate` has a tree change since it diverged from `base`.
///
/// Commit counts are not freshness: a correct staging-based epic is expected
/// to be permanently behind `main` after promotions, and empty/replayed
/// commits should not make a worker base look stale. Compare the declared
/// target's tree against the merge base instead. A clean diff means all target
/// content is already in the base history (including the strict-superset
/// shape), so no warning is warranted.
fn target_has_tree_delta(repo_root: &std::path::Path, base: &str, candidate: &str) -> Option<bool> {
    let merge_base = std::process::Command::new("git")
        .args(["merge-base", base, candidate])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())?;
    let output = std::process::Command::new("git")
        .args(["diff", "--quiet", &merge_base, candidate])
        .current_dir(repo_root)
        .output()
        .ok()?;
    match output.status.code() {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

/// Count commits reachable from `newer` but not from `base`.
///
/// This remains appropriate for choosing between a local branch and its own
/// remote tracking ref in [`prefer_fresher_base_ref`]. It is deliberately not
/// used for declared-target staleness, which is content-based above.
fn commits_behind(repo_root: &std::path::Path, base: &str, newer: &str) -> Option<usize> {
    let output = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{base}..{newer}")])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// `true` when `reference` resolves to a commit in `repo_root`.
fn ref_exists(repo_root: &std::path::Path, reference: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ])
        .current_dir(repo_root)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Short SHA for a ref, or `"unknown"` when it cannot be resolved.
fn short_sha(repo_root: &std::path::Path, reference: &str) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", reference])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Full commit id for a ref, or `None` when it cannot be resolved.
fn full_sha(repo_root: &std::path::Path, reference: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// cas-ecf7 (GH #118): warn — loudly — when the branch a fresh worker worktree
/// is about to be cut from is itself behind the history it is supposed to build
/// on.
///
/// ROOT CAUSE this catches: `git worktree add <path> <worker_base>` always
/// resolves `worker_base` live, so the base is never "cached" — but the base
/// *branch* can be stale. In the reported incident three worktrees were created
/// at a commit 25 behind `origin/main`: the factory had a focused epic pinned,
/// `worker_base_for_spawn` correctly returned that epic branch, and the epic
/// branch had been cut from trunk before a release merge and never refreshed.
/// Nothing on the spawn path compared the base against trunk (the existing
/// `worker_base_mismatch_notice` only asks whether the base contains the epic
/// tip — trivially true when the base *is* the epic branch), so every worker
/// silently started in the past.
///
/// Compares `worker_base` only to its declared integration target (and that
/// target's remote tracking ref), by tree content rather than commit counts.
/// Refs that don't exist locally are skipped; only a target-side tree delta
/// produces a notice.
fn stale_spawn_base_notice(
    repo_root: &std::path::Path,
    worker_base: &str,
    target_branch: &str,
) -> Option<String> {
    let mut candidates = vec![target_branch.to_string()];
    let remote_target = format!("origin/{target_branch}");
    if remote_target != worker_base && remote_target != target_branch {
        candidates.push(remote_target);
    }

    let stale: Vec<String> = candidates
        .into_iter()
        .filter(|reference| ref_exists(repo_root, reference))
        .filter(|reference| target_has_tree_delta(repo_root, worker_base, reference) == Some(true))
        .collect();
    if stale.is_empty() {
        return None;
    }

    let detail = stale
        .iter()
        .map(|reference| {
            format!(
                "target tree differs from '{reference}' ({})",
                short_sha(repo_root, reference)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    Some(format!(
        "⚠️ STALE WORKER BASE: new worker worktrees are being cut from '{worker_base}' ({}), \
         and the declared integration target has changes absent from that base ({detail}). Every \
         worker spawned now starts without those target changes. Refresh the base first (merge or \
         rebase '{target_branch}' into '{worker_base}'). Do not force-sync workers created by this \
         spawn; inspect and update each worker only after confirming its worktree is safe.",
        short_sha(repo_root, worker_base)
    ))
}

/// cas-d897 (GH #146): resolve the *fresher* of a base branch's local ref and
/// its `origin/` counterpart, and say which one won.
///
/// ROOT CAUSE this fixes: `resolve_spawn_base` returns a bare branch name and
/// `git worktree add … <name>` resolves it against the local ref only. In the
/// reported incident the local copy of the chosen branch sat at fb6ac595 (71
/// commits behind trunk) while `origin/`'s copy of that same branch was already
/// at b4f87100 — the fetch had landed, the local ref had simply never been
/// updated. The worker was cut from the past with no warning.
///
/// Behaviour, returned as `(checkout_ref_override, notice)`:
/// - no `origin/<base>` (or identical tips) → `(None, None)`: cut from the
///   local branch exactly as before;
/// - local strictly behind origin (fast-forwardable) → the override is
///   origin's **commit id**, plus a notice naming both SHAs and the gap;
/// - the two have diverged → no override (a divergent remote is not
///   automatically the right history) but the split is reported with both SHAs;
/// - local ahead → `(None, None)`.
///
/// The override is a raw commit id, not `origin/<base>`: the branch name the
/// worktree records as its parent must stay a *local* branch (it is the
/// merge-back target), and cutting from a commit id avoids silently setting the
/// worker branch's upstream to someone else's branch.
fn prefer_fresher_base_ref(
    repo_root: &std::path::Path,
    base: &str,
) -> (Option<String>, Option<String>) {
    // Already an explicit remote-tracking ref: nothing to compare against.
    if base.starts_with("origin/") {
        return (None, None);
    }
    // A remote-tracking ref is only evidence of the last fetch. Refresh it
    // before comparing so a long-running supervisor does not keep cutting
    // workers from an origin/main snapshot that is stale too.
    let _ = crate::worktree::GitOperations::new(repo_root.to_path_buf()).fetch_branch(base);
    let remote = format!("origin/{base}");
    if !ref_exists(repo_root, base) || !ref_exists(repo_root, &remote) {
        return (None, None);
    }
    let local_sha = short_sha(repo_root, base);
    let remote_sha = short_sha(repo_root, &remote);
    if local_sha == remote_sha && local_sha != "unknown" {
        return (None, None);
    }

    let behind = commits_behind(repo_root, base, &remote).unwrap_or(0);
    let ahead = commits_behind(repo_root, &remote, base).unwrap_or(0);

    match (behind, ahead) {
        (0, _) => (None, None),
        (behind, 0) => (
            full_sha(repo_root, &remote),
            Some(format!(
                "SPAWN BASE REFRESHED: local '{base}' ({local_sha}) is {behind} commit(s) behind \
                 '{remote}' ({remote_sha}); the worker worktree was cut from '{remote}' \
                 ({remote_sha}) so it starts on the fetched tip instead of the stale local ref \
                 (cas-d897 / GH #146). Update the local branch \
                 (`git fetch && git branch -f {base} {remote}`) to keep the two in step."
            )),
        ),
        (behind, ahead) => (
            None,
            Some(format!(
                "⚠️ SPAWN BASE DIVERGED: local '{base}' ({local_sha}) and '{remote}' \
                 ({remote_sha}) have diverged ({ahead} local-only, {behind} remote-only \
                 commit(s)). The worker was cut from the LOCAL ref '{base}' ({local_sha}); if the \
                 remote is the truth, reconcile the branch before relying on this worker's base \
                 (cas-d897 / GH #146)."
            )),
        ),
    }
}

/// Resolve the immutable commit a worker will be checked out from, preserving
/// the logical parent branch used for merge-back. A declared WorkTarget is
/// fetched before selection: it uses `origin/<branch>` when that is the
/// fresher tip, but retains a local-only tip when an epic-base refresh could
/// not be published. That keeps the checkout consistent with the refresh
/// notice instead of silently reverting to a stale remote-tracking ref.
fn checkout_ref_for_spawn_base(
    repo_root: &std::path::Path,
    parent_branch: &str,
    source: &SpawnBaseSource,
) -> (Option<String>, Option<String>, String) {
    let (mut base_ref, freshness_notice) = prefer_fresher_base_ref(repo_root, parent_branch);
    let mut checkout_ref = parent_branch.to_string();
    if matches!(source, SpawnBaseSource::WorkTarget { .. }) {
        let remote = format!("origin/{parent_branch}");
        if let (Some(local_sha), Some(remote_sha)) = (
            full_sha(repo_root, parent_branch),
            full_sha(repo_root, &remote),
        ) {
            // `prefer_fresher_base_ref` selected the remote only when it is
            // strictly ahead of local. Otherwise local is equal, ahead, or
            // divergent. Pin the equal case to the fetched object, but retain
            // the local object in the other two cases: an unpublished
            // fast-forward leaves local ahead while origin is necessarily
            // stale (GH #450).
            if local_sha == remote_sha || base_ref.as_deref() == Some(remote_sha.as_str()) {
                base_ref = Some(remote_sha);
                checkout_ref = remote;
            } else {
                base_ref = Some(local_sha);
            }
        }
    }
    (base_ref, freshness_notice, checkout_ref)
}

/// Fast-forward an epic's local base ref from the parent branch it recorded
/// when the two histories are cleanly aligned. A spawned worker merges back
/// into the epic ref, so updating *and publishing* that ref (rather than
/// merely checking out the parent commit) keeps the new worker, later syncs,
/// and the next spawning supervisor on the same integration history.
///
/// The parent is resolved from its fresher local/`origin/` ref before the
/// relationship is tested. This keeps the refresh decision aligned with
/// [`stale_spawn_base_notice`], which also treats a fetched remote parent as
/// current evidence. A genuinely split epic/parent history is refused: a
/// worker must not be cut from a base known to omit target history.
fn fast_forward_epic_base_from_parent(
    repo_root: &std::path::Path,
    epic_branch: &str,
    parent_branch: &str,
) -> Result<Option<String>, String> {
    if epic_branch == parent_branch
        || !ref_exists(repo_root, epic_branch)
        || !ref_exists(repo_root, parent_branch)
    {
        return Ok(None);
    }

    let epic_sha = full_sha(repo_root, epic_branch)
        .ok_or_else(|| format!("could not resolve epic base '{epic_branch}'"))?;
    let parent_ref = freshest_nondivergent_ref(repo_root, parent_branch)?;
    let parent_sha = full_sha(repo_root, &parent_ref)
        .ok_or_else(|| format!("could not resolve recorded parent '{parent_ref}'"))?;
    if epic_sha == parent_sha {
        return Ok(None);
    }

    let is_ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", epic_branch, &parent_ref])
        .current_dir(repo_root)
        .status()
        .map_err(|error| {
            format!(
                "could not check whether epic base '{epic_branch}' can fast-forward from '{parent_ref}': {error}"
            )
        })?;
    if !is_ancestor.success() {
        let parent_is_ancestor = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &parent_ref, epic_branch])
            .current_dir(repo_root)
            .status()
            .map_err(|error| {
                format!(
                    "could not check whether recorded parent '{parent_ref}' is contained in epic base '{epic_branch}': {error}"
                )
            })?;
        if parent_is_ancestor.success() {
            return Ok(None);
        }
        return Err(format!(
            "epic base '{epic_branch}' ({}) and recorded parent '{parent_ref}' ({}) have diverged; refusing to cut a worker from an unreconciled base",
            &epic_sha[..epic_sha.len().min(8)],
            &parent_sha[..parent_sha.len().min(8)],
        ));
    }

    let refname = if epic_branch.starts_with("refs/heads/") {
        epic_branch.to_string()
    } else if epic_branch.starts_with("refs/") || epic_branch.starts_with("origin/") {
        return Err(format!(
            "epic base '{epic_branch}' is not a local branch ref eligible for fast-forward"
        ));
    } else {
        format!("refs/heads/{epic_branch}")
    };
    let update = std::process::Command::new("git")
        .args(["update-ref", &refname, &parent_sha, &epic_sha])
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("could not fast-forward epic base '{epic_branch}': {error}"))?;
    if !update.status.success() {
        return Err(format!(
            "could not fast-forward epic base '{epic_branch}' from recorded parent '{parent_ref}': {}",
            String::from_utf8_lossy(&update.stderr).trim()
        ));
    }

    // Publishing is best-effort. The local fast-forward is already the safe
    // spawn base, so a remote-less checkout or rejected publication must not
    // roll it back and reintroduce the stale-base worker cut this refresh just
    // prevented.
    let origin_configured = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_root)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !origin_configured {
        return Ok(Some(format!(
            "EPIC BASE FAST-FORWARDED (LOCAL-ONLY): epic base '{epic_branch}' ({}) advanced to its recorded parent '{parent_branch}' via '{parent_ref}' ({}); no origin remote is configured, so the refreshed local ref is unpublished and will be used to cut the worker worktree.",
            &epic_sha[..epic_sha.len().min(8)],
            &parent_sha[..parent_sha.len().min(8)],
        )));
    }

    let push = std::process::Command::new("git")
        .args(["push", "origin", &format!("{refname}:{refname}")])
        .current_dir(repo_root)
        .output();
    if !matches!(&push, Ok(output) if output.status.success()) {
        let push_error = match push {
            Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
            Err(error) => error.to_string(),
        };
        return Ok(Some(format!(
            "EPIC BASE FAST-FORWARDED (UNPUBLISHED): epic base '{epic_branch}' ({}) advanced to its recorded parent '{parent_branch}' via '{parent_ref}' ({}), but push to origin failed: {push_error}. The refreshed local ref remains in effect and is unpublished; the worker worktree will be cut from the refreshed tip.",
            &epic_sha[..epic_sha.len().min(8)],
            &parent_sha[..parent_sha.len().min(8)],
        )));
    }

    Ok(Some(format!(
        "EPIC BASE FAST-FORWARDED: epic base '{epic_branch}' ({}) advanced to its recorded parent '{parent_branch}' via '{parent_ref}' ({}) and was pushed to origin before cutting the worker worktree.",
        &epic_sha[..epic_sha.len().min(8)],
        &parent_sha[..parent_sha.len().min(8)],
    )))
}

/// Turn a declared-parent refresh failure into a hard spawn refusal. The
/// branch pair in `error` is intentionally preserved so the operator knows
/// exactly which histories must be reconciled before another cut.
fn epic_base_refresh_refusal(error: &str) -> String {
    format!(
        "EPIC BASE REFRESH REFUSED: {error}. Reconcile the named branches before spawning a worker."
    )
}

/// Choose the current parent ref without silently picking one side of a
/// local/remote split. A fetched remote that strictly contains the local ref
/// is the freshest safe parent; an ahead local ref remains authoritative until
/// pushed; a true split must be reconciled before a worker can inherit it.
fn freshest_nondivergent_ref(repo_root: &std::path::Path, branch: &str) -> Result<String, String> {
    if branch.starts_with("origin/") {
        return Ok(branch.to_string());
    }
    let local = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    // Keep the fast-forward decision authoritative to the remote observed at
    // this spawn, rather than an origin/<branch> ref fetched hours earlier.
    let _ = crate::worktree::GitOperations::new(repo_root.to_path_buf()).fetch_branch(local);
    let remote = format!("origin/{local}");
    if !ref_exists(repo_root, &remote) {
        return Ok(branch.to_string());
    }

    let local_sha = full_sha(repo_root, branch)
        .ok_or_else(|| format!("could not resolve recorded parent '{branch}'"))?;
    let remote_sha = full_sha(repo_root, &remote)
        .ok_or_else(|| format!("could not resolve fetched parent '{remote}'"))?;
    if local_sha == remote_sha {
        return Ok(branch.to_string());
    }

    let local_is_ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", branch, &remote])
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("could not compare parent '{branch}' to '{remote}': {error}"))?;
    if local_is_ancestor.success() {
        return Ok(remote);
    }
    let remote_is_ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &remote, branch])
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("could not compare parent '{remote}' to '{branch}': {error}"))?;
    if remote_is_ancestor.success() {
        return Ok(branch.to_string());
    }

    Err(format!(
        "recorded parent '{branch}' ({}) and fetched '{remote}' ({}) have diverged; refusing to choose a worker base",
        &local_sha[..local_sha.len().min(8)],
        &remote_sha[..remote_sha.len().min(8)],
    ))
}

/// Return an epic branch and its explicitly declared integration parent. An
/// epic without a `WorkTarget` retains the legacy branch-only behaviour.
fn recorded_epic_parent_branch(
    cas_dir: &std::path::Path,
    epic_id: &str,
) -> Option<(String, String)> {
    let store = open_task_store(cas_dir).ok()?;
    let epic = store.get(epic_id).ok()?;
    let epic_branch = epic
        .branch
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or_else(|| crate::ui::factory::app::epic_branch_name(&epic.title));
    let parent_branch = epic
        .deliverables
        .work_target
        .as_ref()
        .map(|target| target.target_branch.trim())
        .filter(|branch| !branch.is_empty())?
        .to_string();
    Some((epic_branch, parent_branch))
}

/// Locate the epic whose recorded branch is the resolved spawn base, then
/// return that epic's declared parent. This is deliberately branch-oriented:
/// a task-level WorkTarget can resolve directly to an *outer* epic branch,
/// leaving no task-epic id in [`SpawnBaseSource`] even though that base itself
/// has a parent that must be refreshed before a worker is cut.
fn recorded_epic_parent_branch_for_resolved_base(
    cas_dir: &std::path::Path,
    resolved_base: &str,
) -> Option<(String, String)> {
    let store = open_task_store(cas_dir).ok()?;
    store
        .list(None)
        .ok()?
        .into_iter()
        .filter(|task| task.task_type == cas_types::TaskType::Epic)
        .find_map(|epic| {
            let epic_branch = epic.branch?.trim().to_string();
            if epic_branch.is_empty() || epic_branch != resolved_base {
                return None;
            }
            let parent_branch = epic
                .deliverables
                .work_target
                .as_ref()
                .map(|target| target.target_branch.trim())
                .filter(|branch| !branch.is_empty())?
                .to_string();
            Some((epic_branch, parent_branch))
        })
}

fn cleanup_cancelled_spawn_worktree_with_manager(
    manager: Option<&mut WorktreeManager>,
    result: &mut WorkerSpawnResult,
) -> anyhow::Result<bool> {
    if !result.worktree_created {
        return Ok(false);
    }
    let Some(worktree) = result.worktree.take() else {
        return Ok(false);
    };
    let Some(manager) = manager else {
        anyhow::bail!(
            "spawn created worktree '{}' but no worktree manager is available for cleanup",
            worktree.path.display()
        );
    };

    manager.register_worktree(&result.worker_name, worktree);
    manager.remove_worker(&result.worker_name, false)?;
    Ok(true)
}

/// Stamp `dirty_on_shutdown=true` (plus path + file count) onto the agent
/// record so the daemon reaper (Unit 3) can later salvage and reclaim the
/// orphaned worktree. Returns error only on store-level failures.
fn flag_agent_dirty_on_shutdown(
    agent_store: &dyn cas_store::AgentStore,
    agent_id: &str,
    path: &std::path::Path,
    file_count: usize,
) -> anyhow::Result<()> {
    let mut agent = agent_store.get(agent_id)?;
    agent
        .metadata
        .insert(DIRTY_ON_SHUTDOWN_KEY.to_string(), "true".to_string());
    agent.metadata.insert(
        "dirty_worktree_path".to_string(),
        path.display().to_string(),
    );
    agent
        .metadata
        .insert("dirty_worktree_files".to_string(), file_count.to_string());
    agent_store.update(&agent)?;
    Ok(())
}

/// Does this agent have any non-Closed task assigned? Used to decide whether
/// graceful shutdown can reclaim the worktree. On lookup failure we err on the
/// side of caution and treat the worker as still-busy so we never destroy work.
fn worker_has_open_tasks(cas_dir: &std::path::Path, agent_id: &str) -> bool {
    match open_task_store(cas_dir) {
        Ok(store) => match store.list(None) {
            Ok(tasks) => tasks
                .iter()
                .any(|t| t.assignee.as_deref() == Some(agent_id) && !t.is_terminal()),
            Err(e) => {
                tracing::warn!(
                    "worker_has_open_tasks: task list failed for agent '{agent_id}': {e} — assuming busy"
                );
                true
            }
        },
        Err(e) => {
            tracing::warn!("worker_has_open_tasks: open_task_store failed: {e} — assuming busy");
            true
        }
    }
}

/// cas-6913 / cas-7a94: pre-assign `task_id` to `worker_name` so the worker's
/// first `task action=mine` shows it without a follow-up assignment message.
/// Assignees are display names, not session IDs (cas-dbbb).
///
/// Called once the worker name is known (after `prepare_worker_spawn`, before
/// the isolate worktree finishes) and again at `finish_worker_spawn` as a
/// confirm path — so codex+isolate async gaps cannot skip the binding.
///
/// Best-effort and silent on failure by design: callers may already have a
/// live PTY, so raising here has nowhere useful to go. Every failure path is
/// logged instead. Never overwrites a *different* assignee; re-assigning to
/// the same worker is a no-op success (finish-path confirm after early assign).
///
/// Returns `true` when the task is assigned to `worker_name` after this call
/// (including already-ours), `false` on any miss/skip/error.
pub(crate) fn assign_task_to_new_worker(
    cas_dir: &std::path::Path,
    task_id: &str,
    worker_name: &str,
) -> bool {
    let store = match open_task_store(cas_dir) {
        Ok(store) => store,
        Err(e) => {
            tracing::error!(
                task_id, worker_name, error = %e,
                "cas-6913: failed to open task store for spawn-time task pre-assignment"
            );
            return false;
        }
    };

    let task = match store.get(task_id) {
        Ok(task) => task,
        Err(e) => {
            tracing::error!(
                task_id, worker_name, error = %e,
                "cas-6913: task not found for spawn-time pre-assignment"
            );
            return false;
        }
    };

    if matches!(
        task.status,
        cas_types::TaskStatus::Closed | cas_types::TaskStatus::Cancelled
    ) {
        tracing::warn!(
            task_id,
            worker_name,
            status = %task.status,
            "cas-8aee: refusing spawn-time pre-assignment for terminal task"
        );
        return false;
    }

    if let Some(ref existing) = task.assignee {
        if existing == worker_name {
            // Early-assign already pinned this worker (cas-7a94 isolate path).
            return true;
        }
        if let Err(reason) = reset_stale_preassign_holder(cas_dir, &task, existing) {
            tracing::warn!(
                task_id, worker_name, existing_assignee = %existing, reason,
                "cas-2327: refused spawn-time pre-assignment holder"
            );
            return false;
        }
        // Reset persisted successfully; reload before assigning so the normal
        // write below never carries stale status/assignee data forward.
        let task = match store.get(task_id) {
            Ok(task) => task,
            Err(e) => {
                tracing::error!(task_id, worker_name, error = %e, "cas-2327: stale-holder reset succeeded but task reload failed");
                return false;
            }
        };
        return assign_unassigned_task(&*store, task, task_id, worker_name);
    }

    assign_unassigned_task(&*store, task, task_id, worker_name)
}

fn assign_unassigned_task(
    store: &dyn cas_store::TaskStore,
    task: cas_types::Task,
    task_id: &str,
    worker_name: &str,
) -> bool {
    let mut updated = task;
    updated.assignee = Some(worker_name.to_string());
    updated.updated_at = chrono::Utc::now();
    match store.update(&updated) {
        Ok(_) => {
            tracing::info!(
                task_id,
                worker_name,
                "cas-6913: pre-assigned task to newly spawned worker"
            );
            true
        }
        Err(e) => {
            tracing::error!(
                task_id, worker_name, error = %e,
                "cas-6913: failed to persist spawn-time task pre-assignment"
            );
            false
        }
    }
}

/// Reset exactly one dead assignee before a replacement worker is bound.
///
/// Unlike a destructive task wipe, this preserves the task's notes, branch,
/// deliverables, and prior-status evidence. The explicit audit note is vital:
/// an orphan may have pushed real work before its session died.
fn reset_stale_preassign_holder(
    cas_dir: &std::path::Path,
    task: &cas_types::Task,
    holder: &str,
) -> Result<(), String> {
    let agents = open_agent_store(cas_dir)
        .map_err(|e| format!("could not inspect current assignee '{holder}': {e}"))?;
    // Task assignees are display names, not registry IDs. Inspect every
    // same-name row so an older stale registration cannot mask a fresh live
    // respawn. Listing failure means ownership is uncertain and must refuse
    // the destructive reset (cas-2327).
    let registered_agents = agents
        .list(None)
        .map_err(|e| format!("could not list current assignee '{holder}': {e}"))?;
    if crate::mcp::tools::service::agent_liveness::has_live_agent_named(&registered_agents, holder)
    {
        return Err(format!("task is still held by live worker '{holder}'"));
    }

    agents
        .release_lease_for_task(&task.id, "Stale pre-assignment force-release")
        .map_err(|e| format!("could not release stale holder '{holder}' lease: {e}"))?;

    let store = open_task_store(cas_dir)
        .map_err(|e| format!("could not open task store for stale-holder reset: {e}"))?;
    let mut reset = store
        .get(&task.id)
        .map_err(|e| format!("could not reload task after lease release: {e}"))?;
    if reset.assignee.as_deref() != Some(holder) {
        return Err(format!(
            "task assignee changed from stale holder '{holder}' while preparing reset"
        ));
    }
    let prior_status = reset.status;
    reset.status = cas_types::TaskStatus::Open;
    reset.assignee = None;
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let audit = format!(
        "[{timestamp}] ✅ DECISION cas-2327: force-released stale assignee '{holder}' with reset semantics; {prior_status:?}→Open. Notes, branch, and prior work were preserved before pre-assigning a replacement worker."
    );
    reset.notes = if reset.notes.is_empty() {
        audit
    } else {
        format!("{}\n\n{audit}", reset.notes)
    };
    reset.updated_at = chrono::Utc::now();
    store
        .update(&reset)
        .map(|_| ())
        .map_err(|e| format!("could not persist stale-holder reset: {e}"))
}

/// cas-7a94: release tasks bound to a dead/shutting-down worker so they are
/// claimable again without a manual `task action=reset`.
///
/// Covers pure spawn-time pre-assigns (`Open` + assignee, no lease) and the
/// ghost `InProgress`/`Blocked` state left when a worker dies before a real
/// lease is held. For each matching non-Closed task assigned to
/// `worker_name`: force-release any lease, force status to `Open`, clear
/// assignee. Closed and AwaitingMerge tasks are left alone (supervisor-owned
/// merge parking must not be clobbered by worker teardown).
///
/// Best-effort: store failures are logged; returns the number of tasks cleared.
pub(crate) fn release_worker_task_bindings(cas_dir: &std::path::Path, worker_name: &str) -> usize {
    let task_store = match open_task_store(cas_dir) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                worker_name, error = %e,
                "cas-7a94: failed to open task store for shutdown task release"
            );
            return 0;
        }
    };
    let agent_store = match open_agent_store(cas_dir) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                worker_name, error = %e,
                "cas-7a94: failed to open agent store for lease release — continuing with assignee clear"
            );
            // Continue without lease release — clearing assignee is still useful.
            // agent_store calls below are skipped when this is None-equivalent by
            // using a local flag.
            return release_worker_task_bindings_tasks_only(&*task_store, worker_name, None);
        }
    };

    release_worker_task_bindings_tasks_only(&*task_store, worker_name, Some(&*agent_store))
}

fn release_worker_task_bindings_tasks_only(
    task_store: &dyn cas_store::TaskStore,
    worker_name: &str,
    agent_store: Option<&dyn cas_store::AgentStore>,
) -> usize {
    let assigned: Vec<_> = match task_store.list(None) {
        Ok(tasks) => tasks
            .into_iter()
            .filter(|t| {
                t.assignee.as_deref() == Some(worker_name)
                    && matches!(
                        t.status,
                        cas_types::TaskStatus::Open
                            | cas_types::TaskStatus::InProgress
                            | cas_types::TaskStatus::Blocked
                    )
            })
            .collect(),
        Err(e) => {
            tracing::error!(
                worker_name, error = %e,
                "cas-7a94: failed to list tasks for shutdown release"
            );
            return 0;
        }
    };

    let mut released = 0usize;
    for mut t in assigned {
        if let Some(agents) = agent_store {
            let _ = agents.release_lease_for_task(&t.id, "Worker shutdown/cancel cleanup");
        }
        t.status = cas_types::TaskStatus::Open;
        t.assignee = None;
        t.updated_at = chrono::Utc::now();
        match task_store.update(&t) {
            Ok(_) => {
                released += 1;
                tracing::info!(
                    task_id = %t.id,
                    worker_name,
                    "cas-7a94: released task binding on worker shutdown/cancel"
                );
            }
            Err(e) => {
                tracing::error!(
                    task_id = %t.id, worker_name, error = %e,
                    "cas-7a94: failed to clear assignee on worker shutdown"
                );
            }
        }
    }
    released
}

/// cas-7a94: release a single task by id if it is still bound to `worker_name`
/// (or bound to anyone when `worker_name` is empty — used for failed pre-boots
/// where we know the task_id but want a surgical clear only when assignee
/// matches the aborted worker).
pub(crate) fn release_preassign_if_bound(
    cas_dir: &std::path::Path,
    task_id: &str,
    worker_name: &str,
) {
    let store = match open_task_store(cas_dir) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                task_id, worker_name, error = %e,
                "cas-7a94: failed to open task store for pre-assign release"
            );
            return;
        }
    };
    let mut task = match store.get(task_id) {
        Ok(t) => t,
        Err(_) => return,
    };
    if task.assignee.as_deref() != Some(worker_name) {
        return;
    }
    if matches!(
        task.status,
        cas_types::TaskStatus::Closed | cas_types::TaskStatus::AwaitingMerge
    ) {
        return;
    }
    if let Ok(agents) = open_agent_store(cas_dir) {
        let _ = agents.release_lease_for_task(task_id, "Preassigned worker startup aborted");
    }
    task.status = cas_types::TaskStatus::Open;
    task.assignee = None;
    task.updated_at = chrono::Utc::now();
    if let Err(e) = store.update(&task) {
        tracing::error!(
            task_id, worker_name, error = %e,
            "cas-7a94: failed to release pre-assign after aborted spawn"
        );
    } else {
        tracing::info!(
            task_id,
            worker_name,
            "cas-7a94: released pre-assign after aborted/cancelled spawn"
        );
    }
}

fn shutdown_scope(count: Option<usize>, names: &[String]) -> &'static str {
    if !names.is_empty() {
        "named"
    } else if count.unwrap_or(0) == 0 {
        "all"
    } else {
        "count"
    }
}

impl FactoryApp {
    /// Get the current epic state
    pub fn epic_state(&self) -> &EpicState {
        &self.epic_state
    }

    /// Handle epic state transitions based on detected events
    ///
    /// Returns true if state changed (for branch management).
    pub fn handle_epic_events(&mut self, events: &[DirectorEvent]) -> Vec<EpicStateChange> {
        let mut changes = Vec::new();

        for event in events {
            match event {
                DirectorEvent::EpicStarted {
                    epic_id,
                    epic_title,
                } => {
                    let source = self.source_for_detected_epic_started(epic_id);
                    if !self.can_adopt_detected_epic_started(epic_id, source) {
                        continue;
                    }
                    let previous = self.set_active_epic(epic_id, epic_title, source);

                    changes.push(EpicStateChange::Started {
                        epic_id: epic_id.clone(),
                        epic_title: epic_title.clone(),
                        previous_state: previous,
                    });
                }

                DirectorEvent::EpicCompleted { epic_id } => {
                    // Check if this is our current epic
                    if self.epic_state.epic_id() == Some(epic_id) {
                        let title = self
                            .epic_state
                            .epic_title()
                            .unwrap_or("Unknown")
                            .to_string();

                        // Transition to Completing state
                        self.epic_state = EpicState::Completing {
                            epic_id: epic_id.clone(),
                            epic_title: title.clone(),
                        };
                        self.current_epic_id = None;
                        self.current_epic_source = None;
                        self.clear_persisted_current_epic_id();

                        changes.push(EpicStateChange::Completed {
                            epic_id: epic_id.clone(),
                            epic_title: title,
                        });
                    }
                }

                _ => {}
            }
        }

        changes
    }

    /// Reset epic state to idle (after merge completes)
    pub fn reset_epic_state(&mut self) {
        self.epic_state = EpicState::Idle;
        self.current_epic_id = None;
        self.current_epic_source = None;
        self.clear_persisted_current_epic_id();
    }

    /// Re-read the live `LlmConfig` from disk and update the mux's worker CLI,
    /// model, and effort before a dynamic spawn.
    ///
    /// This ensures that `cas config set llm.worker.harness codex` is picked up
    /// on the **next** `spawn_workers` call without restarting the daemon
    /// (cas-9bc6 fix: the harness was previously cached at daemon boot).
    ///
    /// Also updates `self.worker_cli` on `FactoryApp` so the per-worker intro
    /// prompt (`queue_codex_worker_intro_prompt`) uses the correct harness.
    ///
    /// On I/O or parse failure the existing cached values are retained —
    /// degraded but not broken.
    fn sync_worker_config_from_live_settings(&mut self) {
        use std::str::FromStr;

        let config = match Config::load(self.cas_dir()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "failed to re-read live config before spawn; \
                     cached worker harness retained: {}",
                    e
                );
                return;
            }
        };
        let llm = config.llm();

        // Harness — the field that was previously cached and never refreshed.
        let live_cli = cas_mux::SupervisorCli::from_str(llm.harness_for_role("worker"))
            .unwrap_or(cas_mux::SupervisorCli::Claude);
        // Keep FactoryApp's own field in sync so queue_codex_worker_intro_prompt
        // and generate_prompt pick up the updated harness as well.
        self.worker_cli = live_cli;

        // Re-read model and effort for consistency; these were already correct
        // at startup but can drift if config changes mid-session.
        let worker_model = llm.model_for_role("worker").map(ToOwned::to_owned);
        let worker_effort = llm
            .reasoning_effort_for_role("worker")
            .and_then(|effort| effort.parse::<cas_mux::Effort>().ok());
        self.mux.set_default_worker_spec(cas_mux::WorkerSpec {
            name: None,
            cli: live_cli,
            model: worker_model,
            effort: worker_effort,
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        });
    }

    /// Add a new worker at runtime (synchronous - blocks during worktree creation).
    ///
    /// Creates a worktree (if isolate is true and worktrees enabled) and spawns a Claude instance.
    /// For non-blocking spawning, use `prepare_worker_spawn` + `finish_worker_spawn`.
    pub fn spawn_worker(&mut self, name: Option<&str>, isolate: bool) -> anyhow::Result<String> {
        let prep = self.prepare_worker_spawn(name, isolate, None)?;
        let result = match prep.run() {
            Ok(result) => result,
            Err(e) => {
                crate::telemetry::track(
                    "factory_worker_spawn_result",
                    vec![("success", "false"), ("reason", "worktree_prepare_failed")],
                );
                return Err(e);
            }
        };
        self.finish_worker_spawn(result, None, None, None)
    }

    /// Remove a worktree created by a spawn generation that was cancelled
    /// before its pane was registered. Reused worktrees predate this spawn and
    /// are deliberately preserved.
    pub(crate) fn cleanup_cancelled_spawn_worktree(
        &mut self,
        result: &mut WorkerSpawnResult,
    ) -> anyhow::Result<bool> {
        cleanup_cancelled_spawn_worktree_with_manager(self.worktree_manager.as_mut(), result)
    }

    /// Phase 1: Prepare spawn data (fast, runs on main thread).
    ///
    /// Resolves the worker name, computes paths, and returns a `WorkerSpawnPrep`
    /// that can be sent to a background thread for the slow git operations.
    ///
    /// When `isolate` is true and worktrees are configured, each worker gets its
    /// own git worktree and branch. When false, workers share the main working directory.
    ///
    /// `task_id` is the task the spawn was requested for (`spawn_workers
    /// task_id=...`). cas-7587 (GH #122): when present, the worktree base is
    /// resolved from *that task's* epic branch, not from the session's pinned
    /// epic focus — the two can name different epics, and the task is right.
    pub fn prepare_worker_spawn(
        &mut self,
        name: Option<&str>,
        isolate: bool,
        task_id: Option<&str>,
    ) -> anyhow::Result<WorkerSpawnPrep> {
        // focus_epic is persisted outside cas.db, so reconcile the task
        // snapshot and session metadata synchronously at spawn time.
        if let Err(error) = self.refresh_data() {
            tracing::warn!(
                error = %error,
                "failed to refresh factory data before worker spawn; using cached task data"
            );
        }
        self.apply_session_metadata_focus();

        let spawn_type = if name.is_some() { "named" } else { "anonymous" };
        crate::telemetry::track(
            "factory_worker_spawn_requested",
            vec![
                ("spawn_type", spawn_type),
                ("worktrees_enabled", bool_prop(self.worktrees_enabled())),
                ("isolate", bool_prop(isolate)),
            ],
        );

        // Generate a unique name if not provided
        let worker_name = match name {
            Some(n) => n.to_string(),
            None => {
                let existing: std::collections::HashSet<&str> =
                    self.worker_names.iter().map(|s| s.as_str()).collect();
                let mut candidate = generate_unique(1)[0].clone();
                let mut attempts = 0;
                while existing.contains(candidate.as_str()) && attempts < 100 {
                    candidate = generate_unique(1)[0].clone();
                    attempts += 1;
                }
                candidate
            }
        };

        if self.worker_names.contains(&worker_name) {
            crate::telemetry::track(
                "factory_worker_spawn_result",
                vec![("success", "false"), ("reason", "worker_exists")],
            );
            anyhow::bail!("Worker '{worker_name}' already exists");
        }

        let (worktree_info, base_warnings, base_provenance) = if isolate {
            if let Some(manager) = &self.worktree_manager {
                // Re-resolve the repository on every request. A daemon started
                // before `git init` may have latched an ancestor repository;
                // continuing with that stale root would create worker branches
                // in the wrong project. The verified-spawn lifecycle surfaces
                // this per-request failure to the supervisor.
                validate_live_spawn_repo_context(manager, self.project_path())?;
                // Verify repo has commits before trying to create worktrees
                if !manager.git().has_commits().unwrap_or(false) {
                    crate::telemetry::track(
                        "factory_worker_spawn_result",
                        vec![("success", "false"), ("reason", "repo_has_no_commits")],
                    );
                    anyhow::bail!(
                        "Repository has no commits. Please make an initial commit before spawning workers."
                    );
                }

                let session_repo_root = manager.repo_root().to_path_buf();
                let task_base = task_id
                    .map(|tid| task_epic_base(&self.cas_dir, &session_repo_root, tid))
                    .unwrap_or(TaskBase::Unresolved);
                let repo_root = resolve_spawn_worktree_repo(
                    &self.cas_dir,
                    &session_repo_root,
                    task_base.work_target(),
                )?;
                let cross_repo = repo_root != session_repo_root;
                let spawn_git = crate::worktree::GitOperations::new(repo_root.clone());
                let worktree_path = if cross_repo {
                    repo_root.join(".cas/worktrees").join(&worker_name)
                } else {
                    manager.worktree_path_for_worker(&worker_name)
                };
                let branch_name = manager.branch_name_for_worker(&worker_name);
                // Dynamic spawns must match startup spawns: never the
                // supervisor's incidental HEAD. cas-7587 (GH #122): precedence
                // is the pre-assigned task's epic branch first, pinned epic
                // focus second, trunk last.
                let configured_trunk = Config::configured_epic_base_branch(&repo_root)
                    .unwrap_or_else(|| spawn_git.detect_default_branch());
                // An epic's declared delivery target is authoritative for both
                // a no-epic child fallback and stale-base comparison. Falling
                // back to factory configuration keeps legacy/taskless spawns.
                let trunk = task_base
                    .target_branch()
                    .unwrap_or(&configured_trunk)
                    .to_string();
                let task_epic = task_base.epic().cloned();
                let (parent_branch, base_source) =
                    resolve_spawn_base(&task_base, self.epic_branch.as_deref(), &trunk);
                let mut notices: Vec<String> = Vec::new();
                let base_epic_id = match &base_source {
                    SpawnBaseSource::TaskEpic { epic_id, .. } => Some(epic_id.as_str()),
                    SpawnBaseSource::PinnedFocus => self.current_epic_id.as_deref(),
                    SpawnBaseSource::WorkTarget { .. }
                    | SpawnBaseSource::TaskWithoutEpic { .. }
                    | SpawnBaseSource::Trunk => None,
                };
                // cas-b6f5 (GH #434): a task-level WorkTarget may point at
                // an outer epic branch, so the winning SpawnBaseSource has no
                // task-epic id even though the resolved base itself records a
                // parent. Look up that base as an epic after retaining the
                // direct child-epic path used by cas-83f6.
                let recorded_base_parent = base_epic_id
                    .and_then(|epic_id| recorded_epic_parent_branch(&self.cas_dir, epic_id))
                    .filter(|(epic_branch, _)| epic_branch == &parent_branch)
                    .or_else(|| {
                        recorded_epic_parent_branch_for_resolved_base(&self.cas_dir, &parent_branch)
                    });
                if let Some((epic_branch, recorded_parent)) = recorded_base_parent {
                    let refresh = fast_forward_epic_base_from_parent(
                        &repo_root,
                        &epic_branch,
                        &recorded_parent,
                    )
                    .map_err(|error| anyhow::anyhow!("{}", epic_base_refresh_refusal(&error)))?;
                    if let Some(notice) = refresh {
                        notices.push(notice);
                    }
                }
                // cas-d897 (GH #146): the winning branch name still has to be
                // resolved to the fresher of its local and origin refs — a
                // stale local ref silently backdates every worker cut from it.
                let (base_ref, freshness_notice, checkout_ref) =
                    checkout_ref_for_spawn_base(&repo_root, &parent_branch, &base_source);
                if let Some(notice) = freshness_notice {
                    notices.push(notice);
                }
                // `parent_branch` remains the local merge-back target, but a
                // refreshed spawn is actually cut from `base_ref`. Warnings
                // must assess that effective checkout ref or they contradict
                // the successful origin-based refresh they just announced.
                let effective_base = base_ref.as_deref().unwrap_or(&parent_branch);
                let mut provenance = spawn_base_provenance_notice(
                    &parent_branch,
                    &base_source,
                    self.epic_branch.as_deref(),
                );
                let checkout_sha = short_sha(&repo_root, effective_base);
                provenance.push_str(&format!(
                    " CHECKOUT BASE: '{checkout_ref}' @ {checkout_sha}."
                ));
                if base_diverges_from_focus(
                    &parent_branch,
                    &base_source,
                    self.epic_branch.as_deref(),
                ) {
                    notices.push(provenance.clone());
                }
                // The base must contain the epic it is meant to serve — the
                // task's epic when that decided the base, otherwise the focus.
                let epic_to_contain = match &base_source {
                    SpawnBaseSource::TaskEpic { .. } => {
                        task_epic.as_ref().map(|t| t.branch.clone())
                    }
                    SpawnBaseSource::WorkTarget { .. } => None,
                    _ => self.epic_branch.clone(),
                };
                if let Some(notice) = epic_to_contain.as_deref().and_then(|epic_branch| {
                    worker_base_mismatch_notice(&repo_root, effective_base, epic_branch)
                }) {
                    notices.push(notice);
                }
                // cas-7587: a task whose epic branch does not exist locally
                // still lands on the focus base — say so instead of letting it
                // look like the task's epic was honoured.
                if !matches!(base_source, SpawnBaseSource::WorkTarget { .. })
                    && let Some(unresolved) = task_epic.as_ref().filter(|t| !t.branch_exists)
                {
                    notices.push(format!(
                        "SPAWN BASE FALLBACK: task {} belongs to epic {} whose branch '{}' does \
                         not exist in this repository; the worker was cut from '{parent_branch}' \
                         instead. Create the epic branch (or fix the epic's branch field) before \
                         relying on this worker's base.",
                        unresolved.task_id, unresolved.epic_id, unresolved.branch
                    ));
                }
                if let Some(notice) =
                    stale_legacy_slug_notice(task_epic.as_ref(), &parent_branch, &base_source)
                {
                    notices.push(notice);
                }
                // cas-ecf7 (GH #118): the base ref is resolved live, but the
                // branch it names can be far behind trunk. Surface that at
                // spawn time instead of leaving it to whoever happens to read
                // `behind:` in worker_status.
                if let Some(notice) =
                    stale_spawn_base_notice(&repo_root, effective_base, &trunk)
                {
                    notices.push(notice);
                }
                (
                    Some(WorktreePrep {
                        worktree_path,
                        branch_name,
                        parent_branch,
                        base_ref,
                        repo_root,
                        cas_dir: self.cas_dir.clone(),
                    }),
                    notices,
                    Some(provenance),
                )
            } else {
                anyhow::bail!(
                    "Worker isolation requested but worktrees are not enabled. \
                     Start the factory with --worktrees to enable isolation."
                );
            }
        } else {
            (None, Vec::new(), None)
        };

        if let Some(provenance) = &base_provenance {
            tracing::info!("{provenance}");
        }

        for notice in &base_warnings {
            tracing::warn!("{notice}");
            self.set_error(notice.clone());
        }

        crate::telemetry::track(
            "factory_worker_spawn_prepared",
            vec![
                ("spawn_type", spawn_type),
                ("worktrees_enabled", bool_prop(worktree_info.is_some())),
            ],
        );

        Ok(WorkerSpawnPrep {
            worker_name,
            worktree_info,
            warnings: base_warnings,
            base_provenance,
        })
    }

    /// Phase 3: Finish spawn on main thread (fast - adds pane to mux, updates tracking).
    ///
    /// `teams` provides per-worker Agent Teams CLI flags. When `Some`, the spawned
    /// agent will bootstrap with native Teams inbox polling. The daemon builds this
    /// from `TeamsManager::spawn_config_for()` for each worker individually.
    pub fn finish_worker_spawn(
        &mut self,
        result: WorkerSpawnResult,
        teams: Option<cas_mux::TeamsSpawnConfig>,
        spec: Option<cas_mux::WorkerSpec>,
        task_id: Option<String>,
    ) -> anyhow::Result<String> {
        // cas-9bc6: re-read live LlmConfig so harness/model/effort changes made
        // via `cas config set` after daemon boot are reflected in this spawn.
        self.sync_worker_config_from_live_settings();

        let worker_name = result.worker_name;
        let cwd = result.cwd;
        let cas_root = result.cas_root;

        // Validate the effective post-cascade spec immediately before the
        // PTY launch. Queue producers validate their resolved payloads, but a
        // dynamic config reload or a legacy queue row can still supply a
        // different default at this boundary. Borrowing the spec here keeps
        // explicit triples immutable while making this the final fail-closed
        // gate before `Mux::add_worker` starts a process.
        let effective_spec = self.mux.effective_worker_spec(&worker_name, spec.clone());
        cas_factory::validate_explicit(
            &effective_spec,
            &cas_factory::CapabilitySnapshot::default(),
        )
        .map_err(|error| anyhow::anyhow!("Failed to validate worker routing spec: {error}"))?;

        // STEP 3 (cas-5232): Capture expected branch before worktree is consumed.
        let expected_branch: Option<String> = result.worktree.as_ref().map(|wt| wt.branch.clone());

        // cas-30c6: PRE-harness binding gate — fail closed.
        //
        // The post-spawn assertion below used to be the only check, and it runs
        // after `mux.add_worker` has already started the PTY, so it can do
        // nothing but log: by then the harness is live on the wrong repository
        // and will accept a task there (spawn request 1031). Prove the binding
        // BEFORE the process exists so a misbound spawn simply fails; the
        // daemon's existing failure path releases the early task pre-assign and
        // reports the refusal to the supervisor.
        if let Some(ref expected) = expected_branch {
            if let Err(e) = crate::factory_isolation::verify_worker_worktree_binding(
                &worker_name,
                &cwd,
                Some(expected),
            ) {
                crate::telemetry::track(
                    "factory_worker_spawn_result",
                    vec![("success", "false"), ("reason", "worktree_binding_failed")],
                );
                tracing::error!(
                    worker = %worker_name,
                    cwd = %cwd.display(),
                    expected_branch = %expected,
                    error = %e,
                    "ISOLATION BUG pre-spawn: refusing to start a harness that is not bound \
                     to its own worktree"
                );
                return Err(e);
            }
        }

        // Register the worktree with the manager if applicable
        if let (Some(manager), Some(wt)) = (&mut self.worktree_manager, result.worktree) {
            manager.register_worktree(&worker_name, wt);
        }

        tracing::info!("Adding worker pane: {} in {:?}", worker_name, cwd);

        // Capture effective CLI before spec is moved into add_worker.
        // Explicit spec overrides session default so the intro prompt matches the actual harness.
        let effective_cli = spec.as_ref().map(|s| s.cli).unwrap_or(self.worker_cli);

        if let Err(e) = self.mux.add_worker(
            &worker_name,
            cwd.clone(),
            cas_root.as_ref(),
            &self.supervisor_name,
            teams.as_ref(),
            spec, // cas-4cae: per-spawn spec override from SpawnWorkers protocol
        ) {
            crate::telemetry::track(
                "factory_worker_spawn_result",
                vec![("success", "false"), ("reason", "mux_add_worker_failed")],
            );
            return Err(e.into());
        }
        self.track_worker_process_group(&worker_name);

        // STEP 3 (cas-5232): Post-spawn branch assertion — now the SECOND belt.
        // The cas-30c6 gate above already refused a misbound spawn before the
        // PTY existed; this remains to catch drift between that gate and the
        // running process (concurrent FS changes, git state drift). It still
        // only logs: the process is live by this point, and unwinding here
        // would orphan it.
        if let Some(ref expected) = expected_branch {
            if let Err(e) =
                crate::ui::factory::app::verify_isolated_worker_branch(&worker_name, &cwd, expected)
            {
                tracing::error!(
                    worker = %worker_name,
                    cwd = %cwd.display(),
                    expected_branch = %expected,
                    error = %e,
                    "ISOLATION BUG post-spawn: worker cwd not on expected branch — \
                     see EPIC cas-073f. Worker is running but may commit to wrong ref."
                );
                // Do NOT return Err here — the PTY process is already running and
                // returning would leave the worker pane without being tracked.
            }
        }

        // Track the worker name
        self.worker_names.push(worker_name.clone());

        // cas-6913 / cas-7a94: pre-assign (or confirm early-assign) now that
        // the worker name is final and the PTY is up. Best-effort — a failure
        // here must not unwind the spawn.
        if let Some(ref task_id) = task_id {
            let ok = assign_task_to_new_worker(self.cas_dir(), task_id, &worker_name);
            if !ok {
                tracing::warn!(
                    task_id = %task_id,
                    worker = %worker_name,
                    "cas-7a94: finish_worker_spawn pre-assign did not stick — \
                     worker boots without the requested task"
                );
            }
        }

        crate::ui::factory::app::queue_codex_worker_intro_prompt(
            self.cas_dir(),
            &worker_name,
            effective_cli,
        );

        // Update event detector so it recognizes this worker's events
        self.event_detector.add_worker(worker_name.clone());

        // Update pane grid for navigation
        self.pane_grid = PaneGrid::new(&self.worker_names, &self.supervisor_name, self.is_tabbed);
        self.sync_worker_pane_branch_titles();

        // Sync pane sizes to accommodate new worker
        let _ = self.sync_pane_sizes();

        let workers_active = self.worker_names.len().to_string();
        crate::telemetry::track(
            "factory_worker_spawn_result",
            vec![("success", "true"), ("workers_active", &workers_active)],
        );

        tracing::info!("spawn_worker completed: {}", worker_name);
        Ok(worker_name)
    }

    /// Shutdown a worker by name
    ///
    /// Kills the worker's PTY process tree and cleans up its clone (if any).
    ///
    /// # Arguments
    /// * `name`  - Worker name to shutdown
    /// * `force` - `true` → SIGKILL the process group immediately;
    ///             `false` → SIGTERM with group-wide SIGKILL escalation
    pub async fn shutdown_worker(&mut self, name: &str, force: bool) -> anyhow::Result<()> {
        // Check if worker exists
        if !self.worker_names.contains(&name.to_string()) {
            anyhow::bail!("Worker '{name}' not found");
        }

        let cas_dir = self.cas_dir().to_path_buf();

        // Mark agent as shutdown in Cassy first; this must succeed so supervisor sees errors
        // instead of silently leaving stale idle agents in director panels.
        let agent_store = open_agent_store(self.cas_dir())?;
        let agents = agent_store.list(None)?;
        let matching_agents =
            worker_registry_rows_for_shutdown(&agents, name, self.factory_session.as_deref());
        let agent = matching_agents.first().copied().ok_or_else(|| {
            let known_workers: Vec<String> = agents
                .iter()
                .filter(|a| a.role == cas_types::AgentRole::Worker)
                .map(|a| a.name.clone())
                .collect();
            anyhow::anyhow!(
                "Cannot shutdown worker '{}': no exact Cassy agent record found. Known worker records: {}",
                name,
                if known_workers.is_empty() {
                    "(none)".to_string()
                } else {
                    known_workers.join(", ")
                }
            )
        })?;

        // `graceful_shutdown` revokes leases. Preserve this snapshot so the
        // death relay can enumerate and park leased tasks even if no assignee
        // column was populated for them.
        let held_task_ids = matching_agents
            .iter()
            .flat_map(|matching| {
                agent_store
                    .list_agent_leases(&matching.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|lease| lease.task_id)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        // Snapshot task state AFTER release — should be free of this worker's
        // Open/InProgress/Blocked bindings. Remaining non-Closed work (e.g.
        // AwaitingMerge) still blocks worktree reclaim.
        let agent_id = agent.id.clone();
        let has_open_tasks =
            worker_has_open_tasks(&cas_dir, name) || worker_has_open_tasks(&cas_dir, &agent_id);

        // Retire every same-name row in this factory session. Older builds
        // allowed nested headless Claude calls to register child session IDs
        // under the parent worker name; shutting down only `.find()` left the
        // remaining rows fresh/Active after the pane and worktree were gone.
        let mut retirement_failures = Vec::new();
        for matching in &matching_agents {
            if let Err(error) = agent_store.graceful_shutdown(&matching.id) {
                let fallback = agent_store.mark_stale(&matching.id);
                retirement_failures.push(format!(
                    "{}: {error} (fallback: {})",
                    matching.id,
                    match fallback {
                        Ok(()) => "marked stale".to_string(),
                        Err(mark_error) => format!("failed: {mark_error}"),
                    }
                ));
            }
        }
        if !retirement_failures.is_empty() {
            anyhow::bail!(
                "Failed to retire all Cassy identities for worker '{name}': {}",
                retirement_failures.join("; ")
            );
        }

        // Kill the worker's PTY process tree and remove from mux (cas-8c5a).
        // kill_worker signals the entire process group (SIGKILL when force=true,
        // SIGTERM when false), ensuring the full node→codex tree is terminated,
        // not just the direct PTY child that SIGHUP-on-drop would reach.
        let process_group = self.mux.pane_process_group_id(name);
        self.mux.kill_worker(name, force).await?;
        if let Some(pgid) = process_group {
            self.untrack_worker_process_group_if_gone(pgid).await;
        }

        // Emit the same durable supervisor lifecycle relay used for unexpected
        // PTY exits. Do this only after the process is actually gone, but
        // before the legacy binding cleanup, so the relay records and parks
        // any task that was held at termination instead of reporting a
        // misleading empty task set.
        crate::mcp::tools::service::orphan_recovery::recover_worker_vanished(
            &cas_dir,
            agent_store.as_ref(),
            agent,
            &held_task_ids,
            "worker terminated by shutdown request",
        );

        // cas-7a94: clear pure Open pre-assigns and any binding the recovery
        // path could not inspect. Assignees are display names (cas-dbbb), so
        // match on `name` rather than the registration UUID.
        let released = release_worker_task_bindings(&cas_dir, name);
        if released > 0 {
            tracing::info!(
                worker = %name,
                released,
                "cas-7a94: released remaining task bindings on shutdown_worker"
            );
        }

        // Remove from tracking
        self.worker_names.retain(|n| n != name);

        // Force a DB reload next refresh; relying only on mtime can miss rapid same-second writes.
        self.last_db_fingerprint = None;
        // Refresh director data immediately so UI shows updated state
        let _ = self.refresh_data();

        // Update event detector
        self.remove_worker_from_event_detector(name);

        // Update pane grid for navigation
        self.pane_grid = PaneGrid::new(&self.worker_names, &self.supervisor_name, self.is_tabbed);

        // Ensure selected tab is still valid
        self.clamp_selected_worker_tab();

        // Teardown the worker's worktree when it's safe. "Safe" means all of its
        // tasks are Closed AND the tree is clean — we never destroy in-progress
        // work. Dirty trees are preserved for the daemon reaper (Unit 3) to
        // salvage later, and we flag the agent record + warn the supervisor so
        // nothing is silently abandoned.
        if !has_open_tasks {
            self.finalize_worker_worktree(&agent_store, &agent_id, name);
        }

        // Sync pane sizes to adjust layout
        let _ = self.sync_pane_sizes();

        Ok(())
    }

    /// Shared teardown: attempt to remove a worker's worktree on graceful close
    /// (all tasks Closed). Branches: clean → removed + branch deleted; dirty →
    /// preserved, warning surfaced, agent metadata flagged for later reaper.
    fn finalize_worker_worktree(
        &mut self,
        agent_store: &std::sync::Arc<dyn cas_store::AgentStore>,
        agent_id: &str,
        name: &str,
    ) {
        let Some(manager) = self.worktree_manager.as_mut() else {
            return;
        };

        let outcome = match manager.attempt_remove_worker(name) {
            Ok(o) => o,
            Err(e) => {
                self.set_error(format!("Worker '{name}' worktree cleanup failed: {e}"));
                return;
            }
        };

        match outcome {
            RemoveOutcome::NotTracked | RemoveOutcome::Removed => {}
            RemoveOutcome::DirtyDeferred(warning) => {
                self.set_error(format!(
                    "Worker '{}' shut down with {} uncommitted file{} at {} — worktree preserved for salvage",
                    warning.worker_name,
                    warning.file_count,
                    if warning.file_count == 1 { "" } else { "s" },
                    warning.path.display(),
                ));

                if let Err(e) = flag_agent_dirty_on_shutdown(
                    agent_store.as_ref(),
                    agent_id,
                    &warning.path,
                    warning.file_count,
                ) {
                    tracing::warn!("Failed to flag dirty_on_shutdown for agent '{agent_id}': {e}");
                }
            }
            RemoveOutcome::ExternalSymlinksBlocked(warning) => {
                // cas-df97: live external symlinks (e.g. dotfiles a
                // stow/install step pointed at this worktree by mistake)
                // resolve into it — removing the directory would leave
                // every one dangling. Preserve the worktree and name every
                // offending link so it's fixable without spelunking.
                let links_desc = warning
                    .links
                    .iter()
                    .map(|l| format!("{} -> {}", l.link.display(), l.target.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.set_error(format!(
                    "Worker '{}' shut down but worktree at {} has {} live external symlink{} pointing into it — worktree preserved, NOT removed: {}",
                    warning.worker_name,
                    warning.path.display(),
                    warning.links.len(),
                    if warning.links.len() == 1 { "" } else { "s" },
                    links_desc,
                ));
            }
        }
    }

    /// Mark a worker as crashed (removes from tracking, keeps worktree for respawn)
    ///
    /// Called when a worker PTY exits unexpectedly. Unlike `shutdown_worker`,
    /// we do not remove the pane from mux (already gone). The worktree is
    /// preserved for respawn *unless* the worker's task has already been closed
    /// AND the tree is clean — in that case we reclaim it the same way graceful
    /// shutdown would. Dirty trees are always preserved and flagged for salvage.
    pub async fn mark_worker_crashed(&mut self, name: &str) {
        // The interactive leader may have exited while a long-lived child
        // remains in its process group. Reap the durable group before making
        // this lane eligible for respawn.
        if let Some(record) = crate::ui::factory::process_groups::list(self.cas_dir())
            .unwrap_or_default()
            .into_iter()
            .find(|record| {
                record.worker_name == name
                    && self
                        .factory_session()
                        .is_none_or(|session| record.factory_session == session)
            })
        {
            match crate::ui::factory::process_groups::reap(self.cas_dir(), &record).await {
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    worker = %name,
                    pgid = record.pgid,
                    error = %error,
                    "crashed worker process group survived reap; retaining GC record"
                ),
            }
        }

        // Remove from worker tracking
        self.worker_names.retain(|n| n != name);

        // Update event detector (suppresses future events from this worker)
        self.remove_worker_from_event_detector(name);

        // Update pane grid for navigation
        self.pane_grid = PaneGrid::new(&self.worker_names, &self.supervisor_name, self.is_tabbed);

        // Ensure selected tab is still valid
        self.clamp_selected_worker_tab();

        // Determine if this crashed worker can have its worktree reclaimed.
        // Default: preserve for respawn. Only reclaim when all assigned tasks
        // are Closed (supervisor has moved on) AND tree is clean.
        let cas_dir = self.cas_dir().to_path_buf();
        if let Ok(agent_store) = open_agent_store(&cas_dir) {
            if let Ok(agents) = agent_store.list(None) {
                if let Some(agent) = agents.iter().find(|a| a.name == name) {
                    let agent_id = agent.id.clone();
                    if !worker_has_open_tasks(&cas_dir, &agent_id) {
                        self.finalize_worker_worktree(&agent_store, &agent_id, name);
                    }
                }
            }
        }

        // Sync pane sizes to adjust layout
        let _ = self.sync_pane_sizes();

        let workers_remaining = self.worker_names.len().to_string();
        crate::telemetry::track(
            "factory_worker_crashed",
            vec![("workers_remaining", &workers_remaining)],
        );
    }

    /// Respawn a crashed worker
    ///
    /// Re-creates a worker with the same name, reusing its existing worktree if available.
    pub fn respawn_worker(
        &mut self,
        name: &str,
        teams: Option<cas_mux::TeamsSpawnConfig>,
    ) -> anyhow::Result<()> {
        // cas-9bc6: re-read live LlmConfig so harness/model/effort changes made
        // via `cas config set` after daemon boot are reflected in this respawn.
        self.sync_worker_config_from_live_settings();

        // Respawn has no resolver call of its own: it reuses the Mux's
        // effective per-worker/default spec after the live config sync. Keep
        // the same shared validator on this recovery path before any worktree
        // or PTY launch occurs.
        let effective_spec = self.mux.effective_worker_spec(name, None);
        cas_factory::validate_explicit(
            &effective_spec,
            &cas_factory::CapabilitySnapshot::default(),
        )
        .map_err(|error| anyhow::anyhow!("Failed to validate worker routing spec: {error}"))?;

        crate::telemetry::track(
            "factory_worker_respawn_requested",
            vec![("worktrees_enabled", bool_prop(self.worktrees_enabled()))],
        );

        // Check if worker is already active
        if self.worker_names.contains(&name.to_string()) {
            crate::telemetry::track(
                "factory_worker_respawn_result",
                vec![("success", "false"), ("reason", "already_active")],
            );
            anyhow::bail!("Worker '{name}' is already active");
        }

        // Check if worktree exists. If it has to be created, use the same base
        // selection as normal worker spawn.
        let (cwd, cas_root) = if let Some(manager) = &mut self.worktree_manager {
            let worker_base = worker_base_for_spawn(self.epic_branch.as_deref(), manager);
            let worktree = match manager.ensure_worker_worktree_from(name, &worker_base) {
                Ok(worktree) => worktree,
                Err(e) => {
                    crate::telemetry::track(
                        "factory_worker_respawn_result",
                        vec![("success", "false"), ("reason", "ensure_worktree_failed")],
                    );
                    return Err(e.into());
                }
            };
            (worktree.path.clone(), Some(self.cas_dir.clone()))
        } else {
            // No worktrees - use main cwd
            let cwd = std::env::current_dir()?;
            (cwd, None)
        };

        // Add pane to mux (spawns new Claude process)
        if let Err(e) = self.mux.add_worker(
            name,
            cwd,
            cas_root.as_ref(),
            &self.supervisor_name,
            teams.as_ref(),
            None, // spec: use Mux default (T3 will supply per-spawn overrides)
        ) {
            crate::telemetry::track(
                "factory_worker_respawn_result",
                vec![("success", "false"), ("reason", "mux_add_worker_failed")],
            );
            return Err(e.into());
        }
        self.track_worker_process_group(name);

        // Track the worker name
        self.worker_names.push(name.to_string());
        crate::ui::factory::app::queue_codex_worker_intro_prompt(
            self.cas_dir(),
            name,
            self.worker_cli,
        );

        // Update pane grid for navigation
        self.pane_grid = PaneGrid::new(&self.worker_names, &self.supervisor_name, self.is_tabbed);
        self.sync_worker_pane_branch_titles();

        // Sync pane sizes
        let _ = self.sync_pane_sizes();

        let workers_active = self.worker_names.len().to_string();
        crate::telemetry::track(
            "factory_worker_respawn_result",
            vec![("success", "true"), ("workers_active", &workers_active)],
        );

        Ok(())
    }

    /// Shutdown N workers (least recently used first, or by name)
    ///
    /// If count is 0 or None, shuts down all workers.
    ///
    /// # Arguments
    /// * `count` - Number of workers to shutdown (0 or None = all)
    /// * `names` - Specific worker names to shutdown (overrides count)
    /// * `force` - Reserved for compatibility; supervisor should pre-check worktree safety
    pub async fn shutdown_workers(
        &mut self,
        count: Option<usize>,
        names: &[String],
        force: bool,
    ) -> anyhow::Result<usize> {
        let scope = shutdown_scope(count, names);
        let requested = if !names.is_empty() {
            names.len()
        } else {
            count.unwrap_or(0)
        };
        let requested_count = requested.to_string();
        crate::telemetry::track(
            "factory_worker_shutdown_requested",
            vec![
                ("scope", scope),
                ("requested_count", &requested_count),
                ("force", bool_prop(force)),
            ],
        );

        let mut shutdown_count = 0;
        let mut failures = Vec::new();

        if !names.is_empty() {
            // Shutdown specific workers by name
            for name in names {
                if let Err(e) = self.shutdown_worker(name, force).await {
                    failures.push(format!("{name}: {e}"));
                } else {
                    shutdown_count += 1;
                }
            }
        } else {
            // Shutdown by count (0 = all)
            let target = count.unwrap_or(0);
            let workers_to_shutdown: Vec<String> = if target == 0 {
                self.worker_names.clone()
            } else {
                self.worker_names.iter().take(target).cloned().collect()
            };

            for name in workers_to_shutdown {
                if let Err(e) = self.shutdown_worker(&name, force).await {
                    failures.push(format!("{name}: {e}"));
                } else {
                    shutdown_count += 1;
                }
            }
        }

        if !failures.is_empty() {
            let summary = failures.join("; ");
            self.set_error(format!("Shutdown had failures: {summary}"));
            let shutdown_count_str = shutdown_count.to_string();
            let failure_count_str = failures.len().to_string();
            crate::telemetry::track(
                "factory_worker_shutdown_result",
                vec![
                    ("success", "false"),
                    ("scope", scope),
                    ("shutdown_count", &shutdown_count_str),
                    ("failure_count", &failure_count_str),
                ],
            );
            anyhow::bail!("Shutdown had failures: {summary}");
        }

        let shutdown_count_str = shutdown_count.to_string();
        crate::telemetry::track(
            "factory_worker_shutdown_result",
            vec![
                ("success", "true"),
                ("scope", scope),
                ("shutdown_count", &shutdown_count_str),
            ],
        );

        Ok(shutdown_count)
    }
}

#[cfg(test)]
mod spawn_base_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_repo(dir: &std::path::Path) {
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir)
            .output()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(dir)
            .output()
            .unwrap();
        std::fs::write(dir.join("README.md"), "# test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir)
            .output()
            .expect("git commit");
    }

    /// cas-7587 (GH #122) fixtures: two epics with real branches, plus a task
    /// store where a child task hangs off the *second* epic.
    fn commit_file(repo: &std::path::Path, name: &str, body: &str) {
        std::fs::write(repo.join(name), body).unwrap();
        Command::new("git")
            .args(["add", name])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", name])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    fn branch_at(repo: &std::path::Path, branch: &str, start: &str) {
        Command::new("git")
            .args(["branch", branch, start])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    fn add_origin(repo: &std::path::Path, url: &str) {
        Command::new("git")
            .args(["remote", "add", "origin", url])
            .current_dir(repo)
            .output()
            .expect("git remote add origin");
    }

    fn seed_epic(cas_dir: &std::path::Path, epic_id: &str, title: &str, branch: Option<&str>) {
        let store = crate::store::open_task_store(cas_dir).unwrap();
        let mut epic = cas_types::Task::new(epic_id.to_string(), title.to_string());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = branch.map(str::to_string);
        store.add(&epic).unwrap();
    }

    fn seed_child(cas_dir: &std::path::Path, task_id: &str, epic_id: &str) {
        let store = crate::store::open_task_store(cas_dir).unwrap();
        let child = cas_types::Task::new(task_id.to_string(), format!("child {task_id}"));
        store.add(&child).unwrap();
        store
            .add_dependency(&cas_types::Dependency {
                from_id: task_id.to_string(),
                to_id: epic_id.to_string(),
                dep_type: cas_types::DependencyType::ParentChild,
                created_at: chrono::Utc::now(),
                created_by: None,
            })
            .unwrap();
    }

    /// GH #670: a task's durable WorkTarget selector may resolve to a sibling
    /// checkout. The worker worktree must be cut from that checkout, where its
    /// target-only branch exists, rather than from the session repository.
    #[test]
    fn cross_repo_spawn_resolves_target_only_branch_and_worktree_repo_cas_052a() {
        TestEnvGuard::run_with_temp_home(|_| {
            crate::store::known_repos::ensure_host_schema().unwrap();

            let tmp = TempDir::new_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let session_repo = tmp.path().join("session-repo");
            let target_repo = tmp.path().join("target-repo");
            std::fs::create_dir(&session_repo).unwrap();
            std::fs::create_dir(&target_repo).unwrap();
            init_repo(&session_repo);
            init_repo(&target_repo);
            let session_cas = crate::store::init_cas_dir(&session_repo).unwrap();
            let _target_cas = crate::store::init_cas_dir(&target_repo).unwrap();
            add_origin(&session_repo, "https://github.com/example/session.git");
            add_origin(&target_repo, "https://github.com/example/target.git");
            branch_at(&target_repo, "cursor/target-only", "main");
            Command::new("git")
                .args(["checkout", "-q", "cursor/target-only"])
                .current_dir(&target_repo)
                .output()
                .unwrap();
            commit_file(&target_repo, "target-only.txt", "target");
            Command::new("git")
                .args(["checkout", "-q", "main"])
                .current_dir(&target_repo)
                .output()
                .unwrap();
            crate::store::known_repos::register_repo_strict(&target_repo).unwrap();

            let target = SpawnWorkTarget {
                task_id: "cas-target-only".into(),
                repo_selector: "remote:github.com/example/target".into(),
                target_branch: "cursor/target-only".into(),
                owner: WorkTargetOwner::Task,
            };
            let target_root = resolve_spawn_worktree_repo(
                &session_cas,
                &session_repo,
                Some(&target),
            )
            .expect("target selector must resolve in the sibling checkout");
            assert_eq!(target_root, target_repo);

            let worker_path = target_root.join(".cas/worktrees/cross-repo-worker");
            WorkerSpawnPrep {
                worker_name: "cross-repo-worker".into(),
                worktree_info: Some(WorktreePrep {
                    worktree_path: worker_path.clone(),
                    branch_name: "factory/cross-repo-worker".into(),
                    parent_branch: target.target_branch.clone(),
                    base_ref: None,
                    repo_root: target_root,
                    cas_dir: session_cas.clone(),
                }),
                warnings: Vec::new(),
                base_provenance: None,
            }
            .run()
            .expect("target repository should provision the worker worktree");
            assert!(worker_path.join("target-only.txt").is_file());
            assert!(worker_path.join(".git").is_file());
            assert_eq!(
                std::process::Command::new("git")
                    .args(["-C", worker_path.to_str().unwrap(), "branch", "--show-current"])
                    .output()
                    .unwrap()
                    .stdout,
                b"factory/cross-repo-worker\n"
            );

            let mut manager = WorktreeManager::new(
                &session_repo,
                WorktreeConfig {
                    enabled: true,
                    base_path: session_repo
                        .join(".cas/worktrees")
                        .to_string_lossy()
                        .to_string(),
                    branch_prefix: "factory/".into(),
                    auto_merge: false,
                    cleanup_on_close: false,
                    promote_entries_on_merge: false,
                },
            )
            .unwrap();
            manager.register_worktree(
                "cross-repo-worker",
                Worktree::new(
                    Worktree::generate_id(),
                    "factory/cross-repo-worker".into(),
                    target.target_branch,
                    worker_path.clone(),
                ),
            );
            manager
                .remove_worker("cross-repo-worker", false)
                .expect("manager should clean up the target-repository worktree");
            assert!(!worker_path.exists());
        });
    }

    /// GH #670: a target selector that resolves to a real sibling repository
    /// must still fail with an explicit cross-repo message when its branch is
    /// absent there (and the session repo does not have it either).
    #[test]
    fn cross_repo_spawn_missing_target_branch_has_explicit_failure_cas_052a() {
        TestEnvGuard::run_with_temp_home(|_| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let tmp = TempDir::new_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let session_repo = tmp.path().join("session-repo");
            let target_repo = tmp.path().join("target-repo");
            std::fs::create_dir(&session_repo).unwrap();
            std::fs::create_dir(&target_repo).unwrap();
            init_repo(&session_repo);
            init_repo(&target_repo);
            let session_cas = crate::store::init_cas_dir(&session_repo).unwrap();
            let _target_cas = crate::store::init_cas_dir(&target_repo).unwrap();
            add_origin(&target_repo, "https://github.com/example/target-missing.git");
            crate::store::known_repos::register_repo_strict(&target_repo).unwrap();

            let target = SpawnWorkTarget {
                task_id: "cas-target-missing".into(),
                repo_selector: "remote:github.com/example/target-missing".into(),
                target_branch: "cursor/exists-nowhere".into(),
                owner: WorkTargetOwner::Task,
            };
            let error = resolve_spawn_worktree_repo(
                &session_cas,
                &session_repo,
                Some(&target),
            )
            .expect_err("missing target branch must fail before git worktree add");
            let text = error.to_string();
            assert!(
                text.starts_with("cross-repo spawn: target_repo "),
                "{text}"
            );
            assert!(text.contains(target_repo.to_str().unwrap()), "{text}");
            assert!(text.contains("cursor/exists-nowhere"), "{text}");
            assert!(!text.contains("Refusing to create worktree branch"), "{text}");
        });
    }

    #[test]
    fn cross_repo_spawn_receipt_names_target_repository_cas_052a() {
        let target_repo = std::path::Path::new("/workspace/target-repo");
        let prep = WorkerSpawnPrep {
            worker_name: "receipt-worker".into(),
            worktree_info: Some(WorktreePrep {
                worktree_path: target_repo.join(".cas/worktrees/receipt-worker"),
                branch_name: "factory/receipt-worker".into(),
                parent_branch: "main".into(),
                base_ref: None,
                repo_root: target_repo.to_path_buf(),
                cas_dir: std::path::PathBuf::from("/workspace/session/.cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };
        let receipt = spawn_provision_receipt(&prep);
        assert!(receipt.contains("Worktree repository: /workspace/target-repo"), "{receipt}");
    }

    /// GH #122 repro, end to end: focus pinned to epic A, spawn requested with
    /// a task belonging to epic B. Pre-fix the worktree was cut from epic A's
    /// branch; it must now be cut from epic B's, and the resulting worktree
    /// must actually contain epic B's commit.
    #[test]
    fn spawn_with_task_id_cuts_from_the_tasks_epic_not_the_pinned_focus_cas_7587() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();

        // epic A: focused, and deliberately behind — the wrong base.
        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/alpha"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit_file(&repo, "alpha-only.txt", "alpha");
        // epic B: owns the task being spawned for.
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/bravo"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit_file(&repo, "bravo-only.txt", "bravo");
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        seed_epic(&cas_dir, "cas-alpha", "Alpha epic", Some("epic/alpha"));
        seed_epic(&cas_dir, "cas-bravo", "Bravo epic", Some("epic/bravo"));
        seed_child(&cas_dir, "cas-child", "cas-bravo");

        let task_base = task_epic_base(&cas_dir, &repo, "cas-child");
        let task_epic = task_base
            .epic()
            .expect("child task must resolve to its parent epic");
        assert_eq!(task_epic.epic_id, "cas-bravo");
        assert_eq!(task_epic.branch, "epic/bravo");
        assert!(task_epic.branch_exists);

        let (base, source) = resolve_spawn_base(&task_base, Some("epic/alpha"), "main");
        assert_eq!(
            base, "epic/bravo",
            "GH #122: the task's epic branch must win over the pinned focus"
        );
        assert_eq!(
            source,
            SpawnBaseSource::TaskEpic {
                task_id: "cas-child".to_string(),
                epic_id: "cas-bravo".to_string(),
            }
        );

        // The mismatch must be named explicitly in spawn output.
        assert!(base_diverges_from_focus(&base, &source, Some("epic/alpha")));
        let provenance = spawn_base_provenance_notice(&base, &source, Some("epic/alpha"));
        assert!(provenance.contains("epic/bravo"), "{provenance}");
        assert!(provenance.contains("cas-bravo"), "{provenance}");
        assert!(provenance.contains("cas-child"), "{provenance}");
        assert!(provenance.contains("epic/alpha"), "{provenance}");
        assert!(provenance.contains("differs"), "{provenance}");

        // And the worktree really lands on epic B's history.
        let worktree_root = repo.join(".cas").join("worktrees");
        let worker_path = worktree_root.join("bravo-worker");
        WorkerSpawnPrep {
            worker_name: "bravo-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/bravo-worker".to_string(),
                parent_branch: base,
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir: cas_dir.clone(),
            }),
            warnings: Vec::new(),
            base_provenance: Some(provenance),
        }
        .run()
        .unwrap();
        assert!(
            worker_path.join("bravo-only.txt").is_file(),
            "worker must start with its own epic's commits"
        );
        assert!(
            !worker_path.join("alpha-only.txt").exists(),
            "worker must NOT be cut from the unrelated focused epic (GH #122)"
        );
    }

    #[test]
    fn task_epic_base_exposes_declared_target_branch_cas_3afc() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "staging", "main");
        branch_at(&repo, "epic/staging-target", "staging");

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-staging".into(), "Staging epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/staging-target".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "staging".into(),
        });
        store.add(&epic).unwrap();

        let base = task_epic_base(&cas_dir, &repo, "cas-staging");
        assert_eq!(base.target_branch(), Some("staging"));
        assert_eq!(
            resolve_spawn_base(&base, None, base.target_branch().unwrap()).0,
            "epic/staging-target",
            "a live epic branch must retain its child integration history"
        );
    }

    /// GH #625: a legacy child may already have the repository default in its
    /// WorkTarget even though that value merely repeated the parent epic's
    /// default. Spawn must treat it as implicit epic scope and cut from the
    /// live integration branch; a distinct task lane remains authoritative.
    #[test]
    fn child_default_work_target_uses_live_epic_branch_cas_d22d() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "epic/live-delivery", "main");

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-d22d-epic".into(), "delivery epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/live-delivery".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "main".into(),
        });
        store.add(&epic).unwrap();

        let mut child = cas_types::Task::new("cas-d22d-child".into(), "default child".into());
        child.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "main".into(),
        });
        store.add(&child).unwrap();
        store
            .add_dependency(&cas_types::Dependency {
                from_id: child.id.clone(),
                to_id: epic.id.clone(),
                dep_type: cas_types::DependencyType::ParentChild,
                created_at: chrono::Utc::now(),
                created_by: None,
            })
            .unwrap();

        let task_base = task_epic_base(&cas_dir, &repo, &child.id);
        assert!(
            task_base
                .work_target()
                .is_some_and(|target| matches!(target.owner, WorkTargetOwner::Epic { .. })),
            "the duplicate default target must not remain task authority"
        );
        let (base, source) = resolve_spawn_base(&task_base, Some("epic/unrelated"), "main");
        assert_eq!(base, "epic/live-delivery");
        assert!(matches!(source, SpawnBaseSource::TaskEpic { .. }));
        let receipt = spawn_base_provenance_notice(&base, &source, Some("epic/unrelated"));
        assert!(receipt.contains("epic/live-delivery"), "{receipt}");
        assert!(receipt.contains("cas-d22d-epic"), "{receipt}");
    }

    #[test]
    fn child_explicit_work_target_still_wins_over_live_epic_branch_cas_d22d() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "epic/live-delivery", "main");
        branch_at(&repo, "release/operator-selected", "main");

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic =
            cas_types::Task::new("cas-d22d-explicit-epic".into(), "delivery epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/live-delivery".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "main".into(),
        });
        store.add(&epic).unwrap();

        let mut child = cas_types::Task::new(
            "cas-d22d-explicit-child".into(),
            "explicit child lane".into(),
        );
        child.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "release/operator-selected".into(),
        });
        store.add(&child).unwrap();
        store
            .add_dependency(&cas_types::Dependency {
                from_id: child.id.clone(),
                to_id: epic.id,
                dep_type: cas_types::DependencyType::ParentChild,
                created_at: chrono::Utc::now(),
                created_by: None,
            })
            .unwrap();

        let task_base = task_epic_base(&cas_dir, &repo, &child.id);
        let (base, source) = resolve_spawn_base(&task_base, Some("epic/unrelated"), "main");
        assert_eq!(base, "release/operator-selected");
        assert!(matches!(
            source,
            SpawnBaseSource::WorkTarget {
                owner: WorkTargetOwner::Task,
                ..
            }
        ));
    }

    #[test]
    fn epic_work_target_bases_spawn_on_declared_branch_when_epic_branch_is_missing_cas_7a87() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "staging", "main");

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-missing-epic".into(), "missing branch".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/not-created".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "staging".into(),
        });
        store.add(&epic).unwrap();

        let base = task_epic_base(&cas_dir, &repo, "cas-missing-epic");
        assert!(!base.epic().unwrap().branch_exists);
        let (branch, source) = resolve_spawn_base(&base, Some("epic/unrelated"), "main");
        assert_eq!(branch, "staging");
        assert!(matches!(
            source,
            SpawnBaseSource::WorkTarget {
                owner: WorkTargetOwner::Epic { .. },
                ..
            }
        ));
    }

    /// GH #433: an older title-derived branch may still exist even though the
    /// epic was subsequently given the authoritative WorkTarget that MCP can
    /// maintain. The slug must be a fallback, never a competing live lane.
    #[test]
    fn epic_work_target_beats_an_existing_legacy_title_slug_cas_0f97() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let legacy_slug = "epic/shockwave-migrate-ai-stack-home-onto-the-new-4tb-e";
        let declared_target = "epic/shockwave-4tb-migration";
        branch_at(&repo, legacy_slug, "main");
        branch_at(&repo, declared_target, "main");

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new(
            "cas-shockwave".into(),
            "Shockwave migrate AI stack home onto the new 4TB E".into(),
        );
        epic.task_type = cas_types::TaskType::Epic;
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: declared_target.into(),
        });
        store.add(&epic).unwrap();
        seed_child(&cas_dir, "cas-shockwave-child", "cas-shockwave");

        let task_base = task_epic_base(&cas_dir, &repo, "cas-shockwave-child");
        let task_epic = task_base.epic().unwrap();
        assert_eq!(task_epic.branch, legacy_slug);
        assert!(task_epic.branch_exists);
        assert!(task_epic.branch_is_title_slug_fallback);

        let (base, source) = resolve_spawn_base(&task_base, Some("epic/unrelated"), "main");
        assert_eq!(base, declared_target, "the declared WorkTarget must win");
        assert!(matches!(
            source,
            SpawnBaseSource::WorkTarget {
                owner: WorkTargetOwner::Epic { .. },
                ..
            }
        ));
        let warning = stale_legacy_slug_notice(task_base.epic(), &base, &source)
            .expect("the stale legacy slug must be surfaced");
        assert!(warning.contains(legacy_slug), "{warning}");
        assert!(warning.contains(declared_target), "{warning}");
    }

    #[test]
    fn taskless_spawn_keeps_pinned_focus_then_trunk_cas_7587() {
        assert_eq!(
            resolve_spawn_base(&TaskBase::Unresolved, Some("epic/alpha"), "main"),
            ("epic/alpha".to_string(), SpawnBaseSource::PinnedFocus),
            "taskless spawns must keep the pre-fix focus-based behavior"
        );
        assert_eq!(
            resolve_spawn_base(&TaskBase::Unresolved, None, "main"),
            ("main".to_string(), SpawnBaseSource::Trunk)
        );
        assert!(!base_diverges_from_focus(
            "epic/alpha",
            &SpawnBaseSource::PinnedFocus,
            Some("epic/alpha")
        ));
    }

    #[test]
    fn task_epic_without_a_branch_on_disk_falls_back_to_focus_cas_7587() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "epic/alpha", "main");

        seed_epic(
            &cas_dir,
            "cas-ghost",
            "Ghost epic",
            Some("epic/never-created"),
        );
        seed_child(&cas_dir, "cas-orphan", "cas-ghost");

        let task_base = task_epic_base(&cas_dir, &repo, "cas-orphan");
        let task_epic = task_base.epic().unwrap();
        assert_eq!(task_epic.branch, "epic/never-created");
        assert!(
            !task_epic.branch_exists,
            "a branch that does not exist must be reported as such, not silently used"
        );
        assert_eq!(
            resolve_spawn_base(&task_base, Some("epic/alpha"), "main"),
            ("epic/alpha".to_string(), SpawnBaseSource::PinnedFocus),
            "an unresolvable task epic branch keeps the focus base rather than failing the spawn"
        );
    }

    #[test]
    fn spawning_for_an_epic_task_uses_that_epics_own_branch_cas_7587() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "epic/bravo", "main");

        seed_epic(&cas_dir, "cas-bravo", "Bravo epic", Some("epic/bravo"));

        let task_base = task_epic_base(&cas_dir, &repo, "cas-bravo");
        let task_epic = task_base.epic().unwrap();
        assert_eq!(task_epic.epic_id, "cas-bravo");
        assert_eq!(task_epic.branch, "epic/bravo");
        assert!(task_epic.branch_exists);
        assert_eq!(
            resolve_spawn_base(&task_base, Some("epic/alpha"), "main").0,
            "epic/bravo"
        );
    }

    #[test]
    fn legacy_epic_without_persisted_branch_falls_back_to_title_slug_cas_7587() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "epic/legacy-burn-down", "main");

        seed_epic(&cas_dir, "cas-legacy", "Legacy burn down", None);
        seed_child(&cas_dir, "cas-legacy-child", "cas-legacy");

        let task_base = task_epic_base(&cas_dir, &repo, "cas-legacy-child");
        let task_epic = task_base.epic().unwrap();
        assert_eq!(
            task_epic.branch, "epic/legacy-burn-down",
            "legacy epics without a persisted branch keep the title-derived name"
        );
        assert!(task_epic.branch_exists);
    }

    #[test]
    fn unknown_task_id_never_breaks_the_spawn_cas_7587() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();

        let task_base = task_epic_base(&cas_dir, &repo, "cas-does-not-exist");
        assert_eq!(
            task_base,
            TaskBase::Unresolved,
            "an unknown task must stay Unresolved — only a *known* task with no epic \
             may redirect the base to trunk (cas-d897)"
        );
        assert_eq!(
            resolve_spawn_base(&task_base, Some("epic/alpha"), "main"),
            ("epic/alpha".to_string(), SpawnBaseSource::PinnedFocus)
        );
    }

    /// cas-d897 (GH #146) repro: focus pinned to an epic branch, spawn requested
    /// for a task that belongs to NO epic. Pre-fix the worktree was cut from the
    /// pinned focus (71 commits behind trunk in the reported incident); it must
    /// now be cut from trunk, and the override must be stated out loud.
    #[test]
    fn epic_less_task_bases_on_trunk_not_the_pinned_focus_cas_d897() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();

        // A pinned focus epic that is deliberately behind main.
        branch_at(&repo, "epic/alpha", "main");
        commit_file(&repo, "main-only.txt", "main moved on");

        // A standalone task with no parent epic at all.
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&cas_types::Task::new(
                "cas-loner".to_string(),
                "standalone task".to_string(),
            ))
            .unwrap();

        let task_base = task_epic_base(&cas_dir, &repo, "cas-loner");
        assert_eq!(
            task_base,
            TaskBase::NoEpic {
                task_id: "cas-loner".to_string(),
                work_target: None,
            },
            "a task with no parent epic must be reported as such, not as 'could not resolve'"
        );

        let (base, source) = resolve_spawn_base(&task_base, Some("epic/alpha"), "main");
        assert_eq!(
            base, "main",
            "GH #146: an epic-less task must base on trunk, never the pinned focus"
        );
        assert_eq!(
            source,
            SpawnBaseSource::TaskWithoutEpic {
                task_id: "cas-loner".to_string()
            }
        );

        // The override of the operator's focus must be surfaced, not silent.
        assert!(base_diverges_from_focus(&base, &source, Some("epic/alpha")));
        let provenance = spawn_base_provenance_notice(&base, &source, Some("epic/alpha"));
        assert!(provenance.contains("cas-loner"), "{provenance}");
        assert!(provenance.contains("epic/alpha"), "{provenance}");
        assert!(provenance.contains("no epic"), "{provenance}");

        // And the worktree really lands on trunk's history, not the stale focus.
        let worker_path = repo.join(".cas").join("worktrees").join("loner-worker");
        WorkerSpawnPrep {
            worker_name: "loner-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/loner-worker".to_string(),
                parent_branch: base,
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir: cas_dir.clone(),
            }),
            warnings: Vec::new(),
            base_provenance: Some(provenance),
        }
        .run()
        .unwrap();
        assert!(
            worker_path.join("main-only.txt").is_file(),
            "epic-less worker must start from trunk's tip (GH #146)"
        );
    }

    /// Cassy #413: an explicit task WorkTarget is the same delivery authority
    /// that `worktree_merge` uses. A worker must therefore start on that
    /// branch, not on ambient trunk or the supervisor's focused epic.
    #[test]
    fn standalone_task_work_target_bases_spawn_on_declared_branch_cas_7a87() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        branch_at(&repo, "staging", "main");

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut task = cas_types::Task::new("cas-targeted".into(), "staging fix".into());
        task.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "staging".into(),
        });
        store.add(&task).unwrap();

        let task_base = task_epic_base(&cas_dir, &repo, "cas-targeted");
        let (base, source) = resolve_spawn_base(&task_base, Some("epic/unrelated"), "main");
        assert_eq!(
            base, "staging",
            "Cassy #413: a task WorkTarget must outrank trunk and pinned focus"
        );
        let receipt = spawn_base_provenance_notice(&base, &source, Some("epic/unrelated"));
        assert!(receipt.contains("WorkTarget"), "{receipt}");
        assert!(receipt.contains("staging"), "{receipt}");
    }

    /// cas-d897 (GH #146) part (b): the chosen base's local ref was stale while
    /// `origin/`'s copy of the same branch was ahead. The spawn must cut from
    /// the fresher commit and name both SHAs.
    #[test]
    fn stale_local_base_ref_loses_to_a_fresher_origin_ref_cas_d897() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        commit_file(&origin, "remote-only.txt", "landed on origin");

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        // Rewind the LOCAL main to the first commit; origin/main keeps the tip.
        Command::new("git")
            .args(["reset", "--hard", "-q", "HEAD~1"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let local_sha = short_sha(&repo, "main");
        let remote_sha = short_sha(&repo, "origin/main");
        assert_ne!(local_sha, remote_sha, "fixture must be stale-local");

        let (base_ref, notice) = prefer_fresher_base_ref(&repo, "main");
        let base_ref = base_ref.expect("origin's fresher tip must win over the stale local ref");
        assert_eq!(
            short_sha(&repo, &base_ref),
            remote_sha,
            "the worktree must be cut from origin's tip, not the stale local ref"
        );
        let notice = notice.expect("a base swapped for origin's tip must be reported");
        assert!(notice.contains(&local_sha), "{notice}");
        assert!(notice.contains(&remote_sha), "{notice}");
        assert!(notice.contains("origin/main"), "{notice}");
        assert_eq!(
            stale_spawn_base_notice(&repo, &base_ref, "main"),
            None,
            "the stale-base warning must inspect the effective origin checkout, not the old local parent"
        );

        // End to end: the worker's files come from origin's tip while the
        // recorded parent branch stays the local, mergeable branch name.
        let worker_path = repo.join(".cas").join("worktrees").join("fresh-worker");
        WorkerSpawnPrep {
            worker_name: "fresh-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/fresh-worker".to_string(),
                parent_branch: "main".to_string(),
                base_ref: Some(base_ref),
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: vec![notice],
            base_provenance: None,
        }
        .run()
        .unwrap();
        assert!(
            worker_path.join("remote-only.txt").is_file(),
            "GH #146: the worker must contain the commit that only origin's ref had"
        );
    }

    /// Cassy #413: WorkTarget spawns must use the freshly fetched remote tip
    /// even when the matching local branch exists but is stale. This is the
    /// regression shape that otherwise starts a worker before the bug exists.
    #[test]
    fn work_target_checkout_uses_fetched_origin_tip_when_local_branch_is_stale_cas_7a87() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        Command::new("git")
            .args(["checkout", "-q", "-b", "staging"])
            .current_dir(&origin)
            .output()
            .unwrap();
        commit_file(&origin, "regression.txt", "only on staging");
        let remote_sha = short_sha(&origin, "staging");
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&origin)
            .output()
            .unwrap();

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["branch", "staging", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let local_sha = short_sha(&repo, "staging");
        assert_ne!(
            local_sha, remote_sha,
            "fixture must have a stale local staging ref"
        );

        let source = SpawnBaseSource::WorkTarget {
            task_id: "cas-targeted".into(),
            owner: WorkTargetOwner::Task,
        };
        let (base_ref, notice, checkout_ref) =
            checkout_ref_for_spawn_base(&repo, "staging", &source);
        let base_ref = base_ref.expect("WorkTarget must check out an immutable fetched tip");
        assert_eq!(checkout_ref, "origin/staging");
        assert_eq!(short_sha(&repo, &base_ref), remote_sha);
        assert!(
            notice
                .expect("stale local branch must be disclosed")
                .contains(&local_sha)
        );

        let worker_path = repo.join(".cas/worktrees/targeted-worker");
        WorkerSpawnPrep {
            worker_name: "targeted-worker".into(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/targeted-worker".into(),
                parent_branch: "staging".into(),
                base_ref: Some(base_ref),
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .expect("worker worktree should cut from fetched WorkTarget");
        assert!(
            worker_path.join("regression.txt").exists(),
            "the worker must include the target-only regression"
        );
    }

    /// A local ref that is *ahead* of (or level with) origin is not stale:
    /// no override, no noise.
    #[test]
    fn base_ahead_of_origin_is_left_alone_cas_d897() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(&repo)
            .output()
            .unwrap();

        assert_eq!(
            prefer_fresher_base_ref(&repo, "main"),
            (None, None),
            "identical local/remote tips must not manufacture a swap or a warning"
        );

        commit_file(&repo, "local-only.txt", "not pushed yet");
        assert_eq!(
            prefer_fresher_base_ref(&repo, "main"),
            (None, None),
            "a local ref ahead of origin is the fresher one and must be kept"
        );
    }

    /// Diverged local/remote: keep the local ref (the remote is not
    /// automatically right) but say so, naming both SHAs.
    #[test]
    fn diverged_base_keeps_local_and_warns_with_both_shas_cas_d897() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        commit_file(&origin, "remote-only.txt", "origin side");

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["reset", "--hard", "-q", "HEAD~1"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit_file(&repo, "local-only.txt", "local side");

        let local_sha = short_sha(&repo, "main");
        let remote_sha = short_sha(&repo, "origin/main");
        let (base_ref, notice) = prefer_fresher_base_ref(&repo, "main");
        assert_eq!(
            base_ref, None,
            "a diverged remote must not silently replace the local base"
        );
        let notice = notice.expect("divergence must be reported");
        assert!(notice.contains(&local_sha), "{notice}");
        assert!(notice.contains(&remote_sha), "{notice}");
        assert!(notice.contains("DIVERGED"), "{notice}");
    }

    /// No `origin/<base>` at all (fresh local-only repo): nothing to compare,
    /// so nothing to change and nothing to warn about.
    #[test]
    fn missing_origin_counterpart_is_not_staleness_cas_d897() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        assert_eq!(prefer_fresher_base_ref(&repo, "main"), (None, None));
    }

    #[test]
    fn dynamic_worker_spawn_base_uses_epic_or_trunk_not_current_head() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        Command::new("git")
            .args(["branch", "epic/current", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["checkout", "-q", "-b", "feature/supervisor-head"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let config = WorktreeConfig {
            enabled: true,
            base_path: repo
                .join(".cas")
                .join("worktrees")
                .to_string_lossy()
                .to_string(),
            branch_prefix: "factory/".to_string(),
            auto_merge: false,
            cleanup_on_close: false,
            promote_entries_on_merge: false,
        };
        let manager = WorktreeManager::new(&repo, config).unwrap();
        assert_eq!(
            manager.git().current_branch().unwrap(),
            "feature/supervisor-head"
        );

        assert_eq!(
            worker_base_for_spawn(Some("epic/current"), &manager),
            "epic/current",
            "dynamic isolated workers should branch from the active epic branch"
        );
        assert_eq!(
            worker_base_for_spawn(None, &manager),
            "main",
            "without an active epic, dynamic isolated workers should branch from trunk, not supervisor HEAD"
        );
    }

    #[test]
    fn daemon_started_before_nested_git_init_rechecks_repo_context_per_spawn() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().canonicalize().unwrap().join("parent");
        std::fs::create_dir(&parent).unwrap();
        init_repo(&parent);
        let project = parent.join("new-project");
        std::fs::create_dir(&project).unwrap();

        let manager = WorktreeManager::new(
            &project,
            WorktreeConfig {
                enabled: true,
                base_path: project.join(".cas/worktrees").to_string_lossy().to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            },
        )
        .unwrap();
        assert_eq!(manager.repo_root(), parent.as_path());

        // Simulate `git init` after daemon construction. The next spawn must
        // not silently keep using the ancestor root cached at startup.
        init_repo(&project);
        let error = validate_live_spawn_repo_context(&manager, &project)
            .expect_err("changed repository context must fail this spawn loudly");
        assert!(error.to_string().contains("Repository context changed"));
        assert!(error.to_string().contains("Restart the factory daemon"));
    }

    #[test]
    fn isolated_worker_spawn_contains_non_trunk_epic_tip() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/stacked"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::fs::write(repo.join("stacked.txt"), "required epic content").unwrap();
        Command::new("git")
            .args(["add", "stacked.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "stacked epic tip"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let epic_tip = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let epic_tip = String::from_utf8(epic_tip.stdout).unwrap();
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let worktree_root = repo.join(".cas").join("worktrees");
        let manager = WorktreeManager::new(
            &repo,
            WorktreeConfig {
                enabled: true,
                base_path: worktree_root.to_string_lossy().to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            },
        )
        .unwrap();
        let worker_base = worker_base_for_spawn(Some("epic/stacked"), &manager);
        assert!(worker_base_mismatch_notice(manager.repo_root(), &worker_base, "epic/stacked").is_none());

        let worker_path = worktree_root.join("stacked-worker");
        let result = WorkerSpawnPrep {
            worker_name: "stacked-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/stacked-worker".to_string(),
                parent_branch: worker_base,
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .unwrap();

        let contains_epic_tip = Command::new("git")
            .args(["merge-base", "--is-ancestor", epic_tip.trim(), "HEAD"])
            .current_dir(result.cwd)
            .status()
            .unwrap();
        assert!(contains_epic_tip.success());
        assert!(worker_path.join("stacked.txt").is_file());
    }

    #[test]
    fn worker_base_mismatch_is_loud_when_trunk_misses_epic_tip() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/ahead"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::fs::write(repo.join("epic-only.txt"), "epic").unwrap();
        Command::new("git")
            .args(["add", "epic-only.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "epic only"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let manager = WorktreeManager::new(
            &repo,
            WorktreeConfig {
                enabled: true,
                base_path: repo.join(".cas/worktrees").to_string_lossy().to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            },
        )
        .unwrap();
        let notice = worker_base_mismatch_notice(manager.repo_root(), "main", "epic/ahead").unwrap();
        assert!(notice.contains("WORKER BASE MISMATCH"));
        assert!(notice.contains("does not contain"));
        assert!(notice.contains("epic/ahead"));
    }

    #[test]
    fn cancelled_spawn_removes_the_worktree_it_created() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let config = WorktreeConfig {
            enabled: true,
            base_path: repo
                .join(".cas")
                .join("worktrees")
                .to_string_lossy()
                .to_string(),
            branch_prefix: "factory/".to_string(),
            auto_merge: false,
            cleanup_on_close: false,
            promote_entries_on_merge: false,
        };
        let mut manager = WorktreeManager::new(&repo, config).unwrap();
        let worktree = manager.create_for_worker("cancelled-worker").unwrap();
        let worktree_path = worktree.path.clone();
        let branch = worktree.branch.clone();
        let mut result = WorkerSpawnResult {
            worker_name: "cancelled-worker".to_string(),
            cwd: worktree_path.clone(),
            cas_root: None,
            worktree: Some(worktree),
            worktree_created: true,
        };

        assert!(
            cleanup_cancelled_spawn_worktree_with_manager(Some(&mut manager), &mut result).unwrap()
        );
        assert!(
            !worktree_path.exists(),
            "discarded spawn must not leak its newly-created worktree"
        );
        assert!(
            !manager.git().branch_exists(&branch).unwrap(),
            "discarded spawn must not leak its worker branch"
        );
    }

    #[test]
    fn cancelled_spawn_preserves_a_reused_worktree() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let mut manager = WorktreeManager::new(
            &repo,
            WorktreeConfig {
                enabled: true,
                base_path: repo
                    .join(".cas")
                    .join("worktrees")
                    .to_string_lossy()
                    .to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            },
        )
        .unwrap();
        let worktree = manager.create_for_worker("reused-worker").unwrap();
        let worktree_path = worktree.path.clone();
        let branch = worktree.branch.clone();
        let mut result = WorkerSpawnResult {
            worker_name: "reused-worker".to_string(),
            cwd: worktree_path.clone(),
            cas_root: None,
            worktree: Some(worktree),
            worktree_created: false,
        };

        assert!(
            !cleanup_cancelled_spawn_worktree_with_manager(Some(&mut manager), &mut result)
                .unwrap()
        );
        assert!(
            result.worktree.is_some(),
            "the reused worktree receipt must remain available to the caller"
        );
        assert!(
            worktree_path.exists(),
            "cancelling a spawn must preserve a reused worker's directory"
        );
        assert!(
            manager.git().branch_exists(&branch).unwrap(),
            "cancelling a spawn must preserve a reused worker's branch"
        );
    }

    #[test]
    fn cancelled_spawn_without_a_worktree_receipt_is_a_no_op() {
        let mut result = WorkerSpawnResult {
            worker_name: "receiptless-worker".to_string(),
            cwd: std::path::PathBuf::from("/unused"),
            cas_root: None,
            worktree: None,
            worktree_created: true,
        };

        assert!(
            !cleanup_cancelled_spawn_worktree_with_manager(None, &mut result).unwrap(),
            "a missing worktree receipt leaves nothing to clean up"
        );
    }

    #[test]
    fn cancelled_created_worktree_without_manager_returns_an_error() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let mut manager = WorktreeManager::new(
            &repo,
            WorktreeConfig {
                enabled: true,
                base_path: repo
                    .join(".cas")
                    .join("worktrees")
                    .to_string_lossy()
                    .to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            },
        )
        .unwrap();
        let worktree = manager.create_for_worker("managerless-worker").unwrap();
        let worktree_path = worktree.path.clone();
        let mut result = WorkerSpawnResult {
            worker_name: "managerless-worker".to_string(),
            cwd: worktree_path.clone(),
            cas_root: None,
            worktree: Some(worktree),
            worktree_created: true,
        };

        let error = cleanup_cancelled_spawn_worktree_with_manager(None, &mut result)
            .expect_err("a created worktree requires a manager for cleanup");
        assert!(error.to_string().contains("no worktree manager"));
        assert!(
            worktree_path.exists(),
            "the error path must not delete the worktree behind the caller's back"
        );
    }

    // -----------------------------------------------------------------------
    // cas-ecf7 (GH #118): stale spawn base detection
    // -----------------------------------------------------------------------

    fn commit(repo: &std::path::Path, file: &str, message: &str) {
        std::fs::write(repo.join(file), message).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    fn head_sha(repo: &std::path::Path, reference: &str) -> String {
        let out = Command::new("git")
            .args(["rev-parse", reference])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The reported incident, reproduced end to end on the provisioning path:
    /// a focused epic branch cut from trunk before a release merge, trunk then
    /// advanced, and three spawns produced worktrees 25 commits in the past
    /// with nothing said about it.
    #[test]
    fn spawn_base_behind_trunk_is_reported_at_provisioning_time() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        // Epic branch cut here...
        Command::new("git")
            .args(["branch", "epic/burn-down", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        // ...trunk then moves on (release merge + version bump).
        commit(&repo, "release.txt", "release v2.44.0");
        commit(&repo, "bump.txt", "version bump");

        let notice = stale_spawn_base_notice(&repo, "epic/burn-down", "main")
            .expect("a base two commits behind trunk must produce a notice");
        assert!(
            notice.contains("STALE WORKER BASE"),
            "notice must be self-labelling: {notice}"
        );
        assert!(
            notice.contains("epic/burn-down"),
            "notice must name the stale base: {notice}"
        );
        assert!(
            notice.contains("target tree differs from 'main'"),
            "notice must name the target-side content gap: {notice}"
        );
        assert!(
            !notice.contains("force=true"),
            "a spawn-time warning must not recommend destructively syncing workers it just made: {notice}"
        );
    }

    #[test]
    fn cas_83f6_epic_base_refresh_uses_fresher_remote_parent_before_cutting_worker() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        Command::new("git")
            .args(["branch", "epic/behind", "main"])
            .current_dir(&origin)
            .output()
            .unwrap();

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["branch", "epic/behind", "origin/epic/behind"])
            .current_dir(&repo)
            .output()
            .unwrap();

        commit(&origin, "parent-advance.txt", "parent advanced upstream");
        std::fs::create_dir_all(origin.join(".claude/workflows")).unwrap();
        commit(
            &origin,
            ".claude/workflows/current-support-playbook.md",
            "current support playbook",
        );
        let parent_tip = head_sha(&origin, "main");

        let notice = fast_forward_epic_base_from_parent(&repo, "epic/behind", "main")
            .expect("clean ancestry must be safe to fast-forward")
            .expect("the stale epic base must be refreshed");
        assert!(notice.contains("EPIC BASE FAST-FORWARDED"), "{notice}");
        assert!(
            notice.contains("via 'origin/main'"),
            "the fetched parent must be the refresh source: {notice}"
        );
        assert!(notice.contains("pushed to origin"), "{notice}");
        assert_eq!(head_sha(&repo, "epic/behind"), parent_tip);
        assert_eq!(
            head_sha(&origin, "epic/behind"),
            parent_tip,
            "the refreshed epic ref must be published before worker provisioning"
        );
        assert_eq!(
            stale_spawn_base_notice(&repo, "epic/behind", "main"),
            None,
            "a refreshed epic base must no longer emit the stale-base warning"
        );

        let worker_path = repo.join(".cas/worktrees/refreshed-worker");
        WorkerSpawnPrep {
            worker_name: "refreshed-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/refreshed-worker".to_string(),
                parent_branch: "epic/behind".to_string(),
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .expect("worker provisioning after the refresh");
        assert!(
            worker_path.join("parent-advance.txt").exists(),
            "the spawned worktree must include the parent branch advance"
        );
        assert_eq!(
            std::fs::read_to_string(
                worker_path.join(".claude/workflows/current-support-playbook.md"),
            )
            .unwrap(),
            "current support playbook",
            "a no-code worker must receive the current playbook from the refreshed parent"
        );
    }

    #[test]
    fn epic_base_refresh_without_origin_keeps_local_tip_and_cuts_worker_from_it() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        Command::new("git")
            .args(["branch", "epic/behind", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        commit(&repo, "parent-advance.txt", "parent advanced locally");
        let parent_tip = head_sha(&repo, "main");

        let notice = fast_forward_epic_base_from_parent(&repo, "epic/behind", "main")
            .expect("a remote-less checkout can still refresh its local epic base")
            .expect("the local refresh must be reported");
        assert!(notice.contains("LOCAL-ONLY"), "{notice}");
        assert!(notice.contains("epic/behind"), "{notice}");
        assert!(notice.contains("main"), "{notice}");
        assert!(notice.contains("unpublished"), "{notice}");
        assert_eq!(head_sha(&repo, "epic/behind"), parent_tip);

        let worker_path = repo.join(".cas/worktrees/local-only-refreshed-worker");
        WorkerSpawnPrep {
            worker_name: "local-only-refreshed-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/local-only-refreshed-worker".to_string(),
                parent_branch: "epic/behind".to_string(),
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .expect("worker provisioning must use the retained local refresh");
        assert!(
            worker_path.join("parent-advance.txt").exists(),
            "the worker must be cut from the refreshed local tip, not the stale base"
        );
    }

    #[test]
    fn work_target_spawn_uses_unpublished_refreshed_epic_tip_after_push_rejection_cas_5504() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        Command::new("git")
            .args(["branch", "epic/behind", "main"])
            .current_dir(&origin)
            .output()
            .unwrap();
        let reject_hook = origin.join(".git/hooks/pre-receive");
        std::fs::write(
            &reject_hook,
            "#!/bin/sh\necho push rejected by test hook >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&reject_hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&reject_hook, permissions).unwrap();

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["branch", "epic/behind", "origin/epic/behind"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let remote_epic_tip = head_sha(&origin, "epic/behind");
        commit(&origin, "parent-advance.txt", "parent advanced upstream");
        let parent_tip = head_sha(&origin, "main");

        let notice = fast_forward_epic_base_from_parent(&repo, "epic/behind", "main")
            .expect("a rejected publish must retain the safe local refresh")
            .expect("the unpublished refresh must be reported");
        assert!(notice.contains("UNPUBLISHED"), "{notice}");
        assert!(notice.contains("epic/behind"), "{notice}");
        assert!(notice.contains("main"), "{notice}");
        assert!(notice.contains("push rejected by test hook"), "{notice}");
        assert_eq!(head_sha(&repo, "epic/behind"), parent_tip);
        assert_eq!(
            head_sha(&origin, "epic/behind"),
            remote_epic_tip,
            "a rejected publication must leave only the remote ref stale"
        );

        let source = SpawnBaseSource::WorkTarget {
            task_id: "cas-5504".into(),
            owner: WorkTargetOwner::Task,
        };
        let (base_ref, freshness_notice, checkout_ref) =
            checkout_ref_for_spawn_base(&repo, "epic/behind", &source);
        let base_ref = base_ref.expect("WorkTarget spawn must pin its checkout commit");
        assert_eq!(
            base_ref, parent_tip,
            "the fresh unpublished local tip must win"
        );
        assert_eq!(checkout_ref, "epic/behind", "origin/epic/behind is stale");
        assert!(
            freshness_notice.is_none(),
            "the preceding unpublished-refresh notice is the applicable disclosure: {freshness_notice:?}"
        );

        let worker_path = repo.join(".cas/worktrees/push-rejected-refreshed-worker");
        WorkerSpawnPrep {
            worker_name: "push-rejected-refreshed-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/push-rejected-refreshed-worker".to_string(),
                parent_branch: "epic/behind".to_string(),
                base_ref: Some(base_ref),
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .expect("worker provisioning must use the retained local refresh");
        assert!(
            worker_path.join("parent-advance.txt").exists(),
            "the worker must be cut from the refreshed local tip after a push rejection"
        );
    }

    /// GH #434 / spawn request 1191: a task-level WorkTarget can name an
    /// outer epic branch directly. The resolved source is WorkTarget rather
    /// than TaskEpic, so the outer epic must be found by its branch and
    /// refreshed before cutting the worker.
    #[test]
    fn cas_b6f5_work_target_base_refreshes_outer_epic_and_pushes_before_cut() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        Command::new("git")
            .args(["branch", "epic/outer", "main"])
            .current_dir(&origin)
            .output()
            .unwrap();

        let repo = tmp.path().join("repo");
        Command::new("git")
            .args([
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ])
            .output()
            .expect("git clone");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["branch", "epic/outer", "origin/epic/outer"])
            .current_dir(&repo)
            .output()
            .unwrap();

        commit(&origin, "parent-advance.txt", "parent advanced upstream");
        let parent_tip = head_sha(&origin, "main");

        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut outer_epic = cas_types::Task::new("cas-outer".into(), "outer epic".into());
        outer_epic.task_type = cas_types::TaskType::Epic;
        outer_epic.branch = Some("epic/outer".into());
        outer_epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "main".into(),
        });
        store.add(&outer_epic).unwrap();
        let mut child = cas_types::Task::new("cas-child".into(), "targeted child".into());
        child.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "epic/outer".into(),
        });
        store.add(&child).unwrap();

        let task_base = task_epic_base(&cas_dir, &repo, "cas-child");
        let (base, source) = resolve_spawn_base(&task_base, None, "main");
        assert_eq!(base, "epic/outer");
        assert!(matches!(source, SpawnBaseSource::WorkTarget { .. }));
        let (epic_branch, recorded_parent) =
            recorded_epic_parent_branch_for_resolved_base(&cas_dir, &base)
                .expect("the resolved WorkTarget branch belongs to the outer epic");
        assert_eq!(epic_branch, base);
        assert_eq!(recorded_parent, "main");

        let notice = fast_forward_epic_base_from_parent(&repo, &epic_branch, &recorded_parent)
            .expect("the strictly-behind outer epic must fast-forward")
            .expect("the refresh must be reported");
        assert!(notice.contains("EPIC BASE FAST-FORWARDED"), "{notice}");
        assert!(notice.contains("pushed to origin"), "{notice}");
        assert_eq!(head_sha(&repo, "epic/outer"), parent_tip);
        assert_eq!(
            head_sha(&origin, "epic/outer"),
            parent_tip,
            "the remote epic ref must be refreshed before worker provisioning"
        );
        assert_eq!(
            stale_spawn_base_notice(&repo, "epic/outer", "main"),
            None,
            "a refreshed WorkTarget base must not emit the stale-base warning"
        );

        let worker_path = repo.join(".cas/worktrees/refreshed-outer-worker");
        WorkerSpawnPrep {
            worker_name: "refreshed-outer-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/refreshed-outer-worker".to_string(),
                parent_branch: "epic/outer".to_string(),
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .expect("worker must be cut from the refreshed outer epic");
        assert!(
            worker_path.join("parent-advance.txt").is_file(),
            "the worker must start on the refreshed parent tip, not the stale epic tip"
        );
    }

    #[test]
    fn diverged_epic_base_refuses_refresh_without_moving_the_epic_ref() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/diverged"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "epic-only.txt", "epic-only work");
        let epic_tip = head_sha(&repo, "epic/diverged");
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "parent-only.txt", "parent-only work");

        let error = fast_forward_epic_base_from_parent(&repo, "epic/diverged", "main")
            .expect_err("divergent epic/parent history cannot be fast-forwarded");
        assert!(error.contains("have diverged"), "{error}");
        let refusal = epic_base_refresh_refusal(&error);
        assert!(refusal.contains("EPIC BASE REFRESH REFUSED"), "{refusal}");
        assert!(refusal.contains("epic/diverged"), "{refusal}");
        assert!(refusal.contains("main"), "{refusal}");
        assert!(
            !refusal.contains("will be cut"),
            "a divergence refusal must never promise a stale worker cut: {refusal}"
        );
        assert!(
            !refusal.contains("STALE WORKER BASE"),
            "the irreconcilable base must receive its specific refusal, not a duplicate generic warning: {refusal}"
        );
        assert_eq!(
            head_sha(&repo, "epic/diverged"),
            epic_tip,
            "non-fast-forward parent histories must not overwrite epic work"
        );
        assert!(
            stale_spawn_base_notice(&repo, "epic/diverged", "main").is_some(),
            "the divergence remains explicitly diagnosable to the spawning caller"
        );
    }

    /// GH #584 / cas-a075: the production spawn-preparation seam must stop
    /// before `WorkerSpawnPrep` can cut a no-code worker from a diverged epic.
    #[test]
    fn no_code_spawn_refuses_diverged_epic_before_worktree_cut_cas_a075() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        add_origin(&repo, "https://github.com/example/test.git");

        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/diverged-support"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "epic-only.txt", "support epic work");
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "parent-only.txt", "current support playbook parent");

        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-support-epic".into(), "support epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/diverged-support".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "remote:github.com/example/test".into(),
            target_branch: "main".into(),
        });
        store.add(&epic).unwrap();
        let mut child = cas_types::Task::new("cas-support-child".into(), "support task".into());
        child.execution_note = Some("no-code".into());
        store.add(&child).unwrap();
        store
            .add_dependency(&cas_types::Dependency {
                from_id: child.id.clone(),
                to_id: epic.id.clone(),
                dep_type: cas_types::DependencyType::ParentChild,
                created_at: chrono::Utc::now(),
                created_by: None,
            })
            .unwrap();

        let worktree_root = repo.join(".cas").join("worktrees");
        let manager = WorktreeManager::new(
            &repo,
            WorktreeConfig {
                enabled: true,
                base_path: worktree_root.to_string_lossy().to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            },
        )
        .unwrap();
        let worker_path = manager.worktree_path_for_worker("diverged-support-worker");
        let mut app = FactoryApp::from_init_result(
            cas_dir.clone(),
            Mux::new(40, 120),
            Some(manager),
            DirectorData::load_fast(&cas_dir).unwrap(),
            "support-supervisor".into(),
            Vec::new(),
            crate::ui::factory::notification::NotifyConfig::default(),
            false,
            AutoPromptConfig::default(),
            cas_mux::SupervisorCli::Claude,
            cas_mux::SupervisorCli::Claude,
            120,
            40,
            false,
            None,
            None,
            repo,
        )
        .unwrap();

        let error = match app.prepare_worker_spawn(
            Some("diverged-support-worker"),
            true,
            Some("cas-support-child"),
        ) {
            Ok(_) => panic!("diverged no-code epic must refuse before preparing a cut"),
            Err(error) => error,
        };
        let error = error.to_string();
        assert!(error.contains("EPIC BASE REFRESH REFUSED"), "{error}");
        assert!(error.contains("epic/diverged-support"), "{error}");
        assert!(error.contains("main"), "{error}");
        assert!(
            !worker_path.exists(),
            "refused spawn must not create the worker worktree"
        );
    }

    /// The base can also be stale against its OWN remote — the live checkout is
    /// current, but the local branch the worktree is cut from was never
    /// fast-forwarded after a fetch.
    #[test]
    fn spawn_base_behind_its_remote_tracking_branch_is_reported() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let stale_local = head_sha(&repo, "HEAD");
        commit(&repo, "fetched.txt", "landed upstream");
        let fetched = head_sha(&repo, "HEAD");

        // origin/main knows about the newer commit; local main does not.
        Command::new("git")
            .args(["update-ref", "refs/remotes/origin/main", &fetched])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["update-ref", "refs/heads/main", &stale_local])
            .current_dir(&repo)
            .output()
            .unwrap();

        let notice = stale_spawn_base_notice(&repo, "main", "main")
            .expect("a base behind its own remote must produce a notice");
        assert!(
            notice.contains("target tree differs from 'origin/main'"),
            "notice must identify the remote target tree: {notice}"
        );
    }

    /// No warning when the base is current — the notice has to stay rare enough
    /// to be worth reading.
    #[test]
    fn current_spawn_base_produces_no_notice() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        commit(&repo, "work.txt", "more work");
        Command::new("git")
            .args(["branch", "epic/fresh", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args([
                "update-ref",
                "refs/remotes/origin/main",
                &head_sha(&repo, "main"),
            ])
            .current_dir(&repo)
            .output()
            .unwrap();

        assert_eq!(
            stale_spawn_base_notice(&repo, "epic/fresh", "main"),
            None,
            "a base that already contains trunk and the remote must not warn"
        );
        assert_eq!(
            stale_spawn_base_notice(&repo, "main", "main"),
            None,
            "trunk level with its remote must not warn"
        );
    }

    /// cas-3afc (GH #299): a staging-based epic is legitimately ahead of an
    /// older main after promotions. The declared target is already contained
    /// in the epic, so main's unrelated history must not manufacture a stale
    /// worker-base warning.
    #[test]
    fn staging_epic_superset_does_not_warn_against_unrelated_main_cas_3afc() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        Command::new("git")
            .args(["checkout", "-q", "-b", "staging"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "promoted.txt", "promotion already on staging");
        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/staging-based"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "epic-only.txt", "epic addition");

        assert_eq!(
            stale_spawn_base_notice(&repo, "epic/staging-based", "staging"),
            None,
            "a base that already contains its declared target must stay silent"
        );

        // The factory must never consult the default `main` for this spawn;
        // only the epic's declared staging target is authoritative.
    }

    /// Missing comparison refs (no remote configured, trunk absent) are not
    /// evidence of staleness — a fresh local-only repo must spawn silently.
    #[test]
    fn absent_comparison_refs_do_not_manufacture_a_warning() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        Command::new("git")
            .args(["branch", "epic/solo", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        assert_eq!(
            stale_spawn_base_notice(&repo, "epic/solo", "trunk-that-does-not-exist"),
            None,
            "an unresolvable trunk ref must not be reported as staleness"
        );
    }

    /// A worker branched from a stale base really does end up missing the newer
    /// commits — this is what the warning is protecting against, asserted on
    /// the real `git worktree add` path rather than on the message text.
    #[test]
    fn worktree_cut_from_stale_base_lacks_trunk_commits() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        Command::new("git")
            .args(["branch", "epic/behind", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit(&repo, "release.txt", "release only on trunk");

        let worktree_root = repo.join(".cas").join("worktrees");
        let worker_path = worktree_root.join("late-worker");
        WorkerSpawnPrep {
            worker_name: "late-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worker_path.clone(),
                branch_name: "factory/late-worker".to_string(),
                parent_branch: "epic/behind".to_string(),
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir: repo.join(".cas"),
            }),
            warnings: Vec::new(),
            base_provenance: None,
        }
        .run()
        .expect("worktree creation from the stale base should still succeed");

        assert!(
            !worker_path.join("release.txt").exists(),
            "worker cut from a stale base must be missing trunk's newer commit — \
             this is exactly the silent failure the spawn warning exists to announce"
        );
        assert!(
            stale_spawn_base_notice(&repo, "epic/behind", "main").is_some(),
            "and provisioning must have had a warning to report for it"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Task, TaskStatus};
    use tempfile::TempDir;

    fn seeded_cas_dir() -> (TempDir, std::path::PathBuf) {
        let temp = TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        (temp, cas_dir)
    }

    fn task_with(id: &str, assignee: Option<&str>, status: TaskStatus) -> Task {
        let mut t = Task::new(id.to_string(), format!("title {id}"));
        t.assignee = assignee.map(str::to_string);
        t.status = status;
        t
    }

    #[test]
    fn targeted_shutdown_selects_every_same_session_duplicate_identity() {
        let worker = |id: &str, session: &str| {
            let mut agent = cas_types::Agent::new(id.into(), "knowledge-worker".into());
            agent.role = cas_types::AgentRole::Worker;
            agent.factory_session = Some(session.into());
            agent
        };
        let rows = vec![
            worker("parent", "factory-a"),
            worker("nested-1", "factory-a"),
            worker("nested-2", "factory-a"),
            worker("other-factory", "factory-b"),
        ];

        let selected =
            worker_registry_rows_for_shutdown(&rows, "knowledge-worker", Some("factory-a"));
        let ids: std::collections::HashSet<_> =
            selected.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::HashSet::from(["parent", "nested-1", "nested-2"])
        );
    }

    #[test]
    fn worker_has_open_tasks_true_when_assigned_and_not_closed() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with("t-open", Some("agent-a"), TaskStatus::Open))
            .unwrap();

        assert!(worker_has_open_tasks(&cas_dir, "agent-a"));
    }

    #[test]
    fn worker_has_open_tasks_true_for_in_progress() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with(
                "t-inprog",
                Some("agent-b"),
                TaskStatus::InProgress,
            ))
            .unwrap();

        assert!(worker_has_open_tasks(&cas_dir, "agent-b"));
    }

    #[test]
    fn worker_has_open_tasks_false_when_only_closed_tasks() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with("t-done", Some("agent-c"), TaskStatus::Closed))
            .unwrap();

        assert!(!worker_has_open_tasks(&cas_dir, "agent-c"));
    }

    #[test]
    fn worker_has_open_tasks_false_when_open_task_belongs_to_other_agent() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with("t-other", Some("agent-other"), TaskStatus::Open))
            .unwrap();

        assert!(!worker_has_open_tasks(&cas_dir, "agent-d"));
    }

    // --- cas-6913 / cas-7a94: spawn-time task pre-assignment ------------

    /// AC3: `spawn_workers task_id=<id>` must result in the task's assignee
    /// being the newly spawned worker's display name — the same field
    /// `task action=mine` filters on (cas-dbbb convention) — so the
    /// worker's very first `task mine` shows it.
    #[test]
    fn assign_task_to_new_worker_sets_assignee() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with("cas-abc1", None, TaskStatus::Open))
            .unwrap();

        assert!(
            assign_task_to_new_worker(&cas_dir, "cas-abc1", "swift-fox"),
            "pre-assign should report success"
        );

        let updated = store.get("cas-abc1").unwrap();
        assert_eq!(
            updated.assignee.as_deref(),
            Some("swift-fox"),
            "task should be assigned to the newly spawned worker's display name"
        );
    }

    /// Never overwrite an existing assignee — e.g. a concurrent supervisor
    /// action (or a stale/replayed spawn) must not silently reassign work
    /// away from whoever already has it.
    #[test]
    fn assign_task_to_new_worker_does_not_overwrite_existing_assignee() {
        let (_temp, cas_dir) = seeded_cas_dir();
        // cas-2327 permits a replacement to reclaim a missing/dead holder.
        // Register this existing display-name holder so this fixture continues
        // to exercise the intended live-worker steal-protection contract.
        let holder = "other-worker";
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut agent = cas_types::Agent::new("other-worker-id".into(), holder.into());
        agent.role = cas_types::AgentRole::Worker;
        agents.register(&agent).unwrap();

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with("cas-abc1", Some(holder), TaskStatus::Open))
            .unwrap();

        assert!(
            !assign_task_to_new_worker(&cas_dir, "cas-abc1", "swift-fox"),
            "must not steal another worker's assignment"
        );

        let updated = store.get("cas-abc1").unwrap();
        assert_eq!(
            updated.assignee.as_deref(),
            Some(holder),
            "existing assignee must be preserved, not overwritten"
        );
    }

    /// A missing task_id must not panic — best-effort, log-and-return.
    #[test]
    fn assign_task_to_new_worker_missing_task_does_not_panic() {
        let (_temp, cas_dir) = seeded_cas_dir();
        // No task seeded — "cas-missing" does not exist.
        assert!(!assign_task_to_new_worker(
            &cas_dir,
            "cas-missing",
            "swift-fox"
        ));
    }

    /// cas-7a94: finish-path confirm after early-assign must treat
    /// already-ours as success (no warn-and-skip that would look like failure).
    #[test]
    fn assign_task_to_new_worker_idempotent_for_same_worker() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with("cas-abc1", None, TaskStatus::Open))
            .unwrap();

        assert!(assign_task_to_new_worker(
            &cas_dir,
            "cas-abc1",
            "recipes-fixer"
        ));
        assert!(
            assign_task_to_new_worker(&cas_dir, "cas-abc1", "recipes-fixer"),
            "confirm-path after early-assign must succeed for the same worker"
        );
        assert_eq!(
            store.get("cas-abc1").unwrap().assignee.as_deref(),
            Some("recipes-fixer")
        );
    }

    /// cas-8aee (GH #336): a spawn that races a terminal close must not make
    /// the close look like a new assignment or queue start instructions.
    #[test]
    fn assign_task_to_new_worker_refuses_terminal_tasks() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        for (id, status) in [
            ("cas-closed", TaskStatus::Closed),
            ("cas-cancelled", TaskStatus::Cancelled),
        ] {
            store.add(&task_with(id, None, status)).unwrap();
            assert!(
                !assign_task_to_new_worker(&cas_dir, id, "swift-fox"),
                "terminal task {id} must not be rebound during worker spawn"
            );
            let task = store.get(id).unwrap();
            assert_eq!(task.status, status);
            assert_eq!(task.assignee, None);
        }
    }

    // --- cas-7a94: shutdown / cancel must release pre-assigns -----------

    /// Inverse bug: Open pre-assign to a dead worker must clear on release so
    /// the next worker can transfer/start without a manual reset.
    #[test]
    fn release_worker_task_bindings_clears_open_preassign() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with(
                "cas-7f61",
                Some("recipes-fixer"),
                TaskStatus::Open,
            ))
            .unwrap();

        let n = release_worker_task_bindings(&cas_dir, "recipes-fixer");
        assert_eq!(n, 1, "should clear the Open pre-assign");

        let updated = store.get("cas-7f61").unwrap();
        assert_eq!(updated.assignee, None, "assignee cleared");
        assert_eq!(updated.status, TaskStatus::Open, "Open stays Open");
    }

    /// Inverse bug (observed): InProgress ghost with no live agent / no lease
    /// blocks transfer. Release must force Open + clear assignee.
    #[test]
    fn release_worker_task_bindings_resets_in_progress_ghost() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with(
                "cas-3d23",
                Some("sow-auditor"),
                TaskStatus::InProgress,
            ))
            .unwrap();

        let n = release_worker_task_bindings(&cas_dir, "sow-auditor");
        assert_eq!(n, 1);

        let updated = store.get("cas-3d23").unwrap();
        assert_eq!(updated.assignee, None);
        assert_eq!(updated.status, TaskStatus::Open);
    }

    /// Surgical release used when isolate prep fails after early assign.
    #[test]
    fn release_preassign_if_bound_only_clears_matching_worker() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with(
                "cas-abc1",
                Some("recipes-fixer"),
                TaskStatus::Open,
            ))
            .unwrap();

        // Wrong worker name → no-op
        release_preassign_if_bound(&cas_dir, "cas-abc1", "other-worker");
        assert_eq!(
            store.get("cas-abc1").unwrap().assignee.as_deref(),
            Some("recipes-fixer")
        );

        // Matching worker → cleared
        release_preassign_if_bound(&cas_dir, "cas-abc1", "recipes-fixer");
        assert_eq!(store.get("cas-abc1").unwrap().assignee, None);
    }

    /// A task persists a display-name assignee, not the registry's opaque ID.
    /// A fresh row under that name must prevent the reset path from stealing
    /// its task (GH #170).
    #[test]
    fn preassign_refuses_live_display_name_holder() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let holder = "fresh-heartbeat-worker";
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut agent = cas_types::Agent::new("opaque-agent-id".into(), holder.into());
        agent.role = cas_types::AgentRole::Worker;
        agents.register(&agent).unwrap();

        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with(
                "cas-live-holder",
                Some(holder),
                TaskStatus::InProgress,
            ))
            .unwrap();

        assert!(
            !assign_task_to_new_worker(&cas_dir, "cas-live-holder", "replacement-worker"),
            "a fresh display-name holder must prevent destructive reset"
        );
        let unchanged = store.get("cas-live-holder").unwrap();
        assert_eq!(unchanged.assignee.as_deref(), Some(holder));
        assert_eq!(unchanged.status, TaskStatus::InProgress);
    }

    /// Closed tasks must never be reopened by shutdown release.
    #[test]
    fn release_worker_task_bindings_skips_closed() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        store
            .add(&task_with(
                "cas-done",
                Some("dead-worker"),
                TaskStatus::Closed,
            ))
            .unwrap();

        assert_eq!(release_worker_task_bindings(&cas_dir, "dead-worker"), 0);
        let t = store.get("cas-done").unwrap();
        assert_eq!(t.assignee.as_deref(), Some("dead-worker"));
        assert_eq!(t.status, TaskStatus::Closed);
    }

    // --- cas-9bc6: resolve_live_worker_harness reads from disk, not cache ----

    fn cas_dir_with_config(config_toml: &str) -> (TempDir, std::path::PathBuf) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        std::fs::write(cas_dir.join("config.toml"), config_toml).unwrap();
        (temp, cas_dir)
    }

    /// AC4 anchor — spawn handler reads live LlmConfig, not the cached field.
    ///
    /// After writing `llm.worker.harness = "codex"` to disk, calling
    /// `resolve_live_worker_harness` must return `Codex`, proving the function
    /// reads the current on-disk config rather than a stale in-memory value.
    #[test]
    fn resolve_live_worker_harness_returns_codex_after_config_change() {
        let (_temp, cas_dir) = cas_dir_with_config("[llm.worker]\nharness = \"codex\"\n");
        let harness = resolve_live_worker_harness(&cas_dir);
        assert_eq!(
            harness,
            cas_mux::SupervisorCli::Codex,
            "live config with worker.harness=codex must yield SupervisorCli::Codex"
        );
    }

    /// Absent config (no config.toml) resolves through the empty-config
    /// cascade to the worker-only stock floor — Codex (cas-fbac). This is
    /// NOT the same code path as a genuinely unparseable/unreadable config;
    /// `Config::load` returns `Ok(Config::default())` for a missing file, so
    /// it goes through `harness_for_role("worker")` like any other config
    /// and only the parse-failure branch below falls back to Claude.
    #[test]
    fn resolve_live_worker_harness_defaults_to_codex_when_config_absent() {
        let temp = TempDir::new().unwrap();
        let empty_cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&empty_cas_dir).unwrap();
        // No config.toml written.
        let harness = resolve_live_worker_harness(&empty_cas_dir);
        assert_eq!(
            harness,
            cas_mux::SupervisorCli::Codex,
            "missing config must resolve to the worker stock floor SupervisorCli::Codex"
        );
    }

    /// Simulates the round-trip in the bug report:
    /// boot with claude → `cas config set codex` → next spawn sees codex
    /// → `cas config set claude` → next spawn reverts to claude.
    #[test]
    fn resolve_live_worker_harness_reflects_config_rewrites() {
        let (_temp, cas_dir) = cas_dir_with_config("[llm.worker]\nharness = \"codex\"\n");
        assert_eq!(
            resolve_live_worker_harness(&cas_dir),
            cas_mux::SupervisorCli::Codex,
            "first read: codex"
        );

        // Rewrite config to claude (simulates `cas config set llm.worker.harness claude`)
        std::fs::write(
            cas_dir.join("config.toml"),
            "[llm.worker]\nharness = \"claude\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve_live_worker_harness(&cas_dir),
            cas_mux::SupervisorCli::Claude,
            "after revert: claude"
        );
    }

    /// Unknown/garbage harness string falls back to Claude.
    #[test]
    fn resolve_live_worker_harness_falls_back_on_unknown_harness_string() {
        let (_temp, cas_dir) = cas_dir_with_config("[llm.worker]\nharness = \"chatgpt\"\n");
        let harness = resolve_live_worker_harness(&cas_dir);
        assert_eq!(
            harness,
            cas_mux::SupervisorCli::Claude,
            "unknown harness string must fall back to SupervisorCli::Claude"
        );
    }
}
