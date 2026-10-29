use crate::store::{open_agent_store, open_task_store_cached};
use crate::ui::factory::app::imports::*;
use crate::worktree::RemoveOutcome;

fn validate_live_spawn_repo_root(
    repo_root: &std::path::Path,
    project_path: &std::path::Path,
) -> anyhow::Result<()> {
    use crate::worktree::GitOperations;

    let live_root = GitOperations::detect_repo_root(project_path).map_err(|error| {
        anyhow::anyhow!(
            "Repository is not available at spawn time: {error}. If git was initialized after \
             the factory started, restart the factory daemon before spawning isolated workers."
        )
    })?;
    if live_root != repo_root {
        anyhow::bail!(
            "Repository context changed after the factory daemon started (cached root: {}, \
             live root: {}). Restart the factory daemon so worker isolation uses the new repository.",
            repo_root.display(),
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

/// Longest a worker's kill-and-drop may hold a shutdown (GH #1363): the
/// 3 s TERM grace, SIGKILL and reap, with margin.
const WORKER_TEARDOWN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

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
    /// Branch recorded on that epic. An empty value means the epic has no
    /// recorded branch authority; callers must not derive one from its title.
    pub branch: String,
    /// Whether `branch` currently resolves to a commit in the repository.
    pub branch_exists: bool,
    /// Retained for receipt compatibility with older task snapshots. Fresh
    /// resolution never synthesizes a title slug, so this is always false.
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
///   4. trunk, when the spawn names a task that provably belongs to no epic
///      (cas-d897 / GH #146);
///   5. the pinned epic focus (taskless spawns, tasks the store could not
///      resolve, and tasks whose epic has no branch yet — falling back to
///      focus there preserves pre-fix behavior rather than silently dropping
///      a worker onto trunk);
///   6. configured trunk / detected default branch.
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

/// Surface the one hazardous legacy shape when it is explicitly represented
/// by a caller. Fresh epic records no longer synthesize a title-derived lane.
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
        Some(worktree) => {
            let checkout_base = worktree
                .base_ref
                .as_deref()
                .unwrap_or(&worktree.parent_branch);
            let checkout_sha = short_sha(&worktree.repo_root, checkout_base);
            format!(
                "Preparing worker filesystem and worktree. Worktree repository: {} (CAS_ROOT remains the factory session store). Spawn base: '{}' @ {} (merge-back parent '{}').",
                worktree.repo_root.display(),
                checkout_base,
                checkout_sha,
                worktree.parent_branch,
            )
        }
        None => "Preparing worker filesystem (no isolated worktree).".to_string(),
    }
}

/// Receipt emitted after slow provisioning has completed. Unlike the initial
/// provision receipt this can truthfully name the immutable snapshot and the
/// hardlink counts observed in the new worker target.
pub(crate) fn target_seed_receipt(
    result: &crate::ui::factory::app::WorkerSpawnResult,
) -> Option<String> {
    result.target_seed.as_ref().map(|stats| {
        format!(
            "Cargo target seed: snapshot='{}'; source_commit='{}'; age_secs={}; skipped_crates='{}'; hardlinked_files={}; hardlinked_bytes={}",
            stats.snapshot,
            stats.source_commit,
            stats.age_secs,
            if stats.skipped_crates.is_empty() {
                "none".to_string()
            } else {
                stats.skipped_crates.join(",")
            },
            stats.files,
            stats.bytes
        )
    })
}

/// cas-7587: resolve `task_id` → its epic → that epic's branch.
///
/// A task that *is* an epic resolves to itself. The branch is the one persisted
/// on the epic task; missing branches are never reconstructed from the title.
/// `branch_exists` records
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
    let store = match open_task_store_cached(cas_dir) {
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

    let branch = crate::mcp::tools::core::task::repo_context::effective_epic_branch(&epic)
        .filter(|_| epic.branch.as_deref().is_some_and(|branch| !branch.trim().is_empty()))
        .unwrap_or_default()
        .to_string();
    let branch_is_title_slug_fallback = false;
    let branch_exists = !branch.is_empty() && ref_exists(repo_root, &branch);
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

/// Cheap, read-only checks shared by the MCP enqueue path. Provisioning still
/// fetches and checks again, since refs may change after a request is queued.
pub(crate) fn preflight_spawn_base(
    cas_dir: &std::path::Path,
    session_repo_root: &std::path::Path,
    task_id: Option<&str>,
    focused_epic_branch: Option<&str>,
) -> Result<(), String> {
    let task_base = task_id
        .map(|id| task_epic_base(cas_dir, session_repo_root, id))
        .unwrap_or(TaskBase::Unresolved);
    if let Some(epic) = task_base.epic()
        && epic.work_target.is_none()
        && (epic.branch.is_empty() || !epic.branch_exists)
    {
        return Err(format!(
            "spawn refused: task {} belongs to epic {} but the epic has no recorded WorkTarget or resolvable branch; nothing was queued",
            epic.task_id, epic.epic_id,
        ));
    }
    let repo = resolve_spawn_worktree_repo(cas_dir, session_repo_root, task_base.work_target())
        .map_err(|error| error.to_string())?;
    let git = crate::worktree::GitOperations::new(repo.clone());
    if !git.has_commits().map_err(|error| error.to_string())? {
        return Err("Repository has no commits. Make an initial commit before spawning workers; nothing was queued.".into());
    }
    let configured_trunk = Config::configured_epic_base_branch(&repo)
        .unwrap_or_else(|| git.detect_default_branch());
    let trunk = task_base.target_branch().unwrap_or(&configured_trunk);
    let (base, _) = resolve_spawn_base(&task_base, focused_epic_branch, trunk);
    let recorded_epic = open_task_store_cached(cas_dir)
        .map_err(|error| error.to_string())?
        .epic_for_branch(&base)
        .map_err(|error| error.to_string())?;
    if let Some(epic) = &recorded_epic
        && epic.is_terminal()
    {
        return Err(format!(
            "spawn refused: epic {} is {}; nothing was queued", epic.id, epic.status,
        ));
    }
    // Managed lanes have a separate reconciliation/publication policy. Their
    // provisioner can preserve unpublished work instead of refusing divergence.
    if base.starts_with("epic/") && recorded_epic.is_some() {
        return Ok(());
    }
    let remote = format!("origin/{base}");
    if git.has_origin_remote()
        && !base.starts_with("origin/")
        && let (Some(ahead), Some(behind)) = (
            commits_behind(&repo, &remote, &base),
            commits_behind(&repo, &base, &remote),
        )
        && ahead > 0
        && behind > 0
    {
        return Err(format!(
            "{}\nNothing was queued.",
            spawn_base_divergence_refusal(&repo, &base, &remote, ahead, behind)?,
        ));
    }
    Ok(())
}

fn spawn_base_divergence_refusal(
    repo: &std::path::Path,
    base: &str,
    remote: &str,
    ahead: usize,
    behind: usize,
) -> Result<String, String> {
    let git = crate::worktree::GitOperations::new(repo.to_path_buf());
    let checkout = git.list_worktrees().map_err(|error| error.to_string())?
        .into_iter()
        .find(|checkout| checkout.branch.as_deref() == Some(base));
    let path = checkout.as_ref()
        .map(|checkout| checkout.path.as_path())
        .unwrap_or(repo);
    let [rebase] = crate::worktree::GitOperations::git_remedies([
        &["-C", &path.to_string_lossy(), "rebase", "--autostash", remote, base],
    ])?;
    Ok(format!(
        "SPAWN BASE DIVERGED: local '{base}' ({}), '{remote}' ({}); {ahead} local-only, {behind} remote-only commit(s). No worker was created and neither branch was moved. Reconcile before retrying:\n{rebase}{}",
        full_sha(repo, base).unwrap_or_else(|| "unresolved".into()),
        full_sha(repo, remote).unwrap_or_else(|| "unresolved".into()),
        git.reconcile_commit_cause(base, remote),
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
/// Successful selection returns `(checkout_ref_override, notice)`:
/// - no `origin/<base>` (or identical tips) → `(None, None)`: cut from the
///   local branch exactly as before;
/// - local strictly behind origin (fast-forwardable) → the override is
///   origin's **commit id**, plus a notice naming both SHAs and the gap;
/// - configured origin fetch fails or the two diverge → refuse before creating
///   a worker, naming the observed refs and counts (cas-02ae);
/// - managed epic observations retain their existing unpublished/local policy;
/// - local ahead → `(None, None)`.
///
/// The override is a raw commit id, not `origin/<base>`: the branch name the
/// worktree records as its parent must stay a *local* branch (it is the
/// merge-back target), and cutting from a commit id avoids silently setting the
/// worker branch's upstream to someone else's branch.
#[cfg(test)]
fn prefer_fresher_base_ref(
    repo_root: &std::path::Path,
    base: &str,
) -> (Option<String>, Option<String>) {
    prefer_fresher_base_ref_observed(repo_root, base, true).expect("verified spawn base")
}

fn prefer_fresher_base_ref_observed(
    repo_root: &std::path::Path, base: &str, fetch: bool,
) -> Result<(Option<String>, Option<String>), String> {
    let git = crate::worktree::GitOperations::new(repo_root.to_path_buf());
    let branch = base.strip_prefix("origin/").unwrap_or(base);
    let remote = format!("origin/{branch}");
    let verify_origin = fetch && git.has_origin_remote();
    let observed_state = || {
        let local_sha = full_sha(repo_root, base).unwrap_or_else(|| "unresolved".into());
        let remote_sha = full_sha(repo_root, &remote).unwrap_or_else(|| "unresolved".into());
        let ahead = commits_behind(repo_root, &remote, base)
            .map(|n| n.to_string()).unwrap_or_else(|| "unknown".into());
        let behind = commits_behind(repo_root, base, &remote)
            .map(|n| n.to_string()).unwrap_or_else(|| "unknown".into());
        format!("local '{base}' ({local_sha}), '{remote}' ({remote_sha}); {ahead} local-only, {behind} remote-only commit(s)")
    };
    // A configured remote must answer successfully. Cached tracking refs cannot
    // prove freshness after a failed fetch. Managed epics own their observation
    // and publication policy, so they deliberately arrive with fetch=false.
    if verify_origin {
        git.fetch_branch(branch).map_err(|error| {
            // A failed fetch alone cannot distinguish missing publication from
            // transport failure. Git's exit 2 means a reachable origin has no
            // matching head; stderr and cached refs are not absence evidence.
            let mut command = std::process::Command::new("git");
            command.args(["ls-remote", "--exit-code", "--heads", "origin", &format!("refs/heads/{branch}")])
                .env("GIT_TERMINAL_PROMPT", "0")
                .current_dir(repo_root);
            let absent = crate::worktree::GitOperations::run_command_bounded(
                command, std::time::Duration::from_secs(10),
            ).is_ok_and(|output| output.status.code() == Some(2));
            if absent {
                let [push] = match crate::worktree::GitOperations::git_remedies([
                    &["push", "origin", branch],
                ]) {
                    Ok(commands) => commands,
                    Err(refusal) => return refusal,
                };
                format!(
                    "SPAWN BASE NOT PUBLISHED: 'origin/{branch}' is not published; {}. No worker was created; publish before retrying:\n{push}",
                    observed_state(),
                )
            } else {
                format!(
                    "SPAWN BASE UNVERIFIED: fetch from origin for '{branch}' failed: {error}; {}. No worker was created; restore origin access before retrying.",
                    observed_state(),
                )
            }
        })?;
    }
    if base.starts_with("origin/") {
        return Ok((None, None));
    }
    let (Some(local_sha), Some(remote_sha)) = (full_sha(repo_root, base), full_sha(repo_root, &remote)) else {
        if verify_origin {
            return Err(format!("SPAWN BASE UNVERIFIED: refs did not resolve after fetch; {}. No worker was created; repair the origin tracking ref before retrying.", observed_state()));
        }
        return Ok((None, None));
    };
    if local_sha == remote_sha {
        return Ok((None, None));
    }

    let behind = commits_behind(repo_root, base, &remote);
    let ahead = commits_behind(repo_root, &remote, base);
    if verify_origin && (behind.is_none() || ahead.is_none()) {
        return Err(format!("SPAWN BASE UNVERIFIED: ancestry comparison failed; {}. No worker was created; repair repository history before retrying.", observed_state()));
    }
    let behind = behind.unwrap_or(0);
    let ahead = ahead.unwrap_or(0);
    if verify_origin && behind > 0 && ahead > 0 {
        return Err(spawn_base_divergence_refusal(repo_root, base, &remote, ahead, behind)?);
    }

    Ok(match (behind, ahead) {
        (0, _) => (None, None),
        (behind, 0) => {
            let git = crate::worktree::GitOperations::new(repo_root.to_path_buf());
            let advance = if base.starts_with("epic/") {
                git.advance_local_branch(base, &local_sha, &remote_sha)
            } else { Ok(()) };
            if verify_origin && let Err(error) = &advance {
                return Err(format!(
                    "SPAWN BASE UNVERIFIED: local base could not fast-forward: {error}; {}. No worker was created; reconcile its checkout before retrying.",
                    observed_state(),
                ));
            }
            let state = match advance {
                Ok(()) if base.starts_with("epic/") => "The local epic branch was fast-forwarded to the fetched tip.".to_string(),
                Ok(()) => "The local integration branch was left unchanged.".to_string(),
                Err(error) => format!("The local branch was preserved: {error}. Reconcile its checkout before retrying."),
            };
            (full_sha(repo_root, &remote), Some(format!(
                "SPAWN BASE REFRESHED: local '{base}' ({local_sha}) is {behind} commit(s) behind \
                 '{remote}' ({remote_sha}); the worker worktree was cut from '{remote}' \
                 ({remote_sha}) so it starts on the fetched tip instead of the stale local ref \
                 (cas-d897 / GH #146). {state}"
            )))
        },
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
    })
}

/// A spawn's logical parent must be a local branch so worktree_merge can use
/// it later. Materialize an origin-only branch before checkout-base selection.
fn ensure_local_spawn_parent(
    repo_root: &std::path::Path,
    parent: &str,
) -> Result<Option<String>, String> {
    if parent.starts_with("origin/") {
        return Ok(None);
    }
    let local = parent.strip_prefix("refs/heads/").unwrap_or(parent);
    if full_sha(repo_root, &format!("refs/heads/{local}")).is_some() {
        return Ok(None);
    }
    let git = crate::worktree::GitOperations::new(repo_root.to_path_buf());
    let local_label = crate::worktree::GitOperations::visible_git_label(local);
    let fetch_error = git.fetch_branch(local).err();
    if git.has_origin_remote() && let Some(error) = &fetch_error {
        let cause = crate::worktree::GitOperations::visible_git_label(&error.to_string());
        return Err(format!("SPAWN BASE UNVERIFIED: fetch from origin for '{local_label}' failed: {cause}; no local spawn parent was created. Restore origin access before retrying."));
    }
    let remote = format!("origin/{local}");
    let remote_label = crate::worktree::GitOperations::visible_git_label(&remote);
    let remote_sha = full_sha(repo_root, &remote).ok_or_else(|| {
        let parent_label = crate::worktree::GitOperations::visible_git_label(parent);
        let remote_label = crate::worktree::GitOperations::visible_git_label(&remote);
        let [fetch] = match crate::worktree::GitOperations::git_remedies([
            &["fetch", "origin", local],
        ]) {
            Ok(commands) => commands,
            Err(refusal) => return refusal,
        };
        format!(
            "spawn base '{parent_label}' does not resolve locally or as '{remote_label}' after fetch{}; fetch and create a local branch from '{remote_label}' before retrying:\n{fetch}",
            fetch_error
                .map(|error| format!(" ({})", crate::worktree::GitOperations::visible_git_label(&error.to_string())))
                .unwrap_or_default()
        )
    })?;
    git.create_branch_from(local, &remote).map_err(|error| {
        let cause = crate::worktree::GitOperations::visible_git_label(&error.to_string());
        format!("could not create local spawn base '{local_label}' from '{remote_label}': {cause}")
    })?;
    Ok(Some(format!(
        "SPAWN BASE MATERIALIZED: local '{local_label}' created from '{remote_label}' ({}) for worker checkout and merge-back.",
        &remote_sha[..remote_sha.len().min(12)]
    )))
}

/// Resolve the immutable commit a worker will be checked out from, preserving
/// the logical parent branch used for merge-back. A declared WorkTarget is
/// fetched before selection: configured-origin fetch failures and divergence
/// refuse before worker creation. Managed epics retain a local-only tip when
/// an epic-base refresh could not be published (GH #450).
#[cfg(test)]
fn checkout_ref_for_spawn_base(
    repo_root: &std::path::Path,
    parent_branch: &str,
    source: &SpawnBaseSource,
) -> Result<(Option<String>, Option<String>, String), String> {
    checkout_ref_for_spawn_base_observed(repo_root, parent_branch, source, true)
}

fn checkout_ref_for_spawn_base_observed(
    repo_root: &std::path::Path, parent_branch: &str, source: &SpawnBaseSource, fetch: bool,
) -> Result<(Option<String>, Option<String>, String), String> {
    let (mut base_ref, mut freshness_notice) = prefer_fresher_base_ref_observed(repo_root, parent_branch, fetch)?;
    let mut checkout_ref = parent_branch.to_string();
    if matches!(source, SpawnBaseSource::WorkTarget { .. }) {
        let remote = format!("origin/{parent_branch}");
        if let (Some(local_sha), Some(remote_sha)) = (
            full_sha(repo_root, parent_branch),
            full_sha(repo_root, &remote),
        ) {
            // `prefer_fresher_base_ref` selected the remote only when it is
            // strictly ahead of local. Pin the equal case to the fetched
            // object; local-only ahead or managed divergent histories retain
            // the local object. An unpublished
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
    if fetch && !crate::worktree::GitOperations::new(repo_root.to_path_buf()).has_origin_remote() {
        let selected = base_ref.as_deref().unwrap_or(parent_branch);
        let sha = full_sha(repo_root, selected).unwrap_or_else(|| "unresolved".into());
        let remote = format!("origin/{parent_branch}");
        if base_ref.is_some() && full_sha(repo_root, &remote).as_deref() == Some(sha.as_str()) {
            checkout_ref = remote;
        }
        freshness_notice = Some(format!(
            "SPAWN BASE LOCAL-ONLY: origin is not configured; checkout '{checkout_ref}' ({sha})."
        ));
    }
    Ok((base_ref, freshness_notice, checkout_ref))
}

#[derive(Clone, Copy)]
enum EpicBasePublication {
    Published,
    Unpublished,
    Unverified,
}

/// Include nested children and legacy explicit-target children. Any durable
/// delivery evidence prevents the planning-only reset shortcut.
fn epic_has_no_task_deliveries(
    cas_dir: &std::path::Path,
    epic: &cas_types::Task,
) -> anyhow::Result<bool> {
    let store = open_task_store_cached(cas_dir)?;
    let mut ids = std::collections::HashSet::new();
    let mut pending = vec![epic.id.clone()];
    while let Some(id) = pending.pop() {
        if !ids.insert(id.clone()) {
            continue;
        }
        pending.extend(
            store
                .get_dependents(&id)?
                .into_iter()
                .filter(|edge| edge.dep_type == cas_types::DependencyType::ParentChild)
                .map(|edge| edge.from_id),
        );
    }
    Ok(!store.list(None)?.iter().any(|task| {
        let belongs = ids.contains(&task.id)
            || task
                .deliverables
                .work_target
                .as_ref()
                .is_some_and(|target| {
                    Some(target.target_branch.as_str()) == epic.branch.as_deref()
                });
        let d = &task.deliverables;
        belongs
            && (!d.files_changed.is_empty()
                || d.commit_hash.is_some()
                || d.merge_commit.is_some()
                || d.factory_branch_anchor.is_some()
                || !d.historical_factory_branch_anchors.is_empty()
                || d.delivery_pr_number.is_some()
                || d.delivery_pr_merge_commit.is_some()
                || d.parked_branch.is_some()
                || d.integration_batch.is_some()
                || !d.handoff_branches.is_empty()
                || d.green_merge_commit.is_some())
    }))
}

/// A positive ancestry proof may use cached shared trunk refs: staleness can
/// miss a safe reset, but cannot invent reachability. Never use a local-only
/// operator commit as shared trunk while origin is configured.
fn epic_unique_work_is_in_trunk(
    git: &crate::worktree::GitOperations,
    repo: &std::path::Path,
    epic: &str,
    parent: &str,
) -> Result<bool, String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["rev-list", "--max-count=1", epic, "--not", parent])
        .current_dir(repo);
    // A legacy branch merely named main/master/trunk is not trunk authority.
    let branch = git.detect_default_branch();
    if branch.starts_with("epic/") || branch.starts_with("factory/") {
        return Ok(false);
    }
    let shared = if git.has_origin_remote() {
        format!("origin/{branch}")
    } else {
        branch
    };
    if let Some(sha) = full_sha(repo, &shared) {
        command.arg(sha);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "could not inspect epic-only history: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout.is_empty())
}

fn epic_reconcile_command(repo: &std::path::Path, epic: &str, parent: &str) -> Result<String, String> {
    let shared = format!(
        "origin/{}",
        parent.strip_prefix("refs/heads/").unwrap_or(parent)
    );
    let parent = if !parent.starts_with("origin/") && full_sha(repo, &shared).is_some() {
        shared.as_str()
    } else {
        parent
    };
    let git = crate::worktree::GitOperations::new(repo.to_path_buf());
    let branch = epic.strip_prefix("refs/heads/").unwrap_or(epic);
    let checkout = git
        .list_worktrees()
        .map_err(|error| format!("could not locate epic checkout for reconciliation: {error}"))?
        .into_iter()
        .find(|checkout| checkout.branch.as_deref() == Some(branch));
    if let Some(checkout) = checkout {
        let [merge] = crate::worktree::GitOperations::git_remedies([
            &["-C", &checkout.path.to_string_lossy(), "merge", parent],
        ])?;
        return Ok(merge);
    }
    let [switch, merge] = crate::worktree::GitOperations::git_remedies([
        &["-C", &repo.to_string_lossy(), "switch", epic],
        &["-C", &repo.to_string_lossy(), "merge", parent],
    ])?;
    Ok(format!("{switch} && {merge}"))
}

/// Fast-forward an epic's local base ref from the parent branch it recorded
/// when the two histories are cleanly aligned. A spawned worker merges back
/// into the epic ref, so updating *and publishing* that ref (rather than
/// merely checking out the parent commit) keeps the new worker, later syncs,
/// and the next spawning supervisor on the same integration history.
///
/// The parent is resolved from `origin/<parent>` whenever that shared ref is
/// available before the relationship is tested. A local parent that is ahead
/// of origin is refused rather than treated as authoritative: a worker must
/// not be cut from an unpublished operator commit. A genuinely split
/// epic/parent history is refused unless the lane is verified unpublished and
/// its unique commits are already in trunk or it has no task deliveries. That
/// stale planning lane resets to the parent, retaining its old graph for recovery.
fn refresh_epic_base_from_parent(
    repo_root: &std::path::Path,
    epic_branch: &str,
    parent_branch: &str,
    parent_ref: &str,
    publication: EpicBasePublication,
    no_task_deliveries: impl FnOnce() -> Result<bool, String>,
) -> Result<Option<String>, String> {
    if epic_branch == parent_branch
        || !ref_exists(repo_root, epic_branch)
        || !ref_exists(repo_root, parent_ref)
    {
        return Ok(None);
    }

    let epic_sha = full_sha(repo_root, epic_branch)
        .ok_or_else(|| format!("could not resolve epic base '{epic_branch}'"))?;
    let parent_sha = full_sha(repo_root, &parent_ref)
        .ok_or_else(|| format!("could not resolve recorded parent '{parent_ref}'"))?;
    if epic_sha == parent_sha {
        return Ok(None);
    }

    let git = crate::worktree::GitOperations::new(repo_root.to_path_buf());
    let mut reset_reason = None;
    let is_ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &epic_sha, &parent_sha])
        .current_dir(repo_root)
        .status()
        .map_err(|error| {
            format!(
                "could not check whether epic base '{epic_branch}' can fast-forward from '{parent_ref}': {error}"
            )
        })?;
    if !matches!(is_ancestor.code(), Some(0 | 1)) {
        return Err("could not compare epic and parent ancestry; no refs changed".into());
    }
    if !is_ancestor.success() {
        let parent_is_ancestor = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &parent_sha, &epic_sha])
            .current_dir(repo_root)
            .status()
            .map_err(|error| {
                format!(
                    "could not check whether recorded parent '{parent_ref}' is contained in epic base '{epic_branch}': {error}"
                )
            })?;
        if !matches!(parent_is_ancestor.code(), Some(0 | 1)) {
            return Err("could not compare parent and epic ancestry; no refs changed".into());
        }
        if parent_is_ancestor.success() {
            return Ok(None);
        }
        if let EpicBasePublication::Unpublished = publication {
            // Delivery inventory is needed only to authorize a diverged reset.
            // Aligned lanes must not depend on a whole-backlog read succeeding.
            if no_task_deliveries()? {
                reset_reason = Some(("NO TASK DELIVERIES", "no task deliveries are recorded"));
            } else if epic_unique_work_is_in_trunk(&git, repo_root, &epic_sha, &parent_sha)? {
                reset_reason = Some((
                    "TRUNK-COVERED WORK",
                    "all epic-only commits are already reachable from trunk",
                ));
            }
        }
        if reset_reason.is_none() {
            return Err(format!(
                "epic base '{epic_branch}' ({}) and recorded parent '{parent_ref}' ({}) have diverged; refusing to cut a worker from an unreconciled base. Reconcile: `{}`{}",
                &epic_sha[..epic_sha.len().min(8)],
                &parent_sha[..parent_sha.len().min(8)],
                epic_reconcile_command(repo_root, epic_branch, parent_ref)?,
                git.reconcile_commit_cause(parent_ref, epic_branch),
            ));
        }
    }

    // A ref update is invisible to any checkout that has this branch out:
    // inventory and guard those checkouts before moving the ref, then refresh
    // them after the compare-and-swap so spawn cannot strand a stale index.
    let branch = epic_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(epic_branch);
    let checkouts = git.list_worktrees().map_err(|error| {
        format!("could not inventory checkouts for epic base '{epic_branch}': {error}")
    })?;
    let checkouts: Vec<_> = checkouts
        .into_iter()
        .filter(|checkout| checkout.branch.as_deref() == Some(branch))
        .collect();
    for checkout in &checkouts {
        let dirty = git.classify_dirty_status(&checkout.path).map_err(|error| {
            format!(
                "could not inspect checkout {} before advancing epic base '{epic_branch}': {error}",
                checkout.path.display()
            )
        })?;
        if dirty.is_blocked() {
            return Err(format!(
                "epic base '{epic_branch}' is checked out with tracked changes at {} ({})",
                checkout.path.display(),
                dirty.describe_blocking()
            ));
        }
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
    // A planning-only lane can still contain an unrecorded manual commit.
    // Preserve the old graph under a durable ref before the guarded reset.
    let recovery = format!("refs/cas/epic-base-resets/{epic_sha}");
    if reset_reason.is_some() {
        let saved = std::process::Command::new("git")
            .args(["update-ref", &recovery, &epic_sha])
            .current_dir(repo_root)
            .output()
            .map_err(|error| format!("could not preserve epic tip before reset: {error}"))?;
        if !saved.status.success() {
            return Err("could not preserve epic tip before reset; no branch changed".into());
        }
    }
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

    for checkout in checkouts {
        if let crate::worktree::git::CheckoutRefresh::LeftStale { reason } =
            git.refresh_linked_checkout(&checkout.path, branch, &epic_sha, &parent_sha)
        {
            return Err(format!(
                "epic base '{epic_branch}' advanced but checkout {} could not be refreshed: {reason}",
                checkout.path.display()
            ));
        }
    }

    if let Some((label, reason)) = reset_reason {
        return Ok(Some(format!(
            "EPIC BASE RESET (UNPUBLISHED, {label}): '{epic_branch}' ({epic_sha}) reset to recorded parent '{parent_ref}' ({parent_sha}); {reason}. Previous tip retained at '{recovery}'. Publication is handled separately before worker checkout."
        )));
    }
    if !matches!(publication, EpicBasePublication::Published) {
        return Ok(Some(format!(
            "EPIC BASE FAST-FORWARDED (LOCAL): epic base '{epic_branch}' ({epic_sha}) advanced to its recorded parent '{parent_branch}' via '{parent_ref}' ({parent_sha}); publication is handled separately; the refreshed local tip will be used."
        )));
    }

    // This path is selected only after a verified origin fetch. A failed
    // push retains the safe local fast-forward for the worker checkout.
    let mut command = std::process::Command::new("git");
    command
        .args(["push", "origin", &format!("{refname}:{refname}")])
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(repo_root);
    let push = crate::worktree::GitOperations::run_command_bounded(
        command,
        std::time::Duration::from_secs(30),
    );
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


#[cfg(test)]
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

/// cas-58a3: what a crashed worker's off-loop cleanup needs.
#[derive(Debug, Clone)]
pub struct CrashCleanup {
    pub cas_dir: std::path::PathBuf,
    pub factory_session: Option<String>,
    pub worker: String,
    /// The worker's tracked worktree, if any; reclaimed only when it holds no
    /// open task and is clean.
    pub worktree: Option<crate::worktree::WorkerRemovalPlan>,
}

/// cas-58a3: what the off-loop crash cleanup did to the worktree. `None` when
/// it was left alone (open tasks, or none tracked).
#[derive(Debug)]
pub struct CrashCleanupOutcome {
    pub worktree: Option<crate::worktree::WorktreeResult<RemoveOutcome>>,
}

pub(crate) const WORKER_CLEANUP_WARN_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) struct WorkerCleanupJob {
    pub(crate) worker: String,
    pub(crate) occurrence: String,
    pub(crate) started: Instant,
    pub(crate) warned: bool,
    pub(crate) handle: std::thread::JoinHandle<CrashCleanupOutcome>,
    pub(crate) recovery_path: Option<PathBuf>,
}

impl WorkerCleanupJob {
    pub(crate) fn new(worker: String, handle: std::thread::JoinHandle<CrashCleanupOutcome>) -> Self {
        Self { worker, occurrence: uuid::Uuid::new_v4().to_string(), started: Instant::now(), warned: false, handle, recovery_path: None }
    }
}

/// Pending work survives a bounded daemon exit, independently of agent-row deletion.
#[derive(serde::Serialize, serde::Deserialize)]
struct DeferredWorkerCleanup {
    occurrence: String,
    agent: cas_types::Agent,
    tasks: Vec<cas_types::Task>,
    worktree: Option<(cas_types::Worktree, PathBuf)>,
}

impl DeferredWorkerCleanup {
    fn path(&self, root: &std::path::Path) -> PathBuf {
        root.join("worker-cleanups").join(format!("{}.json", self.occurrence))
    }

    fn persist(&self, root: &std::path::Path) -> anyhow::Result<()> {
        use std::io::Write;
        let path = self.path(root);
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory)?;
        anyhow::ensure!(std::fs::symlink_metadata(directory)?.file_type().is_dir(), "cleanup intent directory is not a real directory");
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(temporary.as_file_mut(), self)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path)?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    }

    fn run(&self, root: &std::path::Path) -> anyhow::Result<CrashCleanupOutcome> {
        let Some(clone_path) = self.agent.metadata.get("clone_path") else {
            return Ok(CrashCleanupOutcome { worktree: None });
        };
        let path = std::path::Path::new(clone_path);
        if !path.exists() && !crate::factory_scratch::worker_target_dir(path).exists() {
            return Ok(CrashCleanupOutcome { worktree: None });
        }
        if let Some((worktree, _)) = &self.worktree {
            anyhow::ensure!(worktree.path.canonicalize()? == path.canonicalize()?, "cleanup worktree differs from its retired owner");
        }
        if !crate::factory_target_cache::retirement::retire_worker_with_snapshot(root, &self.agent, &self.tasks)? {
            // A deliberate hold completes this attempt; frozen snapshots cannot
            // make progress by replaying at every subsequent daemon startup.
            return Ok(CrashCleanupOutcome { worktree: None });
        }
        let Some((worktree, repo_root)) = &self.worktree else {
            return Ok(CrashCleanupOutcome { worktree: None });
        };
        let outcome = crate::worktree::remove_worker_worktree_io(&crate::worktree::WorkerRemovalPlan {
            worker_name: self.agent.name.clone(), worktree: worktree.clone(), repo_root: repo_root.clone(),
        });
        if let Ok(RemoveOutcome::DirtyDeferred(warning)) = &outcome {
            if let Err(error) = (|| -> anyhow::Result<()> {
                let agents = open_agent_store(root)?;
                flag_agent_dirty_on_shutdown(agents.as_ref(), &self.agent.id, &warning.path, warning.file_count)
            })()
            {
                tracing::warn!(%error, worker = %self.agent.name, "could not flag dirty cleanup output");
            }
        }
        Ok(CrashCleanupOutcome { worktree: Some(outcome) })
    }
}

/// cas-58a3: the I/O half of [`FactoryApp::mark_worker_crashed`], for a job
/// lane. The interactive leader may have exited while a long-lived child
/// remains in its process group: reap the durable group before the lane is
/// eligible for respawn. Then reclaim the worktree only when all of the
/// worker's tasks are Closed (the supervisor has moved on) and the tree is
/// clean; by default it is preserved for respawn.
pub fn crash_cleanup_io(cleanup: &CrashCleanup) -> CrashCleanupOutcome {
    let name = cleanup.worker.as_str();
    if let Some(record) = crate::ui::factory::process_groups::list(&cleanup.cas_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|record| {
            record.worker_name == name
                && cleanup
                    .factory_session
                    .as_deref()
                    .is_none_or(|session| record.factory_session == session)
        })
    {
        if let Err(error) = crate::ui::factory::process_groups::reap_blocking(&cleanup.cas_dir, &record) {
            tracing::warn!(
                worker = %name,
                pgid = record.pgid,
                error = %error,
                "crashed worker process group survived reap; retaining GC record"
            );
        }
    }

    let Some(plan) = cleanup.worktree.as_ref() else {
        return CrashCleanupOutcome { worktree: None };
    };
    let Ok(agent_store) = open_agent_store(&cleanup.cas_dir) else {
        return CrashCleanupOutcome { worktree: None };
    };
    let Some(agent) = agent_store
        .list(None)
        .ok()
        .and_then(|agents| agents.into_iter().find(|a| a.name == name))
    else {
        return CrashCleanupOutcome { worktree: None };
    };
    let agent_id = agent.id.clone();
    if worker_has_open_tasks(&cleanup.cas_dir, &agent_id) || worker_has_open_tasks(&cleanup.cas_dir, name) {
        return CrashCleanupOutcome { worktree: None };
    }
    // Never orphan relocated output by deleting its checkout/provenance first.
    if crate::factory_scratch::worker_target_dir(&plan.worktree.path).exists()
        && !crate::factory_target_cache::retirement::retire_worker(&cleanup.cas_dir, &agent).unwrap_or(false)
    {
        return CrashCleanupOutcome { worktree: None };
    }
    let outcome = crate::worktree::remove_worker_worktree_io(plan);
    if let Ok(RemoveOutcome::DirtyDeferred(warning)) = &outcome
        && let Err(e) = flag_agent_dirty_on_shutdown(
            agent_store.as_ref(),
            &agent_id,
            &warning.path,
            warning.file_count,
        )
    {
        tracing::warn!("Failed to flag dirty_on_shutdown for agent '{agent_id}': {e}");
    }
    CrashCleanupOutcome {
        worktree: Some(outcome),
    }
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
    match open_task_store_cached(cas_dir) {
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
            tracing::warn!("worker_has_open_tasks: open_task_store_cached failed: {e} — assuming busy");
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
    let store = match open_task_store_cached(cas_dir) {
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

    let store = open_task_store_cached(cas_dir)
        .map_err(|e| format!("could not open task store for stale-holder reset: {e}"))?;
    let mut reset = store
        .get(&task.id)
        .map_err(|e| format!("could not reload task after lease release: {e}"))?;
    if reset.assignee.as_deref() != Some(holder) {
        return Err(format!(
            "task assignee changed from stale holder '{holder}' while preparing reset"
        ));
    }
    cas_store::release_qa_claim_for_reviewer(cas_dir, &task.id, holder)
        .map_err(|e| format!("could not release stale QA reviewer '{holder}' claim: {e}"))?;
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
    store.update(&reset)
        .map_err(|e| format!("could not persist stale-holder reset: {e}"))?;
    // Reset settles only the retired identities captured above. A replacement
    // that wins between persistence and this cloud call must keep its claim.
    let retired_ids: std::collections::HashSet<_> = registered_agents.iter()
        .filter(|agent| agent.name == holder).map(|agent| agent.id.clone()).collect();
    {
        let cas_dir = cas_dir.to_path_buf();
        let task_id = task.id.clone();
        let holder = holder.to_string();
        let release = move || {
            let Some(claims) = crate::cloud::peer_claims::PeerClaims::for_project(&cas_dir) else {
                return;
            };
            if let Err(error) = claims.release_reset_holder(&task_id, &holder, &retired_ids) {
                tracing::debug!(task = %task_id, %error, "stale assignee cloud release failed; start will retry");
            }
        };
        // Config, Git discovery, registry/event reads and network all stay off
        // the factory loop. The exact retired owner
        // remains fenced if task start wins before this best-effort release.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn_blocking(release));
        } else {
            std::thread::spawn(release);
        }
    }
    Ok(())
}

/// Why a worker's process was retired, which decides what happens to the
/// tasks it was bound to (cas-a622).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerRetirement {
    /// The worker is gone for good: park what it held and release its
    /// bindings so another worker can take them.
    Shutdown,
    /// The same name is respawned at once in the same worktree to reset its
    /// context. Assignees are display names (cas-dbbb), so every binding
    /// already belongs to the replacement process.
    Recycle,
}

/// Settle the task bindings of a worker whose process has just been retired.
/// Returns how many bindings were released.
pub(crate) fn settle_retired_worker_bindings(
    cas_dir: &std::path::Path,
    agent_store: &dyn cas_store::AgentStore,
    agent: &cas_types::Agent,
    held_task_ids: &[String],
    retirement: WorkerRetirement,
) -> usize {
    settle_retired_worker_bindings_reporting(cas_dir, agent_store, agent, held_task_ids, retirement)
        .0
}

/// [`settle_retired_worker_bindings`], also returning the task ids the
/// shutdown parked so the shutdown receipt can name them (cas-b06f).
pub(crate) fn settle_retired_worker_bindings_reporting(
    cas_dir: &std::path::Path,
    agent_store: &dyn cas_store::AgentStore,
    agent: &cas_types::Agent,
    held_task_ids: &[String],
    retirement: WorkerRetirement,
) -> (usize, Vec<String>) {
    if retirement == WorkerRetirement::Recycle {
        // cas-a622: parking or releasing here is what cost proud-newt-45 its
        // assignment. The replacement is the same name in the same worktree,
        // so it already owns every binding; graceful_shutdown released only
        // the old process's leases, which `task action=start` re-takes from
        // the resume brief (`enqueue_recycle_resume_brief`).
        tracing::info!(
            worker = %agent.name,
            held = held_task_ids.len(),
            "cas-a622: recycle keeps the worker's task bindings"
        );
        return (0, Vec::new());
    }
    // Park any task held at termination through the same recovery used for
    // unexpected PTY exits (GH #197). The process is already gone, and this
    // runs before the legacy binding cleanup. The supervisor asked for this
    // stop, so it gets one shutdown receipt naming these tasks instead of a
    // Critical death alarm per worker (cas-b06f, spec-6194 P2.6).
    let recovered = crate::mcp::tools::service::orphan_recovery::recover_worker_retired_on_request(
        cas_dir,
        agent_store,
        agent,
        held_task_ids,
        "worker terminated by shutdown request",
    )
    .recovered_task_ids;

    // cas-7a94: clear pure Open pre-assigns and any binding the recovery
    // path could not inspect. Assignees are display names (cas-dbbb), so
    // match on `name` rather than the registration UUID.
    let released = release_worker_task_bindings(cas_dir, &agent.name);
    if released > 0 {
        tracing::info!(
            worker = %agent.name,
            released,
            "cas-7a94: released remaining task bindings on shutdown_worker"
        );
    }
    (released, recovered)
}

/// The brief a recycled worker receives so its fresh conversation resumes its
/// assigned work (cas-a622). `None` when it holds no nonterminal task.
pub(crate) fn recycle_resume_brief(
    cas_dir: &std::path::Path,
    worker_name: &str,
    factory_session: &str,
) -> Option<String> {
    let task_store = open_task_store_cached(cas_dir).ok()?;
    let all_tasks = task_store.list(None).ok()?;
    let mut held: Vec<cas_types::Task> = all_tasks
        .iter()
        .filter(|task| task.assignee.as_deref() == Some(worker_name) && !task.is_terminal())
        .cloned()
        .collect();
    if held.is_empty() {
        return None;
    }
    held.sort_by(|a, b| a.id.cmp(&b.id));
    let mut brief =
        "Your context was reset; your worktree and every task binding were kept.\n"
            .to_string();
    let line = |task: &cas_types::Task| format!("- {} [{}] {}", task.id, task.status, task.title);
    for task in held
        .iter()
        .filter(|task| task.status == cas_types::TaskStatus::InProgress)
    {
        brief.push_str(&format!(
            "{}\n  Resume it: run `task action=start id={}` to re-take its lease, read its notes with \
             `task action=notes id={}`, then continue from the last checkpoint.\n",
            line(task),
            task.id,
            task.id
        ));
    }
    let parked: Vec<String> = held
        .iter()
        .filter(|task| task.status == cas_types::TaskStatus::AwaitingMerge)
        .map(line)
        .collect();
    if !parked.is_empty() {
        brief.push_str(&format!(
            "Awaiting merge (re-close each after its merge lands; do not start it):\n{}\n",
            parked.join("\n")
        ));
    }
    let other: Vec<String> = held
        .iter()
        .filter(|task| {
            !matches!(
                task.status,
                cas_types::TaskStatus::InProgress | cas_types::TaskStatus::AwaitingMerge
            )
        })
        .map(line)
        .collect();
    if !other.is_empty() {
        brief.push_str(&format!(
            "Also assigned to you (start only when the supervisor directs):\n{}\n",
            other.join("\n")
        ));
    }
    append_recycle_received_messages(
        cas_dir,
        worker_name,
        factory_session,
        &held,
        &all_tasks,
        &mut brief,
    );
    Some(brief)
}

/// Notes have minute precision. Include that whole minute to avoid losing an
/// answer received after a note in the same minute. Lifecycle `updated_at` is
/// not a checkpoint: a reclaim/start may have just advanced it.
fn recycle_task_checkpoint(task: &cas_types::Task) -> chrono::DateTime<chrono::Utc> {
    task.notes
        .lines()
        .filter_map(|line| {
            let (timestamp, _) = line.strip_prefix('[')?.split_once(']')?;
            chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M")
                .ok()
                .map(|at| at.and_utc())
        })
        .max()
        .unwrap_or(task.created_at)
}

fn append_recycle_received_messages(
    cas_dir: &std::path::Path,
    worker_name: &str,
    factory_session: &str,
    held: &[cas_types::Task],
    all_tasks: &[cas_types::Task],
    brief: &mut String,
) {
    let checkpoints: Vec<_> = held
        .iter()
        .map(|task| (task.id.as_str(), recycle_task_checkpoint(task)))
        .collect();
    let Some(since) = checkpoints.iter().map(|(_, at)| *at).min() else {
        return;
    };
    let known_tasks: std::collections::HashSet<_> =
        all_tasks.iter().map(|task| task.id.as_str()).collect();
    let history = cas_store::SqlitePromptQueueStore::open_read_only(cas_dir).and_then(|queue| {
        queue.received_supervisor_messages_since_matching(
            worker_name,
            factory_session,
            since,
            8,
            |row, received_at| {
                // Assignment envelopes bind even unknown tasks. Generic replies
                // use this worker's checkpoint window; known task mentions use
                // only the corresponding bound-task windows.
                let assignment =
                    crate::prompt_revalidation::assignment_solicited_task_id(&row.prompt);
                let tokens: std::collections::HashSet<_> = row
                    .prompt
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .collect();
                let mentions_task = tokens.iter().any(|token| known_tasks.contains(token));
                checkpoints
                    .iter()
                    .filter(|(id, _)| {
                        if let Some(assignment) = assignment.as_deref() {
                            *id == assignment
                        } else {
                            !mentions_task || tokens.contains(id)
                        }
                    })
                    .map(|(_, at)| *at)
                    .min()
                    .is_some_and(|cutoff| received_at >= cutoff)
            },
        )
    });
    let history = match history {
        Ok(history) => history,
        Err(error) => {
            tracing::warn!(worker = worker_name, %error, "recycle message history unavailable");
            brief.push_str("Recent supervisor message history could not be read; check `coordination action=inbox_poll` and task notes before asking again.\n");
            return;
        }
    };
    let intro = "Recent received supervisor messages (newest first; up to 8). These answers survived the reset; read them before repeating a question.\n";
    let mut included = 0;
    let mut remaining = 12_000usize - intro.len();
    for (row, received_at) in history {
        let header = format!(
            "#{} from {} (received {}):\n",
            row.id,
            row.source,
            received_at.to_rfc3339()
        );
        let body = if header.len() + row.prompt.len() + 1 <= remaining {
            format!("{header}{}\n", row.prompt)
        } else {
            format!(
                "#{}: message exceeds remaining brief budget; read the complete answer with `coordination action=inbox_poll notification_id={}`.\n",
                row.id, row.id
            )
        };
        if body.len() > remaining {
            break;
        }
        if included == 0 {
            brief.push_str(intro);
        }
        remaining -= body.len();
        brief.push_str(&body);
        included += 1;
    }
}

/// Queue [`recycle_resume_brief`] to the recycled worker. Returns the prompt
/// id, or `None` when there was nothing to resume.
pub(crate) fn enqueue_recycle_resume_brief(
    cas_dir: &std::path::Path,
    supervisor_name: &str,
    factory_session: &str,
    worker_name: &str,
) -> anyhow::Result<Option<i64>> {
    enqueue_context_resume_brief(cas_dir, supervisor_name, Some(factory_session), worker_name)
}

/// Shared by daemon recycle completion and a confirmed in-place context reset.
/// Legacy callers without a factory session still receive their task brief;
/// received-message history requires an authenticated session.
pub(crate) fn enqueue_context_resume_brief(
    cas_dir: &std::path::Path,
    supervisor_name: &str,
    factory_session: Option<&str>,
    worker_name: &str,
) -> anyhow::Result<Option<i64>> {
    let Some(brief) =
        recycle_resume_brief(cas_dir, worker_name, factory_session.unwrap_or_default())
    else {
        return Ok(None);
    };
    let queue = crate::store::open_prompt_queue_store(cas_dir)?;
    let id = queue.enqueue_with_summary(
        supervisor_name,
        worker_name,
        &brief,
        factory_session,
        Some("Context reset: resume your assigned work"),
    )?;
    Ok(Some(id))
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
    let task_store = match open_task_store_cached(cas_dir) {
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
            return release_worker_task_bindings_tasks_only(
                cas_dir,
                &*task_store,
                worker_name,
                None,
            );
        }
    };

    release_worker_task_bindings_tasks_only(cas_dir, &*task_store, worker_name, Some(&*agent_store))
}

fn release_worker_task_bindings_tasks_only(
    cas_dir: &std::path::Path,
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
        if let Err(e) = cas_store::release_qa_claim_for_reviewer(cas_dir, &t.id, worker_name) {
            tracing::error!(task_id = %t.id, worker_name, error = %e,
                "cas-3172: QA claim release failed; retaining shutdown task binding for retry");
            continue;
        }
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
    let store = match open_task_store_cached(cas_dir) {
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
    if let Err(e) = cas_store::release_qa_claim_for_reviewer(cas_dir, task_id, worker_name) {
        tracing::error!(task_id, worker_name, error = %e,
            "cas-3172: QA claim release failed; retaining aborted preassignment for retry");
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

/// Retire a stopped worker's identity on the daemon's store worker, which
/// retries without the loop's wait budget (cas-00d5). `graceful_shutdown` is
/// idempotent, so a retry after a partial failure is safe.
fn defer_worker_retirement(cas_dir: &std::path::Path, agent_id: &str, first_error: &str) {
    let cas_dir = cas_dir.to_path_buf();
    let agent_id = agent_id.to_string();
    crate::ui::factory::daemon::runtime::store_worker::defer(
        "retire shut-down worker",
        first_error,
        move || {
            open_agent_store(&cas_dir)
                .map_err(|error| error.to_string())?
                .graceful_shutdown(&agent_id)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    );
}

/// What one shutdown request did to each targeted worker (cas-b06f): the
/// basis of the single receipt the supervisor gets instead of one death alarm
/// per worker (spec-6194 P2.6).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ShutdownReport {
    /// Workers stopped, each with the task ids its shutdown parked.
    pub stopped: Vec<(String, Vec<String>)>,
    /// Workers that could not be stopped, each with the reason.
    pub failed: Vec<(String, String)>,
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


impl WorkerSpawnContext {
    pub(crate) fn resolve(mut self) -> anyhow::Result<WorkerSpawnPrep> {
        if self.isolate {
            match DirectorData::load_fast(&self.cas_dir) {
                Ok(data) => {
                    let focus = self.factory_session.as_deref()
                        .map(crate::ui::factory::app::preferred_epic_focus_from_session_metadata_named)
                        .unwrap_or_default();
                    if self.task_id.is_none() && let Some(id) = focus.epic_id.as_deref() {
                        let store = open_task_store_cached(&self.cas_dir)?;
                        match store.get(id) {
                            Ok(epic) if epic.is_terminal() => anyhow::bail!("spawn refused: pinned epic {id} is {}; no lane was republished", epic.status),
                            Err(_) => anyhow::bail!("spawn refused: pinned epic {id} no longer exists; no lane was republished"),
                            _ => {},
                        }
                    }
                    let state =
                        crate::ui::factory::app::resolve_epic_state_for_focus(&data, &focus);
                    self.current_epic_id = state.epic_id().map(str::to_string);
                    self.epic_branch =
                        crate::ui::factory::app::epic_branch_for_state(&data, &state);
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to refresh spawn focus; using captured focus")
                }
            }
        }
        let worker_name = self.worker_name.clone();
        let task_id = self.task_id.as_deref();
        let isolate = self.isolate;
        let (worktree_info, base_warnings, base_provenance) = if isolate {
            if let Some(session_repo_root) = &self.worktree_repo_root {
                // Re-resolve the repository on every request. A daemon started
                // before `git init` may have latched an ancestor repository;
                // continuing with that stale root would create worker branches
                // in the wrong project. The verified-spawn lifecycle surfaces
                // this per-request failure to the supervisor.
                validate_live_spawn_repo_root(session_repo_root, &self.project_path)?;
                // Verify repo has commits before trying to create worktrees
                if !crate::worktree::GitOperations::new(session_repo_root.clone())
                    .has_commits()
                    .unwrap_or(false)
                {
                    crate::telemetry::track(
                        "factory_worker_spawn_result",
                        vec![("success", "false"), ("reason", "repo_has_no_commits")],
                    );
                    anyhow::bail!(
                        "Repository has no commits. Please make an initial commit before spawning workers."
                    );
                }

                let session_repo_root = session_repo_root.clone();
                let task_base = task_id
                    .map(|tid| task_epic_base(&self.cas_dir, &session_repo_root, tid))
                    .unwrap_or(TaskBase::Unresolved);
                if let Some(epic) = task_base.epic()
                    && epic.work_target.is_none()
                    && (epic.branch.is_empty() || !epic.branch_exists)
                {
                    anyhow::bail!(
                        "spawn refused: task {} belongs to epic {} but the epic has no recorded WorkTarget or resolvable branch; refusing to recompute a title-derived base",
                        epic.task_id,
                        epic.epic_id,
                    );
                }
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
                    self.worktree_root
                        .as_ref()
                        .expect("worktree root snapshot")
                        .join(&worker_name)
                };
                let branch_name = format!("factory/{worker_name}");
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
                let recorded_epic = open_task_store_cached(&self.cas_dir)?.epic_for_branch(&parent_branch)?;
                let managed_epic = parent_branch.starts_with("epic/") && recorded_epic.is_some();
                if let Some(epic) = &recorded_epic
                    && epic.is_terminal()
                {
                    anyhow::bail!(
                        "spawn refused: epic {} is {}; no lane was republished",
                        epic.id,
                        epic.status
                    );
                }
                if managed_epic {
                    // Adopted self-target lanes and legacy target-less epics have
                    // no recorded parent to refresh. Publish their local tip as
                    // it is, even if it is an ancestor of the configured trunk.
                    let refresh_parent = recorded_epic
                        .as_ref()
                        .and_then(|epic| epic.deliverables.work_target.as_ref())
                        .map(|target| target.target_branch.trim())
                        .filter(|parent| !parent.is_empty() && *parent != parent_branch);
                    let observation = spawn_git.observe_epic_publication(&parent_branch, refresh_parent);
                    let existing_remote = matches!(observation.remote, Ok(Some(true)));
                    let lane_origin = recorded_epic.as_ref()
                        .and_then(|epic| epic.deliverables.epic_branch_origin.as_ref());
                    if !ref_exists(&repo_root, &parent_branch) && !existing_remote {
                        if let Err(error) = &observation.remote {
                            anyhow::bail!(
                                "EPIC BRANCH UNVERIFIED: '{parent_branch}' cannot be prepared because origin could not be verified: {error}; no lane was recreated"
                            );
                        }
                        anyhow::bail!(
                            "{}", crate::worktree::GitOperations::missing_epic_branch_message(
                                &parent_branch, lane_origin, observation.previous_remote,
                            )
                        );
                    }
                    // Reconcile a verified existing epic once before preparing from
                    // its parent, so a stale local copy cannot omit origin's work.
                    let preserve = if existing_remote {
                        let readiness = spawn_git
                            .ensure_epic_published_observed(&parent_branch, refresh_parent, &observation, lane_origin)
                            .map_err(|error| anyhow::anyhow!("{}", epic_base_refresh_refusal(&error)))?;
                        if let Some(notice) = readiness.notice {
                            notices.push(notice);
                        }
                        !readiness.can_refresh_parent
                    } else {
                        false
                    };
                    if let Some(parent) = refresh_parent {
                        if let Some(parent_ref) = spawn_git
                            .shared_parent_ref(parent)
                            .map_err(|error| anyhow::anyhow!("{}", epic_base_refresh_refusal(&error)))?
                        {
                            if preserve {
                                // Remote divergence forbids moving either side, but local
                                // ancestry validation still uses the fetched/cached parent.
                                let ahead = spawn_git.commits_behind(&parent_ref, &parent_branch)?;
                                let behind = spawn_git.commits_behind(&parent_branch, &parent_ref)?;
                                if ahead > 0 && behind > 0 {
                                    anyhow::bail!(
                                        "{}",
                                        epic_base_refresh_refusal(&format!(
                                            "epic base '{parent_branch}' and recorded parent '{parent_ref}' have diverged; no refs changed. Reconcile: `{}`{}",
                                            epic_reconcile_command(&repo_root, &parent_branch, &parent_ref).map_err(anyhow::Error::msg)?,
                                            spawn_git.reconcile_commit_cause(&parent_ref, &parent_branch),
                                        ))
                                    );
                                }
                            } else {
                                let publication = if existing_remote {
                                    EpicBasePublication::Published
                                } else if matches!(observation.remote, Ok(None) | Ok(Some(false))) && !observation.previous_remote {
                                    EpicBasePublication::Unpublished
                                } else {
                                    EpicBasePublication::Unverified
                                };
                                if let Some(notice) = refresh_epic_base_from_parent(
                                    &repo_root,
                                    &parent_branch,
                                    &parent,
                                    &parent_ref,
                                    publication,
                                    || epic_has_no_task_deliveries(&self.cas_dir, recorded_epic.as_ref().expect("managed epic"))
                                        .map_err(|error| error.to_string()),
                                )
                                .map_err(|error| anyhow::anyhow!("{}", epic_base_refresh_refusal(&error)))?
                                {
                                    notices.push(notice);
                                }
                            }
                        } else {
                            notices.push(format!("EPIC BASE REFRESH SKIPPED: '{parent_branch}' recorded parent '{parent}' has no resolvable local or cached origin ref; using the existing epic tip unchanged."));
                        }
                    }
                    if !existing_remote {
                        match spawn_git.ensure_epic_published_observed(
                            &parent_branch,
                            refresh_parent,
                            &observation,
                            lane_origin,
                        ) {
                            Ok(readiness) => {
                                if let Some(notice) = readiness.notice {
                                    notices.push(notice);
                                }
                            }
                            Err(error) => notices.push(format!(
                                "EPIC BRANCH UNVERIFIED: '{parent_branch}' ({}); {error}",
                                full_sha(&repo_root, &parent_branch).unwrap_or_else(|| "missing".into())
                            )),
                        }
                    }
                } else if let Some(notice) = ensure_local_spawn_parent(&repo_root, &parent_branch)
                    .map_err(|error| anyhow::anyhow!("{error}"))?
                {
                    notices.push(notice);
                }
                let (base_ref, freshness_notice, checkout_ref) =
                    checkout_ref_for_spawn_base_observed(&repo_root, &parent_branch, &base_source, !managed_epic)
                        .map_err(anyhow::Error::msg)?;
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

        Ok(WorkerSpawnPrep {
            worker_name,
            worktree_info,
            warnings: base_warnings,
            base_provenance,
        })
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
    /// Capture only in-memory spawn state. All store, config and Git work is
    /// resolved by the cancellable provisioner, never on the daemon loop.
    pub(crate) fn snapshot_worker_spawn(
        &self,
        name: Option<&str>,
        isolate: bool,
        task_id: Option<&str>,
    ) -> anyhow::Result<WorkerSpawnContext> {
        let spawn_type = if name.is_some() { "named" } else { "anonymous" };
        crate::telemetry::track(
            "factory_worker_spawn_requested",
            vec![
                ("spawn_type", spawn_type),
                ("worktrees_enabled", bool_prop(self.worktrees_enabled())),
                ("isolate", bool_prop(isolate)),
            ],
        );
        let worker_name = match name {
            Some(name) => name.to_string(),
            None => {
                let existing: std::collections::HashSet<&str> = self
                    .worker_names
                    .iter()
                    .chain(self.crash_cleanups_pending.keys())
                    .map(String::as_str)
                    .collect();
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
        if self.crash_cleanup_pending(&worker_name) {
            anyhow::bail!(
                "Worker '{worker_name}' is still being cleaned up after its exit; retry once its reap and worktree check finish"
            );
        }
        Ok(WorkerSpawnContext {
            worker_name,
            spawn_type: spawn_type.into(),
            isolate,
            task_id: task_id.map(str::to_string),
            project_path: self.project_path().to_path_buf(),
            cas_dir: self.cas_dir.clone(),
            worktree_repo_root: self
                .worktree_manager
                .as_ref()
                .map(|m| m.repo_root().to_path_buf()),
            worktree_root: self.worktree_manager.as_ref().map(|m| m.worktree_root()),
            epic_branch: self.epic_branch.clone(),
            current_epic_id: self.current_epic_id.clone(),
            factory_session: self
                .factory_session
                .clone()
                .or_else(|| std::env::var("CAS_FACTORY_SESSION").ok()),
        })
    }

    pub fn prepare_worker_spawn(
        &mut self,
        name: Option<&str>,
        isolate: bool,
        task_id: Option<&str>,
    ) -> anyhow::Result<WorkerSpawnPrep> {
        let prep = self
            .snapshot_worker_spawn(name, isolate, task_id)?
            .resolve()?;
        crate::telemetry::track(
            "factory_worker_spawn_prepared",
            vec![
                (
                    "spawn_type",
                    if name.is_some() { "named" } else { "anonymous" },
                ),
                ("worktrees_enabled", bool_prop(prep.worktree_info.is_some())),
            ],
        );
        for notice in &prep.warnings {
            self.set_error(notice.clone());
        }
        Ok(prep)
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
    /// Returns the task ids the shutdown parked (cas-b06f receipt).
    pub async fn shutdown_worker(&mut self, name: &str, force: bool) -> anyhow::Result<Vec<String>> {
        self.shutdown_worker_inner(name, force, false).await
    }

    /// Stop a worker while retaining its existing isolated worktree for an
    /// immediate in-place recycle. Ordinary shutdowns reclaim a clean tree
    /// once all tasks are terminal; recycling must preserve that path so the
    /// replacement starts from the same checkout and branch.
    pub async fn shutdown_worker_for_recycle(
        &mut self,
        name: &str,
    ) -> anyhow::Result<()> {
        self.shutdown_worker_inner(name, false, true).await.map(|_| ())
    }

    async fn shutdown_worker_inner(
        &mut self,
        name: &str,
        force: bool,
        preserve_worktree: bool,
    ) -> anyhow::Result<Vec<String>> {
        // cas-00d5 (GH #1190, #1195): the daemon runs this inside a loop pass
        // whose store-wait budget earlier work has often spent. Every store
        // call below then failed fast, retirement bailed, and the worker kept
        // running. The shutdown gets its own bounded budget instead.
        let _store_budget = cas_store::wait_budget::grant_waits_for(
            crate::ui::factory::daemon::runtime::store_worker::SHUTDOWN_STORE_WAIT_BUDGET,
        );
        // Shutdown authorization must not queue behind a background writer's
        // process-shared connection mutex. Read the same committed registry on
        // an independent connection (no schema writes). Ordinary dead-pane
        // cleanup is deferred; recycle retires the old identity before relaunch.
        let agent_store: std::sync::Arc<dyn cas_store::AgentStore> = std::sync::Arc::new(
            cas_store::SqliteAgentStore::with_connection(
                cas_store::shared_db::dedicated_connection(&self.cas_dir().join("cas.db"))?,
            ),
        );
        let agents = agent_store.list(None)?;
        // Crash/recycle handling can remove the pane before shutdown arrives.
        // A dead registration in our session is still a valid cleanup target.
        if !self.worker_names.contains(&name.to_string()) {
            let matching: Vec<_> = agents
                .iter()
                .filter(|agent| {
                    agent.name == name
                        && agent.role == cas_types::AgentRole::Worker
                        && agent.visible_to_factory_session(self.factory_session.as_deref())
                })
                .collect();
            if !matching.is_empty()
                && !crate::mcp::tools::service::agent_liveness::has_live_agent_named(matching.iter().copied(), name)
            {
                if preserve_worktree {
                    // Recycle must finish retiring the old identities before
                    // launching the same-name replacement. Deferred shutdown
                    // cleanup could cancel its resume brief before registration.
                    // Like a live-pane recycle, release only the old leases:
                    // task bindings and unread mail belong to the replacement.
                    let agent = (*matching[0]).clone();
                    let tasks = crate::factory_target_cache::retirement::snapshot_worker_tasks(self.cas_dir(), &agent);
                    for agent in matching {
                        if let Err(error) = agent_store.graceful_shutdown(&agent.id) {
                            defer_worker_retirement(self.cas_dir(), &agent.id, &error.to_string());
                        }
                    }
                    self.queue_worker_cleanup(&agent_store, agent, tasks, false);
                    self.last_db_fingerprint = None;
                    return Ok(Vec::new());
                }
                // The process is already dead. Inline notification/identity
                // cleanup can spend this worker's entire budget and falsely
                // report a failed stop. Revalidate on the store worker before
                // cleanup, so a live replacement keeps its identity and mail.
                let cas_dir = self.cas_dir().to_path_buf();
                let worker_name = name.to_string();
                let session = self.factory_session.clone();
                let queued = crate::ui::factory::daemon::runtime::store_worker::submit_write(
                    "retire dead worker after shutdown",
                    move || {
                        crate::mcp::tools::service::factory_ops::retire_dead_worker_for_shutdown(
                            &cas_dir,
                            &worker_name,
                            session.as_deref(),
                        )
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                    },
                );
                if queued == crate::ui::factory::daemon::runtime::store_worker::Persisted::Dropped {
                    anyhow::bail!("Dead worker '{name}' cleanup could not be queued");
                }
                self.last_db_fingerprint = None;
                return Ok(Vec::new());
            }
            anyhow::bail!("Worker '{name}' not found");
        }

        let cas_dir = self.cas_dir().to_path_buf();

        // Mark agent as shutdown in Cassy first; this must succeed so supervisor sees errors
        // instead of silently leaving stale idle agents in director panels.
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

        // SessionEnd can delete these registrations before the final director
        // refresh. Keep the exact session-scoped IDs now so reminder cleanup
        // does not have to recover them from a name-only snapshot afterward.
        let matching_agent_ids = matching_agents
            .iter()
            .map(|matching| matching.id.clone())
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
                // cas-00d5: a busy store must never keep the worker alive.
                // The process is stopped below; its retirement lands on the
                // store worker, which waits out the contention.
                defer_worker_retirement(&cas_dir, &matching.id, &error.to_string());
            }
        }
        if !retirement_failures.is_empty() {
            let detail = format!(
                "Worker '{name}' is being stopped, but retiring its Cassy identity is deferred until the store frees up: {}",
                retirement_failures.join("; ")
            );
            tracing::warn!(worker = name, "{detail}");
            self.set_error(detail);
        }

        // Kill the worker's PTY process tree and remove from mux (cas-8c5a).
        // kill_worker signals the entire process group (SIGKILL when force=true,
        // SIGTERM when false), ensuring the full node→codex tree is terminated,
        // not just the direct PTY child that SIGHUP-on-drop would reach.
        let process_group = self.mux.pane_process_group_id(name);
        // GH #1363: this runs on the daemon loop. A teardown that never
        // returned here silenced the whole factory, so it runs off the loop
        // and a stall is named instead.
        if let cas_mux::WorkerTeardown::Stalled {
            stage,
            process_group,
        } = self
            .mux
            .kill_worker_within(name, force, WORKER_TEARDOWN_DEADLINE)
            .await?
        {
            let detail = format!(
                "Worker '{name}' teardown stalled in {stage} (process group {}) for {}s; \
                 the daemon continues and the teardown thread finishes when the call returns",
                process_group.map_or_else(|| "unknown".to_string(), |pgid| pgid.to_string()),
                WORKER_TEARDOWN_DEADLINE.as_secs(),
            );
            tracing::error!(worker = name, stage, ?process_group, "GH #1363: {detail}");
            if let Some(session) = self.factory_session.as_deref() {
                crate::ui::factory::record_daemon_lifecycle(session, "teardown_stalled", &detail);
            }
            self.set_error(detail);
        }
        if let Some(pgid) = process_group {
            self.untrack_worker_process_group_if_gone(pgid).await;
        }

        // Reclaim safe terminal/parked output for shutdown AND replacement.
        // Check before settling bindings: that step can clear task assignees.
        // Retirement checks the configured scratch path and preserves evidence
        // before deleting; no absent in-tree target can stand in for success.
        let retirement_tasks = if agent.metadata.contains_key("clone_path") {
            crate::factory_target_cache::retirement::snapshot_worker_tasks(&cas_dir, agent)
        } else { Ok(Vec::new()) };

        // Only after the process is actually gone: a shutdown parks and
        // releases what the worker held; a recycle keeps it (cas-a622).
        let (_, released_task_ids) = settle_retired_worker_bindings_reporting(
            &cas_dir,
            agent_store.as_ref(),
            agent,
            &held_task_ids,
            if preserve_worktree {
                WorkerRetirement::Recycle
            } else {
                WorkerRetirement::Shutdown
            },
        );

        // Remove from tracking
        self.worker_names.retain(|n| n != name);

        // Force a DB reload next refresh; relying only on mtime can miss rapid same-second writes.
        self.last_db_fingerprint = None;
        // Refresh director data immediately so UI shows updated state
        let _ = self.refresh_data();

        // Update event detector
        self.remove_worker_from_event_detector_with_known_ids(name, &matching_agent_ids);

        // Update pane grid for navigation
        self.pane_grid = PaneGrid::new(&self.worker_names, &self.supervisor_name, self.is_tabbed);

        // Ensure selected tab is still valid
        self.clamp_selected_worker_tab();

        // Teardown the worker's worktree when it's safe. "Safe" means all of its
        // tasks are Closed AND the tree is clean — we never destroy in-progress
        // work. Dirty trees are preserved for the daemon reaper (Unit 3) to
        // salvage later, and we flag the agent record + warn the supervisor so
        // nothing is silently abandoned.
        self.queue_worker_cleanup(&agent_store, agent.clone(), retirement_tasks,
            crate::worktree::should_finalize_worker_worktree(preserve_worktree, has_open_tasks));

        // Sync pane sizes to adjust layout
        let _ = self.sync_pane_sizes();

        Ok(released_task_ids)
    }

    /// Queue target retirement and optional worktree removal after shutdown.
    /// Git, evidence copies and deletion must not hold the control loop.
    /// Reserve the name until its outcome lands, just as crash cleanup does.
    fn queue_worker_cleanup(
        &mut self,
        _agent_store: &std::sync::Arc<dyn cas_store::AgentStore>,
        agent: cas_types::Agent,
        tasks: std::io::Result<Vec<cas_types::Task>>,
        remove_worktree: bool,
    ) {
        if !agent.metadata.contains_key("clone_path") { return; }
        let tasks = match tasks {
            Ok(tasks) => tasks,
            Err(error) => {
                tracing::warn!(%error, worker = %agent.name, "cleanup snapshot unavailable; output retained");
                return;
            }
        };
        let name = agent.name.clone();
        let plan = remove_worktree.then(|| self.worktree_manager.as_ref()
            .and_then(|manager| manager.worker_removal_plan(&name))).flatten();
        self.start_worker_cleanup(DeferredWorkerCleanup {
            occurrence: uuid::Uuid::new_v4().to_string(), agent, tasks,
            worktree: plan.map(|plan| (plan.worktree, plan.repo_root)),
        });
    }

    fn start_worker_cleanup(&mut self, request: DeferredWorkerCleanup) {
        let name = request.agent.name.clone();
        let cas_dir = self.cas_dir().to_path_buf();
        let recovery_path = request.path(&cas_dir);
        let occurrence = request.occurrence.clone();
        let started = std::thread::Builder::new()
            .name(format!("worktree-cleanup-{name}"))
            .spawn(move || {
                // Commit the recovery intent before any probes, copies or deletion.
                if let Err(error) = request.persist(&cas_dir) {
                    tracing::error!(%error, worker = %request.agent.name, "cleanup intent could not be persisted; output retained");
                    return CrashCleanupOutcome { worktree: None };
                }
                match request.run(&cas_dir) {
                    Ok(outcome) => {
                        if !matches!(&outcome.worktree, Some(Err(_))) {
                            if let Err(error) = std::fs::remove_file(request.path(&cas_dir)) {
                                tracing::warn!(%error, "completed cleanup intent could not be removed");
                            }
                        }
                        outcome
                    }
                    Err(error) => {
                        tracing::warn!(%error, worker = %request.agent.name, "cleanup deferred; recovery intent retained");
                        CrashCleanupOutcome { worktree: None }
                    }
                }
            });
        match started {
            Ok(handle) => {
                let mut job = WorkerCleanupJob::new(name, handle);
                job.occurrence = occurrence;
                job.recovery_path = Some(recovery_path);
                self.register_worker_cleanup(job);
            }
            Err(error) => {
                self.set_error(format!("Worker '{name}' worktree cleanup failed: {error}"));
            }
        }
    }

    /// Run before accepting spawn controls, restoring the old name fences first.
    pub(crate) fn resume_deferred_worker_cleanups(&mut self) {
        let directory = self.cas_dir().join("worker-cleanups");
        if !std::fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.file_type().is_dir()) { return; }
        for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) { continue; }
            let request = std::fs::read(entry.path()).ok()
                .and_then(|bytes| serde_json::from_slice::<DeferredWorkerCleanup>(&bytes).ok());
            let Some(request) = request else {
                tracing::warn!(path = %entry.path().display(), "invalid deferred cleanup intent retained");
                continue;
            };
            if uuid::Uuid::parse_str(&request.occurrence).is_err()
                || request.path(self.cas_dir()) != entry.path()
                || self.crash_cleanup_pending(&request.agent.name)
            { continue; }
            self.start_worker_cleanup(request);
        }
    }

    /// Called only after the exit budget. Keep the name fenced for this daemon's
    /// remaining teardown; persisted intents restore that fence in the next run.
    pub(crate) fn detach_unfinished_worker_cleanups(&mut self) -> Vec<String> {
        self.poll_worker_worktree_cleanups();
        let mut unfinished = Vec::new();
        for job in std::mem::take(&mut self.worktree_cleanup_jobs) {
            if job.handle.is_finished() { self.apply_worker_cleanup(job); continue; }
            tracing::warn!(worker = %job.worker, recovery = ?job.recovery_path,
                "daemon cleanup deadline reached; unfinished cleanup detached for recovery");
            unfinished.push(job.worker);
            // Dropping the JoinHandle detaches it; its intent remains until completion.
        }
        unfinished
    }

    pub(crate) fn register_worker_cleanup(&mut self, job: WorkerCleanupJob) {
        *self.crash_cleanups_pending.entry(job.worker.clone()).or_default() += 1;
        self.worktree_cleanup_jobs.push(job);
    }

    pub(crate) fn join_worker_worktree_cleanups(&mut self) {
        for job in std::mem::take(&mut self.worktree_cleanup_jobs) {
            self.apply_worker_cleanup(job);
        }
    }

    pub(crate) fn worker_cleanup_threads_running(&self) -> bool {
        self.worktree_cleanup_jobs.iter().any(|job| !job.handle.is_finished())
    }

    /// A warning is spent only after its durable write is admitted.
    pub(crate) fn warn_worker_cleanups_at(
        &mut self,
        now: Instant,
        mut warn: impl FnMut(&str, &str, std::time::Duration) -> bool,
    ) {
        for job in &mut self.worktree_cleanup_jobs {
            let elapsed = now.saturating_duration_since(job.started);
            if !job.warned && !job.handle.is_finished() && elapsed >= WORKER_CLEANUP_WARN_AFTER {
                job.warned = warn(&job.worker, &job.occurrence, elapsed);
            }
        }
    }

    pub(crate) fn worker_worktree_cleanups_pending(&self) -> bool {
        !self.worktree_cleanup_jobs.is_empty()
    }

    /// Apply finished removals without waiting for Git or filesystem work.
    pub(crate) fn poll_worker_worktree_cleanups(&mut self) {
        let mut pending = Vec::new();
        for job in std::mem::take(&mut self.worktree_cleanup_jobs) {
            if job.handle.is_finished() { self.apply_worker_cleanup(job); }
            else { pending.push(job); }
        }
        self.worktree_cleanup_jobs = pending;
    }

    fn apply_worker_cleanup(&mut self, job: WorkerCleanupJob) {
        match job.handle.join() {
            Ok(outcome) => self.apply_crash_cleanup(&job.worker, outcome),
            Err(_) => {
                self.abandon_crash_cleanup(&job.worker);
                self.set_error(format!("Worker '{}' worktree cleanup failed: cleanup thread panicked", job.worker));
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
        let cleanup = self.mark_worker_crashed_in_memory(name);
        let outcome = crash_cleanup_io(&cleanup);
        self.apply_crash_cleanup(name, outcome);
    }

    /// cas-58a3: the loop half of [`Self::mark_worker_crashed`]. It drops the
    /// worker from the roster, event detector, pane grid and layout, and
    /// returns what [`crash_cleanup_io`] needs: the process-group reap and the
    /// worktree reclaim check, which must not run inside the daemon loop's
    /// enforced PTY exchange (C6).
    pub fn mark_worker_crashed_in_memory(&mut self, name: &str) -> CrashCleanup {
        // cas-58a3 review H1: the name stays reserved until its cleanup is
        // applied (or abandoned), so no respawn shares its scope or worktree.
        *self.crash_cleanups_pending.entry(name.to_string()).or_default() += 1;
        // Remove from worker tracking
        self.worker_names.retain(|n| n != name);

        // Update event detector (suppresses future events from this worker)
        self.remove_worker_from_event_detector(name);

        // Update pane grid for navigation
        self.pane_grid = PaneGrid::new(&self.worker_names, &self.supervisor_name, self.is_tabbed);

        // Ensure selected tab is still valid
        self.clamp_selected_worker_tab();

        // Sync pane sizes to adjust layout
        let _ = self.sync_pane_sizes();

        let workers_remaining = self.worker_names.len().to_string();
        crate::telemetry::track(
            "factory_worker_crashed",
            vec![("workers_remaining", &workers_remaining)],
        );
        CrashCleanup {
            cas_dir: self.cas_dir().to_path_buf(),
            factory_session: self.factory_session().map(str::to_string),
            worker: name.to_string(),
            worktree: self
                .worktree_manager
                .as_ref()
                .and_then(|manager| manager.worker_removal_plan(name)),
        }
    }

    /// cas-58a3 review H1: whether `name`'s crash cleanup is still pending.
    /// Respawn and named spawn treat such a name as busy.
    pub fn crash_cleanup_pending(&self, name: &str) -> bool {
        self.crash_cleanups_pending.contains_key(name)
    }

    /// cas-58a3 review H1: release a name whose crash cleanup will never be
    /// applied (it was given up), so the name can be spawned again.
    pub fn abandon_crash_cleanup(&mut self, name: &str) {
        self.release_crash_cleanup(name);
    }

    /// cas-58a3 review L-a: one cleanup of `name` finished. The name stays
    /// reserved while another crash cleanup of the same worker is pending
    /// (a second exit of a respawn that raced the first cleanup).
    fn release_crash_cleanup(&mut self, name: &str) {
        if let Some(count) = self.crash_cleanups_pending.get_mut(name) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.crash_cleanups_pending.remove(name);
            }
        }
    }

    /// cas-58a3: the loop half after [`crash_cleanup_io`] reports: forget a
    /// reclaimed worktree, and surface a preserved or failed one.
    pub fn apply_crash_cleanup(&mut self, name: &str, outcome: CrashCleanupOutcome) {
        self.release_crash_cleanup(name);
        match outcome.worktree {
            None | Some(Ok(RemoveOutcome::NotTracked)) => {}
            Some(Ok(RemoveOutcome::Removed)) => {
                if let Some(manager) = self.worktree_manager.as_mut() {
                    manager.forget_worker(name);
                }
            }
            Some(Err(error)) => {
                self.set_error(format!("Worker '{name}' worktree cleanup failed: {error}"));
            }
            Some(Ok(RemoveOutcome::DirtyDeferred(warning))) => {
                self.set_error(format!(
                    "Worker '{}' shut down with {} uncommitted file{} at {} — worktree preserved for salvage",
                    warning.worker_name,
                    warning.file_count,
                    if warning.file_count == 1 { "" } else { "s" },
                    warning.path.display(),
                ));
            }
            Some(Ok(RemoveOutcome::ExternalSymlinksBlocked(warning))) => {
                // cas-df97: live external symlinks (e.g. dotfiles a
                // stow/install step pointed at this worktree by mistake)
                // resolve into it — removing the directory would leave every
                // one dangling. Preserve the worktree and name every
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

    /// Resolve the same relaunch directory before recycle stops the old pane.
    fn worker_respawn_cwd(&mut self, name: &str) -> anyhow::Result<PathBuf> {
        // Check if worktree exists. If it has to be created, use the same base
        // selection as normal worker spawn.
        let cwd = if let Some(manager) = &mut self.worktree_manager {
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
            worktree.path.clone()
        } else {
            // Daemon cwd need not be the project root. Shared-directory
            // workers still require explicit Cassy MCP isolation.
            self.project_path().to_path_buf()
        };

        anyhow::ensure!(
            cwd.is_dir(),
            "Worker launch directory is unavailable: {}",
            cwd.display()
        );
        Ok(cwd)
    }

    /// Refuse known backend launch failures while the old worker still runs.
    /// OS/process failures can still race the later spawn; this checks the
    /// actual workspace trust and isolation configuration, not only routing.
    pub(crate) fn preflight_worker_recycle(
        &mut self,
        name: &str,
        teams: Option<&cas_mux::TeamsSpawnConfig>,
        spec: &cas_mux::WorkerSpec,
    ) -> anyhow::Result<()> {
        let cwd = self.worker_respawn_cwd(name)?;
        let config_dir = spec
            .config_dir
            .as_deref()
            .or(spec.requester_config_dir.as_deref());
        spec.cli.backend().prepare_workdir(&cwd, config_dir)?;
        let source = if spec.config_dir.is_some() {
            Some("explicit")
        } else if spec.requester_config_dir.is_some() || spec.requester_secure_storage_dir.is_some()
        {
            Some("supervisor")
        } else {
            None
        };
        let secure_storage = if spec.config_dir.is_some() {
            None
        } else if spec.requester_config_dir.is_some() || spec.requester_secure_storage_dir.is_some()
        {
            Some(spec.requester_secure_storage_dir.as_deref())
        } else {
            None
        };
        let effort = spec
            .effort
            .map(|effort| spec.cli.backend().effort_arg(effort).to_string());
        let config = cas_mux::Pane::build_worker_config(
            name,
            cwd,
            Some(&self.cas_dir),
            &self.supervisor_name,
            self.supervisor_cli,
            spec.cli,
            spec.model.as_deref(),
            effort.as_deref(),
            config_dir,
            source,
            secure_storage,
            teams,
            Some(self.mux.worker_count()),
        );
        if let Some((_, reason)) = config
            .env
            .iter()
            .find(|(key, _)| key == "CAS_FACTORY_WORKER_LAUNCH_ERROR")
        {
            anyhow::bail!("{reason}");
        }
        Ok(())
    }

    /// Respawn a crashed worker
    ///
    /// Re-creates a worker with the same name, reusing its existing worktree if available.
    pub fn respawn_worker(
        &mut self,
        name: &str,
        teams: Option<cas_mux::TeamsSpawnConfig>,
    ) -> anyhow::Result<()> {
        self.respawn_worker_with_spec(name, teams, None)
    }

    /// Respawn a worker, optionally overriding the daemon's default recipe.
    /// The optional spec is used by in-place recycling to retain the worker's
    /// provider, model, effort, and account directory.
    pub fn respawn_worker_with_spec(
        &mut self,
        name: &str,
        teams: Option<cas_mux::TeamsSpawnConfig>,
        requested_spec: Option<cas_mux::WorkerSpec>,
    ) -> anyhow::Result<()> {
        // cas-9bc6: re-read live LlmConfig so harness/model/effort changes made
        // via `cas config set` after daemon boot are reflected in this respawn.
        self.sync_worker_config_from_live_settings();

        // Respawn has no resolver call of its own: it reuses the Mux's
        // effective per-worker/default spec after the live config sync. Keep
        // the same shared validator on this recovery path before any worktree
        // or PTY launch occurs.
        let effective_spec = requested_spec
            .clone()
            .unwrap_or_else(|| self.mux.effective_worker_spec(name, None));
        cas_factory::validate_explicit(
            &effective_spec,
            &cas_factory::CapabilitySnapshot::default(),
        )
        .map_err(|error| anyhow::anyhow!("Failed to validate worker routing spec: {error}"))?;

        crate::telemetry::track(
            "factory_worker_respawn_requested",
            vec![("worktrees_enabled", bool_prop(self.worktrees_enabled()))],
        );

        // cas-58a3 review H1: a crashed worker's late cleanup would act on the
        // respawn's scope and worktree.
        if self.crash_cleanup_pending(name) {
            anyhow::bail!(
                "Worker '{name}' is still being cleaned up after its exit; retry once its reap and worktree check finish"
            );
        }

        // Check if worker is already active
        if self.worker_names.contains(&name.to_string()) {
            crate::telemetry::track(
                "factory_worker_respawn_result",
                vec![("success", "false"), ("reason", "already_active")],
            );
            anyhow::bail!("Worker '{name}' is already active");
        }

        let cwd = self.worker_respawn_cwd(name)?;

        // Add pane to mux (spawns new Claude process)
        if let Err(e) = self.mux.add_worker(
            name,
            cwd,
            Some(&self.cas_dir),
            &self.supervisor_name,
            teams.as_ref(),
            requested_spec,
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
            effective_spec.cli,
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
    ) -> ShutdownReport {
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

        let targets: Vec<String> = if !names.is_empty() {
            names.to_vec()
        } else {
            // Shutdown by count (0 = all)
            let target = count.unwrap_or(0);
            if target == 0 {
                self.worker_names.clone()
            } else {
                self.worker_names.iter().take(target).cloned().collect()
            }
        };

        let mut report = ShutdownReport::default();
        for name in targets {
            match self.shutdown_worker(&name, force).await {
                Ok(released) => report.stopped.push((name, released)),
                Err(error) => report.failed.push((name, error.to_string())),
            }
        }

        let shutdown_count_str = report.stopped.len().to_string();
        if !report.failed.is_empty() {
            let summary = report
                .failed
                .iter()
                .map(|(name, error)| format!("{name}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            self.set_error(format!("Shutdown had failures: {summary}"));
            let failure_count_str = report.failed.len().to_string();
            crate::telemetry::track(
                "factory_worker_shutdown_result",
                vec![
                    ("success", "false"),
                    ("scope", scope),
                    ("shutdown_count", &shutdown_count_str),
                    ("failure_count", &failure_count_str),
                ],
            );
        } else {
            crate::telemetry::track(
                "factory_worker_shutdown_result",
                vec![
                    ("success", "true"),
                    ("scope", scope),
                    ("shutdown_count", &shutdown_count_str),
                ],
            );
        }
        report
    }
}

#[cfg(test)]
#[path = "epic_spawn_readiness_tests.rs"]
mod spawn_readiness_tests;

#[cfg(test)]
mod spawn_base_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use rmcp::handler::server::wrapper::Parameters;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn cas_bfa1_spawn_batch_advances_local_base_once() {
        let temp = TempDir::new().unwrap();
        let origin = temp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        let old = head_sha(&origin, "main");
        commit_file(&origin, "new-base", "current origin");
        let expected = head_sha(&origin, "main");
        let repo = temp.path().join("repo");
        assert!(Command::new("git")
            .args(["clone", "-q", origin.to_str().unwrap(), repo.to_str().unwrap()])
            .status().unwrap().success());
        assert!(Command::new("git").args(["branch", "epic/batch", &old])
            .current_dir(&repo).status().unwrap().success());
        assert!(Command::new("git").args(["branch", "epic/batch", "main"])
            .current_dir(&origin).status().unwrap().success());
        let notices = (0..8).filter_map(|_| prefer_fresher_base_ref(&repo, "epic/batch").1).count();
        assert_eq!(head_sha(&repo, "epic/batch"), expected, "spawn must advance the stale local merge-back ref");
        assert_eq!(notices, 1, "one stale tip must not produce eight identical refresh notices");
    }

    /// Base-selection fixtures exercise real provisioning without depending on
    /// the host's free disk. Disk-floor refusal has its own spawn-isolation test.
    fn without_spawn_disk_floor(repo: &std::path::Path) {
        let cas_dir = repo.join(".cas");
        let mut config = crate::config::Config::load(&cas_dir).unwrap();
        config.factory.get_or_insert_with(Default::default).spawn_min_free_gib = 0;
        std::fs::create_dir_all(&cas_dir).unwrap();
        config.save(&cas_dir).unwrap();
    }

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
        let store = crate::store::open_task_store_cached(cas_dir).unwrap();
        let mut epic = cas_types::Task::new(epic_id.to_string(), title.to_string());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = branch.map(str::to_string);
        store.add(&epic).unwrap();
    }

    fn seed_child(cas_dir: &std::path::Path, task_id: &str, epic_id: &str) {
        let store = crate::store::open_task_store_cached(cas_dir).unwrap();
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

    /// cas-3228: exercise the actual create and child-inheritance path before
    /// resolving the worker spawn base. The title is deliberately long so a
    /// title-only branch recomputation cannot accidentally pass this check.
    #[tokio::test]
    async fn epic_create_child_target_and_spawn_base_round_trip_cas_3228() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().expect("initialize host registry");
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let cas_dir = repo.join(".cas");
        std::fs::create_dir(&cas_dir).unwrap();
        std::fs::write(
            cas_dir.join("config.toml"),
            "[project]\ncanonical_id = \"cas-3228-roundtrip\"\n",
        )
        .unwrap();
        init_repo(&repo);

        let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
        let request = |title: &str, task_type: &str, epic: Option<String>| {
            crate::mcp::tools::TaskCreateRequest {
                title: title.to_string(),
                description: Some("round-trip spawn authority".to_string()),
                priority: 2,
                task_type: task_type.to_string(),
            risk: Some("none".to_string()),
            door: None,
            proof_targets: None,
            supervisor_override: None,
            reason: None,
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic,
                depth: None,
            }
        };
        core.cas_task_create(Parameters(request(
            "A deliberately long epic title whose slug must be truncated before the id suffix",
            "epic",
            None,
        )))
        .await
        .expect("create long-title epic");

        let store = core.open_task_store().unwrap();
        let epic = store
            .list(None)
            .unwrap()
            .into_iter()
            .find(|task| task.task_type == cas_types::TaskType::Epic)
            .expect("created epic");
        let branch = epic.branch.clone().expect("created branch");
        assert_eq!(
            branch,
            crate::mcp::tools::epic_branch_name(&epic.title, &epic.id)
        );
        assert!(branch.ends_with(&format!("-{}", epic.id)));
        assert_eq!(
            epic.deliverables
                .work_target
                .as_ref()
                .map(|target| target.target_branch.as_str()),
            Some(branch.as_str()),
            "epic create must persist the canonical branch as WorkTarget"
        );

        core.cas_task_create(Parameters(request(
            "child inherits canonical epic lane",
            "task",
            Some(epic.id.clone()),
        )))
        .await
        .expect("create child under epic");
        let child = store
            .list(None)
            .unwrap()
            .into_iter()
            .find(|task| task.title == "child inherits canonical epic lane")
            .expect("created child");
        assert_eq!(
            child
                .deliverables
                .work_target
                .as_ref()
                .expect("child WorkTarget")
                .target_branch,
            branch
        );

        let task_base = task_epic_base(&cas_dir, &repo, &child.id);
        let (spawn_base, source) =
            resolve_spawn_base(&task_base, Some("epic/stale-focus"), "main");
        assert_eq!(spawn_base, branch);
        assert!(matches!(
            source,
            SpawnBaseSource::TaskEpic { .. } | SpawnBaseSource::WorkTarget { .. }
        ));
    }

    /// GH #670: a task's durable WorkTarget selector may resolve to a sibling
    /// checkout. The worker worktree must be cut from that checkout, where its
    /// target-only branch exists, rather than from the session repository.
    #[test]
    fn cross_repo_spawn_resolves_target_only_branch_and_worktree_repo_cas_052a() {
        TestEnvGuard::run_with_temp_home(|_| {
            crate::store::known_repos::ensure_host_schema().unwrap();

            let tmp = TempDir::new_in(crate::test_paths::runtime_fixture_parent()).unwrap();
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
            without_spawn_disk_floor(&target_repo);
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
            let tmp = TempDir::new_in(crate::test_paths::runtime_fixture_parent()).unwrap();
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
        assert!(receipt.contains("Spawn base: 'main'"), "{receipt}");
        assert!(receipt.contains("merge-back parent 'main'"), "{receipt}");
    }

    #[test]
    fn target_seed_receipt_names_provenance_age_and_skipped_crates() {
        use crate::ui::factory::app::TargetSeedStats;

        let result = WorkerSpawnResult {
            worker_name: "receipt-worker".into(),
            cwd: std::path::PathBuf::from("/workspace/target-repo"),
            cas_root: None,
            worktree: None,
            worktree_created: false,
            target_seed: Some(TargetSeedStats {
                snapshot: "target-old".into(),
                source_commit: "abc123".into(),
                age_secs: 86_400,
                skipped_crates: vec!["cas-pty".into()],
                files: 2,
                bytes: 42,
            }),
            target_seed_warning: Some("refresh".into()),
        };
        let receipt = target_seed_receipt(&result).expect("target seed receipt");
        assert!(receipt.contains("snapshot='target-old'"), "{receipt}");
        assert!(receipt.contains("source_commit='abc123'"), "{receipt}");
        assert!(receipt.contains("age_secs=86400"), "{receipt}");
        assert!(receipt.contains("skipped_crates='cas-pty'"), "{receipt}");
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
        without_spawn_disk_floor(&repo);
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
    /// maintain. An unrecorded slug is not a spawn authority.
    #[test]
    fn epic_work_target_ignores_an_unrecorded_legacy_title_slug_cas_0f97() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let legacy_slug = "epic/shockwave-migrate-ai-stack-home-onto-the-new-4tb-e";
        let declared_target = "epic/shockwave-4tb-migration";
        branch_at(&repo, legacy_slug, "main");
        branch_at(&repo, declared_target, "main");

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        assert!(task_epic.branch.is_empty());
        assert!(!task_epic.branch_exists);
        assert!(!task_epic.branch_is_title_slug_fallback);

        let (base, source) = resolve_spawn_base(&task_base, Some("epic/unrelated"), "main");
        assert_eq!(base, declared_target, "the declared WorkTarget must win");
        assert!(matches!(
            source,
            SpawnBaseSource::WorkTarget {
                owner: WorkTargetOwner::Epic { .. },
                ..
            }
        ));
        assert!(stale_legacy_slug_notice(task_base.epic(), &base, &source).is_none());
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

    /// GH #746: an epic explicitly targeted at staging still owns a separate
    /// coordination branch cut from staging. A taskless spawn under the
    /// focused epic must use that live epic tip, not the WorkTarget's parent
    /// branch (and never the project's default main branch).
    #[test]
    fn focused_staging_epic_taskless_spawn_uses_epic_tip_cas_ad05() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        Command::new("git")
            .args(["checkout", "-q", "-b", "staging"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit_file(&repo, "staging-only.txt", "staging baseline");
        Command::new("git")
            .args(["checkout", "-q", "-b", "epic/staging-based"])
            .current_dir(&repo)
            .output()
            .unwrap();
        commit_file(&repo, "epic-only.txt", "focused epic change");
        Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-ad05-epic".into(), "Staging epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/staging-based".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "staging".into(),
        });
        store.add(&epic).unwrap();

        let data = crate::ui::factory::director::DirectorData::load_fast(&cas_dir).unwrap();
        let state = crate::ui::factory::app::EpicState::Active {
            epic_id: epic.id.clone(),
            epic_title: epic.title.clone(),
        };
        let focused_branch = crate::ui::factory::app::epic_branch_for_state(&data, &state)
            .expect("focused epic must resolve a branch");
        assert_eq!(
            focused_branch, "epic/staging-based",
            "focus resolution must preserve the live epic branch over its staging parent"
        );

        let (base, source) = resolve_spawn_base(
            &TaskBase::Unresolved,
            Some(&focused_branch),
            "main",
        );
        assert_eq!(base, "epic/staging-based");
        assert!(matches!(source, SpawnBaseSource::PinnedFocus));
    }

    #[test]
    fn task_epic_without_a_resolvable_branch_keeps_focus_for_pure_resolution_cas_7587() {
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
            "pure base resolution retains focus; prepare_worker_spawn rejects this missing authority"
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
    fn legacy_epic_without_persisted_branch_does_not_reconstruct_title_slug_cas_7587() {
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
        assert!(task_epic.branch.is_empty());
        assert!(!task_epic.branch_exists);
        assert_eq!(
            resolve_spawn_base(&task_base, Some("epic/focus"), "main"),
            ("epic/focus".to_string(), SpawnBaseSource::PinnedFocus),
            "a legacy title slug must not become a synthetic spawn authority"
        );
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        // Force the fixture to start with a refusing floor regardless of host disk.
        std::fs::write(
            cas_dir.join("config.toml"),
            format!("[factory]\nspawn_min_free_gib = {}\n", u32::MAX),
        )
        .unwrap();
        without_spawn_disk_floor(&repo);
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

    #[test]
    fn origin_only_spawn_base_is_fetched_and_materialized_gh_1034() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        let repo = tmp.path().join("repo");
        assert!(Command::new("git")
            .args(["clone", "-q", origin.to_str().unwrap(), repo.to_str().unwrap()])
            .status()
            .unwrap()
            .success());

        assert!(Command::new("git")
            .args(["checkout", "-q", "-b", "epic/magic-links-followups"])
            .current_dir(&origin)
            .status()
            .unwrap()
            .success());
        commit_file(&origin, "epic-only.txt", "remote epic tip");
        let branch = "epic/magic-links-followups";
        assert!(full_sha(&repo, branch).is_none());
        assert!(full_sha(&repo, &format!("origin/{branch}")).is_none());

        let notice = ensure_local_spawn_parent(&repo, branch)
            .expect("origin-only branch should provision")
            .expect("materialization notice");
        assert!(notice.contains("SPAWN BASE MATERIALIZED"), "{notice}");
        assert_eq!(full_sha(&repo, branch), full_sha(&repo, &format!("origin/{branch}")));
        let (base_ref, _, _) = checkout_ref_for_spawn_base(&repo, branch, &SpawnBaseSource::PinnedFocus).unwrap();
        let worker_path = tmp.path().join("worker");
        crate::worktree::GitOperations::new(repo.clone())
            .create_worktree(&worker_path, "factory/magic-worker", base_ref.as_deref().or(Some(branch)))
            .expect("worker worktree should use the fetched commit");
        // pin: fixture file content proves the worktree checked out the fetched epic commit.
        assert_eq!(std::fs::read_to_string(worker_path.join("epic-only.txt")).unwrap(), "remote epic tip");
    }

    #[test]
    fn spawn_parent_creation_failure_escapes_synced_branch_labels_cas_b010() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        let branch = "epic/collision\u{85}/child";
        let git = |repo: &std::path::Path, args: &[&str]| {
            let output = Command::new("git").args(args).current_dir(repo).output().unwrap();
            assert!(output.status.success(), "{args:?}: {output:?}");
        };
        git(&origin, &["branch", branch]);
        let repo = tmp.path().join("repo");
        git(tmp.path(), &["clone", "-q", origin.to_str().unwrap(), repo.to_str().unwrap()]);
        // A local prefix blocks materialization of the fetched child ref.
        git(&repo, &["branch", "epic/collision\u{85}"]);
        let error = ensure_local_spawn_parent(&repo, branch).unwrap_err();
        assert!(error.contains("could not create local spawn base"), "{error}");
        assert!(error.contains("epic/collision\\u{85}/child"), "{error:?}");
        assert!(error.contains("origin/epic/collision\\u{85}/child"), "{error:?}");
        assert!(!error.contains('\u{85}'), "raw C1 leaked through labels or Git cause: {error:?}");
        assert!(full_sha(&repo, &format!("refs/heads/{branch}")).is_none());
    }

    #[test]
    fn absent_spawn_base_still_refuses_without_writing_local_branch_gh_1034() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let error = ensure_local_spawn_parent(&repo, "epic/missing").unwrap_err();
        assert!(error.contains("git fetch origin epic/missing"), "{error}");
        assert!(full_sha(&repo, "epic/missing").is_none());
        let mut failures = Vec::new();
        for branch in ["epic/{a,b}", "epic/a!e", "epic/$(touch${IFS}SUBSTITUTED)", "epic/a b", "epic/a'quote", "epic/control\u{85}label"] {
            let error = ensure_local_spawn_parent(&repo, branch).unwrap_err();
            if branch.chars().any(char::is_control) {
                // pin: no shell command may be printed for a control-bearing ref.
                if !error.contains("contains control characters") || !error.contains("\\u{85}")
                    || error.lines().any(|line| line.starts_with("git "))
                    || error.contains('`') || error.chars().any(|c| c.is_control() && c != '\n')
                {
                    failures.push(format!("unsafe control-ref advice: {error:?}"));
                }
                continue;
            }
            let standalone = error.lines().find(|line| line.starts_with("git fetch "));
            if standalone.is_none() {
                failures.push(format!("fetch remedy is not a standalone line: {error}"));
            }
            let command = standalone.or_else(|| error.split('`').nth(1)).unwrap();
            let output = super::spawn_readiness_tests::run_bash_remedy(
                &repo, command, "git() { printf '%s\\0' \"$@\"; }");
            let args = output.stdout.split(|byte| *byte == 0).filter(|arg| !arg.is_empty()).collect::<Vec<_>>();
            if !output.status.success() || args != [b"fetch".as_slice(), b"origin".as_slice(), branch.as_bytes()] {
                failures.push(format!("bash altered fetch arguments for {branch:?}: {output:?}"));
            }
        }
        assert!(!repo.join("SUBSTITUTED").exists(), "remedy executed branch substitution");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
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
        assert_eq!(short_sha(&repo, "main"), local_sha, "spawn selection must not move the operator's trunk");
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
        without_spawn_disk_floor(&repo);
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
            checkout_ref_for_spawn_base(&repo, "staging", &source).unwrap();
        let base_ref = base_ref.expect("WorkTarget must check out an immutable fetched tip");
        assert_eq!(checkout_ref, "origin/staging");
        assert_eq!(short_sha(&repo, &base_ref), remote_sha);
        assert!(
            notice
                .expect("stale local branch must be disclosed")
                .contains(&local_sha)
        );

        let worker_path = repo.join(".cas/worktrees/targeted-worker");
        without_spawn_disk_floor(&repo);
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

    /// cas-02ae: a diverged integration branch must refuse the real spawn
    /// before a worker can test obsolete code, preserving both histories.
    #[test]
    fn diverged_base_refuses_spawn_with_both_shas_cas_02ae() {
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
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        std::fs::write(cas_dir.join("config.toml"), "[project]\ncanonical_id = \"test\"\n").unwrap();
        let mut task = cas_types::Task::new("cas-diverged".into(), "Diverged staging".into());
        task.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "main".into(),
        });
        open_task_store_cached(&cas_dir).unwrap().add(&task).unwrap();
        let error = super::spawn_readiness_tests::prepare_existing_task(&repo, &cas_dir, &task.id)
            .err().expect("a diverged base must refuse worker creation").to_string();
        assert!(error.contains(&local_sha), "{error}");
        assert!(error.contains(&remote_sha), "{error}");
        assert!(error.contains("DIVERGED"), "{error}");
        assert!(error.contains("local-only") && error.contains("remote-only"), "{error}");
        assert_eq!(short_sha(&repo, "main"), local_sha);
        assert_eq!(short_sha(&repo, "origin/main"), remote_sha);
        assert!(!repo.join(".cas/worktrees/legacy-path-worker").exists());
        assert!(!ref_exists(&repo, "factory/legacy-path-worker"));
        let amended = Command::new("git").current_dir(&repo).args([
            "-c", "commit.gpgsign=false", "-c", "user.name=Cassy", "-c", "user.email=cassy@localhost",
            "commit", "--amend", "--reset-author", "-qm", "chore(cassy): reconcile managed project files",
        ]).output().unwrap();
        assert!(amended.status.success());
        let repair_tip = short_sha(&repo, "main");
        let error = super::spawn_readiness_tests::prepare_existing_task(&repo, &cas_dir, &task.id)
            .err().expect("repair-only divergence still refuses").to_string();
        assert!(error.contains("reconcile commit") && error.contains("PR"), "{error}");
        assert_eq!(short_sha(&repo, "main"), repair_tip);
        assert!(!ref_exists(&repo, "factory/legacy-path-worker"));
        commit_file(&repo, "human-work.txt", "unpublished project work");
        let error = super::spawn_readiness_tests::prepare_existing_task(&repo, &cas_dir, &task.id)
            .err().expect("mixed divergence still refuses").to_string();
        assert!(!error.contains("Cause:"), "mixed history must not be blamed only on repairs: {error}");
    }

    #[test]
    fn configured_origin_fetch_failure_refuses_unmanaged_spawn_cas_02ae() {
        super::spawn_readiness_tests::with_fixture(|f| {
            let mut task = cas_types::Task::new("cas-offline".into(), "Offline integration".into());
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: "project:bfa1-round2".into(),
                target_branch: "main".into(),
            });
            open_task_store_cached(&f.root).unwrap().add(&task).unwrap();
            // cas-cd99: reachable origin lacks this local-only branch; the
            // operator needs publication guidance, not an access repair.
            super::spawn_readiness_tests::git(&f.repo, &["branch", "feature/unpublished"]);
            task.deliverables.work_target.as_mut().unwrap().target_branch = "feature/unpublished".into();
            open_task_store_cached(&f.root).unwrap().update(&task).unwrap();
            let error = super::spawn_readiness_tests::prepare_existing_task(&f.repo, &f.root, &task.id)
                .err().expect("unpublished branch must name the publication requirement").to_string();
            assert!(error.contains("origin/feature/unpublished") && error.contains("not published"), "{error}");
            assert!(error.contains("git push origin feature/unpublished"), "{error}");
            assert!(!error.contains("restore origin access"), "{error}");
            assert_eq!(full_sha(&f.repo, "feature/unpublished").as_deref(), Some(f.old.as_str()));
            assert!(!ref_exists(&f.repo, "factory/legacy-path-worker"));
            assert!(!f.repo.join(".cas/worktrees/legacy-path-worker").exists());
            task.deliverables.work_target.as_mut().unwrap().target_branch = "main".into();
            open_task_store_cached(&f.root).unwrap().update(&task).unwrap();
            f.offline();
            let error = super::spawn_readiness_tests::prepare_existing_task(&f.repo, &f.root, &task.id)
                .err().expect("cached equal refs cannot prove a configured origin is current").to_string();
            assert!(error.contains("fetch") && error.contains("failed"), "{error}");
            assert!(error.contains(&f.old), "{error}");
            assert!(error.contains("0 local-only, 0 remote-only"), "{error}");
            assert_eq!(full_sha(&f.repo, "main").as_deref(), Some(f.old.as_str()));
            assert_eq!(full_sha(&f.repo, "origin/main").as_deref(), Some(f.old.as_str()));
            assert!(!ref_exists(&f.repo, "factory/legacy-path-worker"));
            assert!(!f.repo.join(".cas/worktrees/legacy-path-worker").exists());
        });
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
        let (_, notice, _) = checkout_ref_for_spawn_base(&repo, "main", &SpawnBaseSource::Trunk).unwrap();
        assert!(notice.unwrap().contains("LOCAL-ONLY"));

        // cas-cd99: an origin-less repo can retain a tracking ref from a
        // previous fetch. The notice must name that actual checkout source.
        let local = full_sha(&repo, "main").unwrap();
        commit_file(&repo, "cached-only.txt", "cached origin content");
        let cached = full_sha(&repo, "main").unwrap();
        super::spawn_readiness_tests::git(&repo, &["update-ref", "refs/remotes/origin/main", &cached]);
        super::spawn_readiness_tests::git(&repo, &["reset", "--hard", &local]);
        let (base_ref, notice, checkout_ref) = checkout_ref_for_spawn_base(&repo, "main", &SpawnBaseSource::Trunk).unwrap();
        assert_eq!(base_ref.as_deref(), Some(cached.as_str()));
        assert_eq!(checkout_ref, "origin/main");
        let notice = notice.unwrap();
        assert!(notice.contains("LOCAL-ONLY") && notice.contains("origin/main"), "{notice}");
        assert!(notice.contains(&cached), "{notice}");
        assert_eq!(full_sha(&repo, "main").as_deref(), Some(local.as_str()));
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
        let error = validate_live_spawn_repo_root(manager.repo_root(), &project)
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
        without_spawn_disk_floor(&repo);
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
            target_seed: None,
            target_seed_warning: None,
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
            target_seed: None,
            target_seed_warning: None,
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
            target_seed: None,
            target_seed_warning: None,
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
            target_seed: None,
            target_seed_warning: None,
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

        let notice = super::spawn_readiness_tests::prepare_recorded_lane(&repo, "epic/behind", "main")
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
        without_spawn_disk_floor(&repo);
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

    /// cas-93e8: spawn-time epic refresh must not promote a worker or operator
    /// commit that exists only on the local parent branch. Origin is the
    /// shared parent authority whenever that remote-tracking ref exists.
    #[test]
    fn epic_base_refresh_refuses_local_parent_ahead_of_origin_cas_93e8() {
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

        let remote_parent_tip = head_sha(&origin, "main");
        commit(&repo, "operator-only.txt", "unpublished local parent work");
        let local_parent_tip = head_sha(&repo, "main");
        let epic_tip = head_sha(&repo, "epic/behind");
        assert_ne!(local_parent_tip, remote_parent_tip);

        let error = super::spawn_readiness_tests::prepare_recorded_lane(&repo, "epic/behind", "main")
            .expect_err("an unpublished local parent must refuse epic refresh");
        assert!(error.contains("ahead of shared 'origin/main'"), "{error}");
        assert!(error.contains("unpublished local parent"), "{error}");
        assert_eq!(head_sha(&repo, "epic/behind"), epic_tip);
        assert_eq!(head_sha(&origin, "epic/behind"), epic_tip);
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

        let notice = super::spawn_readiness_tests::prepare_recorded_lane(&repo, "epic/behind", "main")
            .expect("a remote-less checkout can still refresh its local epic base")
            .expect("the local refresh must be reported");
        assert!(notice.contains("EPIC BASE FAST-FORWARDED (LOCAL):"), "{notice}");
        assert!(notice.contains("epic/behind"), "{notice}");
        assert!(notice.contains("main"), "{notice}");
        assert!(notice.contains("unpublished"), "{notice}");
        assert_eq!(head_sha(&repo, "epic/behind"), parent_tip);

        let worker_path = repo.join(".cas/worktrees/local-only-refreshed-worker");
        without_spawn_disk_floor(&repo);
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
    fn epic_base_refresh_realigns_a_linked_checkout_before_spawn() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        branch_at(&repo, "epic/behind", "main");

        let sibling = tmp.path().join("supervisor-merge");
        let added = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                sibling.to_str().unwrap(),
                "epic/behind",
            ])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            added.status.success(),
            "{}",
            String::from_utf8_lossy(&added.stderr)
        );

        commit(&repo, "parent-advance.txt", "parent advanced locally");
        let parent_tip = head_sha(&repo, "main");
        let notice = super::spawn_readiness_tests::prepare_recorded_lane(&repo, "epic/behind", "main")
            .unwrap()
            .unwrap();

        assert!(notice.contains("EPIC BASE FAST-FORWARDED (LOCAL):"), "{notice}");
        assert_eq!(head_sha(&sibling, "HEAD"), parent_tip);
        assert!(sibling.join("parent-advance.txt").is_file());
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&sibling)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(status.stdout.is_empty(), "linked checkout was left dirty");
    }

    #[test]
    fn work_target_spawn_uses_unpublished_refreshed_epic_tip_after_push_rejection_cas_5504() {

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
        crate::test_paths::warm_stub(
            &reject_hook,
            "#!/bin/sh\necho push rejected by test hook >&2\nexit 1\n",
        );

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

        let notice = super::spawn_readiness_tests::prepare_recorded_lane(&repo, "epic/behind", "main")
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
            checkout_ref_for_spawn_base(&repo, "epic/behind", &source).unwrap();
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
        without_spawn_disk_floor(&repo);
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        std::fs::write(cas_dir.join("config.toml"), "[project]\ncanonical_id = \"test\"\n").unwrap();
        let prepared = super::spawn_readiness_tests::prepare_existing_task(&repo, &cas_dir, "cas-child")
            .expect("the strictly-behind outer epic must prepare through production");
        assert_eq!(prepared.worktree_info.as_ref().unwrap().parent_branch, "epic/outer");
        let notice = prepared.warnings.join("\n");
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
        without_spawn_disk_floor(&repo);
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

        let error = super::spawn_readiness_tests::prepare_recorded_lane(&repo, "epic/diverged", "main")
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
        let amended = Command::new("git").current_dir(&repo).args([
            "-c", "commit.gpgsign=false", "-c", "user.name=Cassy", "-c", "user.email=cassy@localhost",
            "commit", "--amend", "--reset-author", "-qm", "chore(cassy): reconcile managed project files",
        ]).output().unwrap();
        assert!(amended.status.success());
        let parent_tip = head_sha(&repo, "main");
        let error = super::spawn_readiness_tests::prepare_existing_task(&repo, &repo.join(".cas"), "cas-legacy-child")
            .err().expect("repair-only parent divergence still refuses").to_string();
        assert!(error.contains("reconcile commit") && error.contains("PR"), "{error}");
        assert_eq!(head_sha(&repo, "epic/diverged"), epic_tip);
        assert_eq!(head_sha(&repo, "main"), parent_tip);

        // A published repair can be ahead of the local parent after rebase
        // merging. The real lane check chooses its remote-tracking parent.
        for args in [
            vec!["update-ref", "refs/remotes/origin/main", parent_tip.as_str()],
            vec!["reset", "--hard", "-q", "HEAD~1"],
        ] {
            assert!(Command::new("git").current_dir(&repo).args(args).status().unwrap().success());
        }
        let error = super::spawn_readiness_tests::prepare_existing_task(&repo, &repo.join(".cas"), "cas-legacy-child")
            .err().expect("remote-parent repair divergence still refuses").to_string();
        let cause = error.split("Cause:").nth(1).expect("Cassy-authored repair needs attribution");
        assert!(!cause.contains("local-only"), "published repair was mislabelled: {error}");
        assert!(cause.contains("not on") && cause.contains("epic/diverged"), "cause must identify the compared ref: {error}");
        assert_eq!(head_sha(&repo, "epic/diverged"), epic_tip);
        assert_eq!(head_sha(&repo, "origin/main"), parent_tip);
    }

    /// GH #584 / cas-a075: the production spawn-preparation seam must stop
    /// before `WorkerSpawnPrep` can cut a no-code worker from a diverged epic.
    #[test]
    fn no_code_spawn_refuses_diverged_epic_before_worktree_cut_cas_a075() {
        for offline in [false, true] {
            super::spawn_readiness_tests::with_fixture(|fixture| {
                let tasks = open_task_store_cached(&fixture.root).unwrap();
                super::spawn_readiness_tests::git(
                    &fixture.repo,
                    &["branch", "-m", "epic/round2", "epic/diverged-support"],
                );
                let mut epic = tasks.get("cas-round2-epic").unwrap();
                epic.branch = Some("epic/diverged-support".into());
                tasks.update(&epic).unwrap();
                let mut child = tasks.get("cas-round2-child").unwrap();
                child.execution_note = Some("no-code".into());
                tasks.update(&child).unwrap();
                super::spawn_readiness_tests::git(
                    &fixture.repo,
                    &["checkout", "epic/diverged-support"],
                );
                super::spawn_readiness_tests::commit(&fixture.repo, "epic-only");
                let tip = super::spawn_readiness_tests::git(
                    &fixture.repo,
                    &["rev-parse", "epic/diverged-support"],
                );
                // A published lane remains protected even with no recorded
                // delivery: another clone can already depend on its graph.
                super::spawn_readiness_tests::git(&fixture.repo, &["push", "origin", "epic/diverged-support"]);
                super::spawn_readiness_tests::git(&fixture.repo, &["checkout", "main"]);
                fixture.advance_parent();
                if offline {
                    fixture.offline();
                }
                let error = fixture
                    .prepare()
                    .err()
                    .expect("diverged no-code epic must refuse")
                    .to_string();
                assert!(
                    error.contains("EPIC BASE REFRESH REFUSED") && error.contains("have diverged"),
                    "{error}"
                );
                assert!(
                    error.contains("epic/diverged-support") && error.contains("main"),
                    "{error}"
                );
                assert_eq!(
                    super::spawn_readiness_tests::git(
                        &fixture.repo,
                        &["rev-parse", "epic/diverged-support"]
                    ),
                    tip
                );
                assert!(!fixture.repo.join(".cas/worktrees/round2-worker").exists());
            });
        }
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
        without_spawn_disk_floor(&repo);
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

    #[test]
    fn exit_joins_held_cleanup_before_releasing_name_cas_af63() {
        use std::sync::mpsc;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let (release, held) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = held.recv();
            CrashCleanupOutcome { worktree: None }
        });
        let (returned, observe) = mpsc::channel();
        let gate = std::thread::spawn(move || {
            let returned_before_release = observe.recv_timeout(std::time::Duration::from_secs(2)).is_ok();
            drop(release);
            returned_before_release
        });
        let mut app = FactoryApp::for_test_at(_env.home().join("project/.cas"));
        app.register_worker_cleanup(WorkerCleanupJob::new("held-worker".into(), handle));
        app.join_worker_worktree_cleanups();
        let _ = returned.send(());
        assert!(!gate.join().unwrap(), "exit returned while cleanup still held");
        assert!(!app.worker_worktree_cleanups_pending());
        assert!(!app.crash_cleanup_pending("held-worker"));
    }

    #[tokio::test]
    async fn restart_replays_deferred_cleanup_intent_cas_af63() {
        use std::process::Command;
        let env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().join("project");
        std::fs::create_dir(&repo).unwrap();
        for args in [vec!["init", "-q"], vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"]] {
            assert!(Command::new("git").args(args).current_dir(&repo).status().unwrap().success());
        }
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        assert!(Command::new("git").args(["add", ".gitignore"]).current_dir(&repo).status().unwrap().success());
        assert!(Command::new("git").args(["commit", "-qm", "fixture"]).current_dir(&repo).status().unwrap().success());
        let root = crate::store::init_cas_dir(&repo).unwrap();
        let mut manager = WorktreeManager::new(&repo, crate::worktree::WorktreeConfig {
            enabled: true, base_path: root.join("worktrees").to_string_lossy().into_owned(),
            branch_prefix: "factory/".into(), auto_merge: false,
            cleanup_on_close: false, promote_entries_on_merge: false,
        }).unwrap();
        for (name, disposition) in [
            ("held-wip", "wip"), ("held-unowned", "unowned"), ("dead-recycle", "duplicates"),
            ("interrupted-worker", "interrupted"), ("failed-worker", "error"),
        ] {
            manager.create_for_worker(name).unwrap();
            let plan = manager.worker_removal_plan(name).unwrap();
            let worker = plan.worktree.path.clone();
            let mut agent = cas_types::Agent::new(format!("{name}-id"), name.into());
            agent.metadata.insert("clone_path".into(), worker.display().to_string());
            let mut tasks = Vec::new();
            if disposition == "wip" {
                let mut task = Task::new("unfinished-task".into(), "Still working".into());
                task.assignee = Some(agent.id.clone());
                task.status = TaskStatus::InProgress;
                tasks.push(task);
            }
            if disposition == "unowned" {
                let target = crate::factory_scratch::worker_target_dir(&worker);
                std::fs::create_dir_all(&target).unwrap();
                std::fs::write(target.join("unverified.log"), "preserve").unwrap();
            }
            if disposition == "duplicates" {
                let agents = open_agent_store(&root).unwrap();
                agent.role = cas_types::AgentRole::Worker;
                agent.pid = Some(i32::MAX as u32);
                agent.status = cas_types::AgentStatus::Shutdown;
                agents.register(&agent).unwrap();
                let mut sibling = agent.clone();
                sibling.id = "sibling-registration".into();
                agents.register(&sibling).unwrap();
                let matching: Vec<_> = agents.list(None).unwrap().into_iter().filter(|row| row.name == name).collect();
                let mut task = Task::new("duplicate-task".into(), "Still working".into());
                task.assignee = Some(matching[1].id.clone());
                task.status = TaskStatus::InProgress;
                crate::store::open_task_store_cached(&root).unwrap().add(&task).unwrap();
                drop(crate::factory_target_cache::owner::acquire(&root, &worker).unwrap().unwrap());
                let target = crate::factory_scratch::worker_target_dir(&worker);
                let proof = target.join("worker-check.log");
                std::fs::write(&proof, "preserve").unwrap();
                let mut app = FactoryApp::for_test_at(root.clone());
                app.shutdown_worker_for_recycle(name).await.unwrap();
                app.join_worker_worktree_cleanups();
                assert!(proof.is_file(), "a task owned by a later duplicate identity must protect recycled output");
                continue;
            }
            let request = DeferredWorkerCleanup {
                occurrence: uuid::Uuid::new_v4().to_string(), agent, tasks,
                worktree: Some((plan.worktree, if disposition == "error" {
                    // A missing repository makes worktree removal return an I/O error.
                    repo.join("missing-repository")
                } else { plan.repo_root })),
            };
            let path = request.path(&root);
            let mut first = FactoryApp::for_test_at(root.clone());
            if disposition == "interrupted" {
                // A crash after intent persistence, before the job runs.
                request.persist(&root).unwrap();
                assert!(path.is_file(), "an interrupted job keeps its intent");
            } else {
                first.start_worker_cleanup(request);
                first.join_worker_worktree_cleanups();
                if disposition == "error" {
                    assert!(path.is_file(), "I/O errors keep their recovery intent");
                    std::fs::remove_file(path).unwrap();
                    continue;
                }
                assert!(worker.exists(), "{name}: deliberate holds preserve output");
                if disposition == "unowned" {
                    // pin: ownership refusal must preserve the fixture evidence bytes exactly.
                    assert_eq!(std::fs::read_to_string(crate::factory_scratch::worker_target_dir(&worker).join("unverified.log")).unwrap(), "preserve");
                }
                assert!(!path.exists(), "{name}: a deliberate hold must complete its intent");
            }
            drop(first);
            let mut restarted = FactoryApp::for_test_at(root.clone());
            restarted.resume_deferred_worker_cleanups();
            if disposition == "interrupted" {
                assert!(restarted.crash_cleanup_pending(name), "startup restores the name fence");
                restarted.join_worker_worktree_cleanups();
                assert!(!worker.exists(), "restart finishes interrupted cleanup");
                assert!(!path.exists(), "completed recovery removes its intent");
                assert!(!restarted.crash_cleanup_pending(name));
            } else {
                assert!(!restarted.worker_worktree_cleanups_pending(), "completed holds do not replay");
            }
            restarted.resume_deferred_worker_cleanups();
            assert!(!restarted.worker_worktree_cleanups_pending(), "a completed intent replays only once");
        }
    }

    /// Shutdown must give control back while Git inside target retirement is held.
    /// The same name cannot be relaunched underneath that outstanding cleanup.
    #[cfg(unix)]
    #[tokio::test]
    async fn held_worktree_cleanup_leaves_control_and_reserves_name_cas_f9f7() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        use std::sync::mpsc;
        use std::time::Duration;

        for dead in [false, true] {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args)
                .current_dir(&repo).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        git(&["add", ".gitignore"]);
        git(&["commit", "--allow-empty", "-qm", "fixture"]);
        let cas = crate::store::init_cas_dir(&repo).unwrap();
        let mut manager = WorktreeManager::new(&repo, crate::worktree::WorktreeConfig {
            enabled: true, base_path: cas.join("worktrees").to_string_lossy().into_owned(),
            branch_prefix: "factory/".into(), auto_merge: false,
            cleanup_on_close: false, promote_entries_on_merge: false,
        }).unwrap();
        let name = "held-cleanup";
        manager.create_for_worker(name).unwrap();
        let worker = manager.worker_removal_plan(name).unwrap().worktree.path;
        drop(crate::factory_target_cache::owner::acquire(&cas, &worker).unwrap().unwrap());
        let target = crate::factory_scratch::worker_target_dir(&worker);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("worker-check.log"), "preserve proof").unwrap();
        let agents = open_agent_store(&cas).unwrap();
        let mut agent = cas_types::Agent::new("fixture-agent".into(), name.into());
        agent.role = cas_types::AgentRole::Worker;
        if dead {
            agent.pid = Some(i32::MAX as u32);
            agent.status = cas_types::AgentStatus::Shutdown;
        }
        agent.metadata.insert("clone_path".into(), worker.display().to_string());
        agents.register(&agent).unwrap();
        let entered = env.home().join("entered");
        let release = env.home().join("release");
        for path in [&entered, &release] {
            let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        let monitor = env.home().join("fsmonitor");
        std::fs::write(&monitor, format!(
            "#!/bin/sh\nif mkdir '{}' 2>/dev/null; then\nprintf x > '{}'\nread signal < '{}'\nfi\nprintf 'token\\0'\n",
            env.home().join("once").display(), entered.display(), release.display(),
        )).unwrap();
        std::fs::set_permissions(&monitor, std::fs::Permissions::from_mode(0o700)).unwrap();
        git(&["config", "extensions.worktreeConfig", "true"]);
        git(&["-C", worker.to_str().unwrap(), "config", "--worktree", "core.fsmonitor", monitor.to_str().unwrap()]);
        let (returned_tx, returned_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let gate = std::thread::spawn(move || {
            std::fs::File::open(entered).unwrap().read_exact(&mut [0]).unwrap();
            entered_tx.send(()).unwrap();
            // This is a responsiveness deadline, not a sleep to order the test.
            // On the old path it releases Git so the failing test cannot hang.
            let returned_while_held = returned_rx.recv_timeout(Duration::from_secs(2)).is_ok();
            if returned_while_held { let _ = release_rx.recv_timeout(Duration::from_secs(10)); }
            std::fs::File::create(release).unwrap().write_all(b"continue\n").unwrap();
            returned_while_held
        });
        let mut app = FactoryApp::for_test_at(cas.clone());
        app.set_worktree_manager_for_test(manager);
        if !dead {
        let pty = cas_mux::Pty::spawn(name, cas_mux::PtyConfig {
            command: "/bin/sleep".into(), args: vec!["300".into()],
            cwd: Some(worker.clone()), env: vec![], env_remove: vec![], rows: 24, cols: 80,
        }).unwrap();
        app.adopt_worker_pane_for_test(cas_mux::Pane::with_pty(
            name, cas_mux::PaneKind::Worker, pty, 24, 80, cas_mux::SupervisorCli::Codex,
        ).unwrap());
        }
        app.shutdown_worker(name, true).await.unwrap();
        let (flushed_tx, flushed_rx) = mpsc::channel();
        crate::ui::factory::daemon::runtime::store_worker::submit_write("cleanup regression sentinel", move || {
            let _ = returned_tx.send(());
            let _ = flushed_tx.send(());
            Ok::<_, String>(())
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            app.poll_worker_worktree_cleanups();
            if entered_rx.try_recv().is_ok() { break; }
            assert!(std::time::Instant::now() < deadline, "retirement reached the held Git call");
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let recovery_path = app.worktree_cleanup_jobs.first().and_then(|job| job.recovery_path.clone());
        let intent_durable = recovery_path.as_ref().is_some_and(|path| path.is_file());
        let reserved = app.crash_cleanup_pending(name);
        let _ = release_tx.send(());
        let lane_free = gate.join().unwrap();
        flushed_rx.recv_timeout(Duration::from_secs(10)).expect("store write lane finishes after release");
        assert!(lane_free, "dead={dead}: target retirement blocked the store write lane");
        assert!(intent_durable, "cleanup intent must be durable before retirement I/O");
        let recovery_path = recovery_path.unwrap();
        assert!(reserved, "a late cleanup must not remove a same-name replacement");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while app.worker_worktree_cleanups_pending() && std::time::Instant::now() < deadline {
            app.poll_worker_worktree_cleanups();
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(worker.exists(), dead, "dead registrations retain their checkout; live shutdown reclaims it");
        assert!(!target.exists(), "target retirement finishes before checkout removal");
        assert!(!recovery_path.exists(), "successful cleanup removes its recovery intent");
        assert!(!app.crash_cleanup_pending(name), "completed cleanup releases the name");
        assert_eq!(app.worktree_manager().unwrap().worker_removal_plan(name).is_none(), !dead);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cas_50713_stale_reset_releases_cloud_then_replacement_starts_without_claim_lost() {
        use std::sync::{Arc, Mutex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        for (initial_owner, unregistered) in [("old-session", false), ("old-session", true),
            ("new-session", false), ("new-session", true)] {
            let _env = crate::test_support::TestEnvGuard::temp_home();
            let (_temp, cas_dir) = seeded_cas_dir();
            let server = MockServer::start().await;
            let holder = Arc::new(Mutex::new(Some(initial_owner.to_string())));
            let state = Arc::clone(&holder);
            let originally_renewed = chrono::Utc::now();
            Mock::given(wiremock::matchers::path_regex("^/api/agents/tasks/"))
                .respond_with(move |request: &wiremock::Request| {
                    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
                    let caller = body["agent_id"].as_str().unwrap_or_default();
                    let action = request.url.path().rsplit('/').next().unwrap();
                    let mut owner = state.lock().unwrap();
                    let lock = |id: &str| serde_json::json!({
                        "id":"lock-id", "task_id":"cas-reset", "agent_id":id, "status":"active",
                        "expires_at":(chrono::Utc::now()+chrono::Duration::minutes(10)).to_rfc3339(),
                        "renewed_at":originally_renewed.to_rfc3339(), "renewal_count":0, "epoch":1,
                        "claim_reason":null
                    });
                    let (status, answer) = match action {
                        "lock" => (200, serde_json::json!({"lock":owner.as_deref().map(lock)})),
                        "release" => {
                            if owner.as_deref() == Some(caller) { *owner = None; }
                            (200, serde_json::json!({"status":"success"}))
                        }
                        "claim" if owner.as_deref().is_some_and(|id| id != caller) => (409,
                            serde_json::json!({"error":"already_claimed", "owner_agent_id":owner.clone(),
                                "expires_at":(chrono::Utc::now()+chrono::Duration::minutes(10)).to_rfc3339()})),
                        "claim" => {
                            *owner = Some(caller.into());
                            (200, serde_json::json!({"status":"success", "result":"claimed", "lock":lock(caller)}))
                        }
                        "renew" => (200, serde_json::json!({"lock":lock(caller)})),
                        _ => (404, serde_json::json!({"error":"unexpected route"})),
                    };
                    ResponseTemplate::new(status).set_body_json(answer)
                }).mount(&server).await;
            std::fs::write(
                cas_dir.join("config.toml"),
                r#"[project]
    canonical_id = "github.com/acme/widget"
    [verification]
    enabled = false
    "#,
            )
            .unwrap();
            std::fs::write(
                cas_dir.join("cloud.json"),
                serde_json::json!({
                    "endpoint":server.uri(), "token":"test-token"
                })
                .to_string(),
            )
            .unwrap();
            let tasks = open_task_store_cached(&cas_dir).unwrap();
            let mut task = task_with("cas-reset", Some("dead-worker"), TaskStatus::InProgress);
            task.notes = "Prior WIP must survive reset".into();
            tasks.add(&task).unwrap();
            let agents = open_agent_store(&cas_dir).unwrap();
            let mut old = cas_types::Agent::new("old-session".into(), "dead-worker".into());
            agents.register(&old).unwrap();
            agents.try_claim(&task.id, &old.id, 600, None).unwrap();
            old.status = cas_types::AgentStatus::Shutdown;
            old.pid = None;
            old.last_heartbeat = chrono::Utc::now() - chrono::Duration::minutes(20);
            agents.register(&old).unwrap();
            if unregistered { agents.unregister(&old.id).unwrap(); }
            agents
                .register(&cas_types::Agent::new(
                    "new-session".into(),
                    "replacement".into(),
                ))
                .unwrap();
            let root = cas_dir.clone();
            assert!(
                tokio::task::spawn_blocking(move || assign_task_to_new_worker(
                    &root,
                    "cas-reset",
                    "replacement"
                ))
                .await
                .unwrap()
            );
            if initial_owner == "old-session" {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while holder.lock().unwrap().is_some() {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }).await.expect("background cloud release must settle the retired owner");
            }
            assert_eq!(
                holder.lock().unwrap().as_deref(),
                if initial_owner == "old-session" {
                    None
                } else {
                    Some("new-session")
                },
                "reset must release only retired cloud owners, preserving a concurrent replacement"
            );
            assert!(
                tasks
                    .get(&task.id)
                    .unwrap()
                    .notes
                    .contains("Prior WIP must survive reset")
            );
            let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
            core.set_agent_id_for_testing("new-session".into());
            core.cas_task_start_with_options(rmcp::handler::server::wrapper::Parameters(
                crate::mcp::tools::TaskStartRequest {
                    id: task.id.clone(),
                    brief: Some(true),
                    force: None,
                },
            ))
            .await
            .unwrap();
            assert_eq!(holder.lock().unwrap().as_deref(), Some("new-session"));
            let root = cas_dir.clone();
            let report = tokio::task::spawn_blocking(move || {
                crate::cloud::peer_claims::renew_agent_claims(&root, "new-session")
            })
            .await
            .unwrap();
            assert!(report.conflicts.is_empty());
            assert!(
                !crate::store::open_prompt_queue_store(&cas_dir)
                    .unwrap()
                    .peek_all(100)
                    .unwrap()
                    .iter()
                    // pin: this alarm class is the externally observed false-conflict regression.
                    .any(|row| row.prompt.contains("PEER CLAIM LOST"))
            );
        }
    }

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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        store
            .add(&task_with("t-open", Some("agent-a"), TaskStatus::Open))
            .unwrap();

        assert!(worker_has_open_tasks(&cas_dir, "agent-a"));
    }

    #[test]
    fn worker_has_open_tasks_true_for_in_progress() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        store
            .add(&task_with("t-done", Some("agent-c"), TaskStatus::Closed))
            .unwrap();

        assert!(!worker_has_open_tasks(&cas_dir, "agent-c"));
    }

    #[test]
    fn worker_has_open_tasks_false_when_open_task_belongs_to_other_agent() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        store
            .add(&task_with("t-other", Some("agent-other"), TaskStatus::Open))
            .unwrap();

        assert!(!worker_has_open_tasks(&cas_dir, "agent-d"));
    }

    fn claimed_qa_fixture(cas_dir: &std::path::Path) -> cas_types::QaPass {
        let store = open_task_store_cached(cas_dir).unwrap();
        let mut task = task_with("cas-qa3172", Some("dead-reviewer"), TaskStatus::InProgress);
        task.labels.push("qa-pass".to_string());
        task.notes = "prior review evidence".into();
        task.branch = Some("factory/prior-review".into());
        store.add(&task).unwrap();
        let now = chrono::Utc::now();
        let opened = cas_store::open_qa_pass(
            cas_dir,
            &cas_store::NewQaPass {
                task_id: "cas-delivery3172",
                implementer_agent_id: "implementer",
                branch: "factory/implementer",
                bound_head: "aaaa1111",
                deadline_at: now + chrono::Duration::minutes(45),
                max_rounds: 3,
            },
            now,
        )
        .unwrap();
        let cas_store::QaPassOpen::Dispatched(pass) = opened else {
            panic!("new round");
        };
        cas_store::set_qa_task(cas_dir, &pass.id, "cas-qa3172").unwrap();
        cas_store::claim_qa_pass(cas_dir, "cas-delivery3172", "dead-reviewer", now).unwrap()
    }

    fn assert_pending_qa(cas_dir: &std::path::Path, before: &cas_types::QaPass) {
        let pass = cas_store::latest_qa_pass(cas_dir, &before.task_id, chrono::Utc::now()).unwrap().unwrap();
        assert_eq!(pass.id, before.id);
        assert_eq!(pass.round, before.round);
        assert_eq!(pass.deadline_at, before.deadline_at);
        assert_eq!(pass.state, cas_types::QaPassState::Pending);
        assert!(pass.reviewer_agent_id.is_none());
    }

    #[tokio::test]
    async fn spawn_workers_replaces_dead_qa_reviewer_same_round_cas_3172() {
        use cas_store::SpawnQueueStore;
        use rmcp::handler::server::wrapper::Parameters;
        let mut guard = crate::test_env_guard::TestEnvGuard::temp_home();
        let bin = guard.home().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        crate::test_paths::warm_stub(
            &bin.join("codex"),
            "#!/bin/sh\nprintf 'codex-cli 0.0.0-test\\n'\n",
        );
        let auth = guard.home().join(".codex/auth.json");
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        std::fs::write(auth, "{}").unwrap();
        guard.set(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        let (_temp, cas_dir) = seeded_cas_dir();
        let before = claimed_qa_fixture(&cas_dir);
        let agents = open_agent_store(&cas_dir).unwrap();
        agents
            .register(&cas_types::Agent::new_with_role(
                "supervisor-id".into(),
                "supervisor".into(),
                cas_types::AgentRole::Supervisor,
            ))
            .unwrap();
        let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
        core.set_agent_id_for_testing("supervisor-id".into());
        let service = crate::mcp::CasService::new(core, None);
        let req = serde_json::from_value(serde_json::json!({"action":"spawn_workers", "task_id":"cas-qa3172", "count":1, "cli":"codex"})).unwrap();
        service.factory_request(Parameters(req)).await.expect("MCP accepts replacement spawn");
        let queue = crate::store::open_spawn_queue_store(&cas_dir).unwrap();
        let queued = queue.peek(1).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].task_id.as_deref(), Some("cas-qa3172"));
        // The PTY boot is omitted; invoke exactly the preassignment seam that
        // early registration and finish_worker_spawn call with the queued id.
        assert!(assign_task_to_new_worker(&cas_dir, queued[0].task_id.as_deref().unwrap(), "replacement-reviewer"));
        assert_pending_qa(&cas_dir, &before);
        let task = open_task_store_cached(&cas_dir).unwrap().get("cas-qa3172").unwrap();
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.assignee.as_deref(), Some("replacement-reviewer"));
        assert!(task.notes.contains("prior review evidence"));
        assert!(task.notes.contains("force-released"));
        assert_eq!(task.branch.as_deref(), Some("factory/prior-review"));
        agents
            .register(&cas_types::Agent::new_with_role(
                "replacement-id".into(),
                "replacement-reviewer".into(),
                cas_types::AgentRole::Worker,
            ))
            .unwrap();
        let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
        core.set_agent_id_for_testing("replacement-id".into());
        core.cas_task_start(Parameters(crate::mcp::tools::IdRequest {
            id: "cas-qa3172".into(),
        }))
        .await
        .expect("replacement starts same round now, not at deadline");
        let pass = cas_store::latest_qa_pass(&cas_dir, &before.task_id, chrono::Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(pass.id, before.id);
        assert_eq!(pass.deadline_at, before.deadline_at);
        assert_eq!(pass.reviewer_agent_id.as_deref(), Some("replacement-reviewer"));
    }

    #[test]
    fn qa_claim_tracks_shutdown_and_aborted_preassignment_cas_3172() {
        for aborted in [false, true] {
            let (_temp, cas_dir) = seeded_cas_dir();
            let before = claimed_qa_fixture(&cas_dir);
            if aborted { release_preassign_if_bound(&cas_dir, "cas-qa3172", "dead-reviewer"); }
            else { assert_eq!(release_worker_task_bindings(&cas_dir, "dead-reviewer"), 1); }
            assert_pending_qa(&cas_dir, &before);
            assert_eq!(open_task_store_cached(&cas_dir).unwrap().get("cas-qa3172").unwrap().assignee, None);
        }
    }

    #[test]
    fn qa_claim_survives_live_holder_and_mismatched_cleanup_cas_3172() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let before = claimed_qa_fixture(&cas_dir);
        open_agent_store(&cas_dir)
            .unwrap()
            .register(&cas_types::Agent::new_with_role(
                "live-id".into(),
                "dead-reviewer".into(),
                cas_types::AgentRole::Worker,
            ))
            .unwrap();
        assert!(!assign_task_to_new_worker(
            &cas_dir,
            "cas-qa3172",
            "replacement"
        ));
        release_preassign_if_bound(&cas_dir, "cas-qa3172", "other-worker");
        let pass = cas_store::latest_qa_pass(&cas_dir, &before.task_id, chrono::Utc::now()).unwrap().unwrap();
        assert_eq!(pass.state, cas_types::QaPassState::Claimed);
        assert_eq!(pass.reviewer_agent_id.as_deref(), Some("dead-reviewer"));
    }

    // --- cas-6913 / cas-7a94: spawn-time task pre-assignment ------------

    #[test]
    fn failed_qa_release_keeps_binding_for_retry_cas_3172() {
        for recovery in ["spawn", "shutdown", "abort"] {
            let (_temp, cas_dir) = seeded_cas_dir();
            let before = claimed_qa_fixture(&cas_dir);
            let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
            conn.execute_batch(
                "CREATE TRIGGER reject_qa_release BEFORE UPDATE ON qa_passes
                WHEN OLD.state = 'claimed' AND NEW.state = 'pending'
                BEGIN SELECT RAISE(ABORT, 'injected QA release failure'); END;",
            )
            .unwrap();
            let recover = || match recovery {
                "spawn" => {
                    assign_task_to_new_worker(&cas_dir, "cas-qa3172", "replacement");
                }
                "shutdown" => {
                    release_worker_task_bindings(&cas_dir, "dead-reviewer");
                }
                _ => release_preassign_if_bound(&cas_dir, "cas-qa3172", "dead-reviewer"),
            };
            recover();
            let task = open_task_store_cached(&cas_dir)
                .unwrap()
                .get("cas-qa3172")
                .unwrap();
            assert_eq!(
                task.assignee.as_deref(),
                Some("dead-reviewer"),
                "{recovery}"
            );
            assert_eq!(task.status, TaskStatus::InProgress, "{recovery}");
            assert_eq!(
                cas_store::latest_qa_pass(&cas_dir, &before.task_id, chrono::Utc::now())
                    .unwrap()
                    .unwrap(),
                before
            );
            conn.execute_batch("DROP TRIGGER reject_qa_release;")
                .unwrap();
            recover();
            assert_pending_qa(&cas_dir, &before);
            let task = open_task_store_cached(&cas_dir)
                .unwrap()
                .get("cas-qa3172")
                .unwrap();
            assert_eq!(task.status, TaskStatus::Open);
            assert_ne!(task.assignee.as_deref(), Some("dead-reviewer"));
        }
    }

    /// AC3: `spawn_workers task_id=<id>` must result in the task's assignee
    /// being the newly spawned worker's display name — the same field
    /// `task action=mine` filters on (cas-dbbb convention) — so the
    /// worker's very first `task mine` shows it.
    #[test]
    fn assign_task_to_new_worker_sets_assignee() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

    // --- cas-a622: a recycle keeps bindings and re-delivers the brief ----

    /// The binding set wise-raven-87 held when its refresh was refused.
    fn seed_wise_raven_tasks(cas_dir: &std::path::Path) -> Vec<(&'static str, TaskStatus)> {
        let store = crate::store::open_task_store_cached(cas_dir).unwrap();
        let held = vec![
            ("cas-e0be", TaskStatus::InProgress),
            ("cas-7cb3", TaskStatus::AwaitingMerge),
            ("cas-ed87", TaskStatus::Open),
            ("cas-96c0", TaskStatus::Blocked),
        ];
        for (id, status) in &held {
            store.add(&task_with(id, Some("wise-raven"), *status)).unwrap();
        }
        store
            .add(&task_with("cas-done", Some("wise-raven"), TaskStatus::Closed))
            .unwrap();
        store
            .add(&task_with("cas-peer", Some("other-worker"), TaskStatus::InProgress))
            .unwrap();
        held
    }

    fn retired_worker(cas_dir: &std::path::Path) -> cas_types::Agent {
        let mut agent = cas_types::Agent::new("raven-old-session".into(), "wise-raven".into());
        agent.role = cas_types::AgentRole::Worker;
        // A pid that cannot be alive, so the worker reads as retired. Stamped
        // like every pid assignment (cas-389c); a dead pid has no starttime,
        // so the stamp records nothing and the row stays unfingerprinted.
        agent.pid = Some(u32::MAX - 1);
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, u32::MAX - 1);
        crate::store::open_agent_store(cas_dir).unwrap().register(&agent).unwrap();
        agent
    }

    /// cas-a622 (and the proud-newt-45 loss it cites): retiring a worker for
    /// an in-place recycle must not park or release a single binding — the
    /// same name is respawned in the same worktree at once.
    #[test]
    fn recycle_retirement_keeps_every_task_binding_cas_a622() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let held = seed_wise_raven_tasks(&cas_dir);
        let agent = retired_worker(&cas_dir);
        let agent_store = crate::store::open_agent_store(&cas_dir).unwrap();

        let released = settle_retired_worker_bindings(
            &cas_dir,
            agent_store.as_ref(),
            &agent,
            &["cas-e0be".to_string()],
            WorkerRetirement::Recycle,
        );

        assert_eq!(released, 0, "a recycle releases nothing");
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        for (id, status) in held {
            let task = store.get(id).unwrap();
            assert_eq!(task.assignee.as_deref(), Some("wise-raven"), "{id} keeps its assignee");
            assert_eq!(task.status, status, "{id} keeps its status");
        }
    }

    #[test]
    fn recycle_retirement_preserves_reassigned_snapshot_cas_35af() {
        let (_temp, cas_dir) = seeded_cas_dir();
        seed_wise_raven_tasks(&cas_dir);
        let agent = retired_worker(&cas_dir);
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let tasks = crate::store::open_task_store_cached(&cas_dir).unwrap();
        // The task was held when recycling began, then reassigned before
        // retirement/death cleanup observed that old snapshot.
        let snapshot = vec!["cas-e0be".to_string()];
        let mut reassigned = tasks.get("cas-e0be").unwrap();
        reassigned.assignee = Some("replacement-worker".into());
        tasks.update(&reassigned).unwrap();
        assert_eq!(settle_retired_worker_bindings(
            &cas_dir, agents.as_ref(), &agent, &snapshot, WorkerRetirement::Recycle,
        ), 0);
        crate::mcp::tools::service::orphan_recovery::recover_worker_vanished(
            &cas_dir, agents.as_ref(), &agent, &snapshot, "late exit after recycle",
        );
        let after = tasks.get("cas-e0be").unwrap();
        assert_eq!(after.assignee, reassigned.assignee);
        assert_eq!(after.status, reassigned.status);
    }

    /// The contrast that keeps the shutdown contract honest: a real shutdown
    /// still frees the worker's Open/InProgress/Blocked bindings.
    #[test]
    fn shutdown_retirement_still_releases_bindings_cas_a622() {
        let (_temp, cas_dir) = seeded_cas_dir();
        seed_wise_raven_tasks(&cas_dir);
        let agent = retired_worker(&cas_dir);
        let agent_store = crate::store::open_agent_store(&cas_dir).unwrap();

        settle_retired_worker_bindings(
            &cas_dir,
            agent_store.as_ref(),
            &agent,
            &[],
            WorkerRetirement::Shutdown,
        );

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        assert_eq!(store.get("cas-ed87").unwrap().assignee, None, "Open pre-assign released");
        assert_eq!(
            store.get("cas-7cb3").unwrap().assignee.as_deref(),
            Some("wise-raven"),
            "AwaitingMerge parking is never clobbered"
        );
    }

    /// cas-a622: the recycled worker's fresh conversation is told what it
    /// holds and how to resume, and only its own nonterminal tasks are named.
    #[test]
    fn recycle_resume_brief_is_queued_to_the_worker_cas_a622() {
        let (_temp, cas_dir) = seeded_cas_dir();
        seed_wise_raven_tasks(&cas_dir);

        // cas-a02a: receipt history survives draining/acknowledgement. The
        // checkpoint is deliberately older than task.updated_at (start/reclaim
        // mutates that field without saving conversation context).
        let tasks = open_task_store_cached(&cas_dir).unwrap();
        for task_id in ["cas-e0be", "cas-7cb3", "cas-ed87", "cas-96c0"] {
            tasks
                .append_note(task_id, "[2026-10-20 12:00] 📝 PROGRESS Saved checkpoint")
                .unwrap();
        }
        tasks
            .append_note(
                "cas-96c0",
                "[2026-10-20 12:20] 📝 PROGRESS Later checkpoint",
            )
            .unwrap();
        let agents = open_agent_store(&cas_dir).unwrap();
        let mut supervisor = cas_types::Agent::new_with_role(
            "supervisor-a02a".into(),
            "zen-condor".into(),
            cas_types::AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("session-a622".into());
        agents.register(&supervisor).unwrap();
        let mut peer = cas_types::Agent::new_with_role(
            "peer-a02a".into(),
            "peer".into(),
            cas_types::AgentRole::Worker,
        );
        peer.factory_session = Some("session-a622".into());
        agents.register(&peer).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        let mut received = Vec::new();
        for (body, target, session, origin, surfaced, at) in [
            (
                "PRE_CHECKPOINT",
                "wise-raven",
                "session-a622",
                "supervisor-a02a",
                true,
                "2026-10-20T11:59:00Z",
            ),
            (
                "OTHER_WORKER",
                "peer",
                "session-a622",
                "supervisor-a02a",
                true,
                "2026-10-20T12:01:00Z",
            ),
            (
                "OTHER_SESSION",
                "wise-raven",
                "other-session",
                "supervisor-a02a",
                true,
                "2026-10-20T12:02:00Z",
            ),
            (
                "UNRECEIVED",
                "wise-raven",
                "session-a622",
                "supervisor-a02a",
                false,
                "2026-10-20T12:03:00Z",
            ),
            (
                "SPOOFED_SUPERVISOR",
                "wise-raven",
                "session-a622",
                "peer-a02a",
                true,
                "2026-10-20T12:04:00Z",
            ),
            (
                "<cas-assignment task_id=\"cas-peer\"/> UNBOUND_TASK",
                "wise-raven",
                "session-a622",
                "supervisor-a02a",
                true,
                "2026-10-20T12:05:00Z",
            ),
            (
                "cas-96c0 STALE_FOR_BOUND_TASK",
                "wise-raven",
                "session-a622",
                "supervisor-a02a",
                true,
                "2026-10-20T12:15:00Z",
            ),
            (
                "TRANSPORT_ONLY",
                "wise-raven",
                "session-a622",
                "supervisor-a02a",
                true,
                "2026-10-20T12:16:00Z",
            ),
            (
                "OLDEST_OVER_LIMIT",
                "wise-raven",
                "session-a622",
                "supervisor-a02a",
                true,
                "2026-10-20T12:06:00Z",
            ),
        ] {
            let id = queue
                .enqueue_urgent_with_outcome(
                    "zen-condor",
                    target,
                    body,
                    Some(session),
                    None,
                    None,
                    false,
                    Some(&cas_store::QueueOrigin::RegisteredAgent {
                        agent_id: origin.into(),
                    }),
                )
                .unwrap()
                .id();
            if surfaced {
                if body == "TRANSPORT_ONLY" {
                    queue
                        .record_recipient_transport_delivered(id, target)
                        .unwrap();
                } else {
                    queue
                        .record_recipient_surfaced(
                            id,
                            target,
                            cas_store::SurfacingSource::InboxPoll,
                        )
                        .unwrap();
                    queue.ack(id).unwrap();
                }
                queue.mark_processed(id).unwrap();
                conn.execute(
                    "UPDATE prompt_queue_recipient_seen SET seen_at = ? WHERE prompt_id = ?",
                    rusqlite::params![at, id],
                )
                .unwrap();
            }
        }
        for index in 0..10 {
            let body = if index == 9 {
                "Yes — use the supplied candidate; that answers your question.".to_string()
            } else if index == 7 {
                format!("OVERSIZED_ANSWER{}", "λ".repeat(9000))
            } else {
                format!("Decision {index} for cas-e0be: continue with the agreed candidate.")
            };
            let id = queue
                .enqueue_urgent_with_outcome(
                    "zen-condor",
                    "wise-raven",
                    &body,
                    Some("session-a622"),
                    None,
                    None,
                    false,
                    Some(&cas_store::QueueOrigin::RegisteredAgent {
                        agent_id: supervisor.id.clone(),
                    }),
                )
                .unwrap()
                .id();
            queue
                .record_recipient_surfaced(id, "wise-raven", cas_store::SurfacingSource::InboxPoll)
                .unwrap();
            queue.mark_processed(id).unwrap();
            queue.ack(id).unwrap();
            // Enqueued before the checkpoint, actually received afterwards.
            conn.execute(
                "UPDATE prompt_queue SET created_at = '2026-10-20 11:00:00' WHERE id = ?",
                [id],
            )
            .unwrap();
            conn.execute(
                "UPDATE prompt_queue_recipient_seen SET seen_at = ? WHERE prompt_id = ?",
                rusqlite::params![
                    format!(
                        "2026-10-20T12:10:{:02}Z",
                        if index == 8 { 11 } else { index }
                    ),
                    id
                ],
            )
            .unwrap();
            received.push((id, body));
        }

        let id = enqueue_recycle_resume_brief(&cas_dir, "zen-condor", "session-a622", "wise-raven")
            .unwrap()
            .expect("a worker holding tasks gets a resume brief");

        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let queued = queue
            .peek_all(20)
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .expect("brief row");
        assert_eq!(queued.target, "wise-raven");
        assert_eq!(queued.source, "zen-condor");
        assert_eq!(queued.factory_session.as_deref(), Some("session-a622"));
        let brief = &queued.prompt;
        assert!(
            brief.contains("task action=start id=cas-e0be"),
            "the in-progress task is resumed by re-taking its lease: {brief}"
        );
        for id in ["cas-e0be", "cas-7cb3", "cas-ed87", "cas-96c0"] {
            assert!(brief.contains(id), "{id} is named: {brief}");
        }
        for id in ["cas-done", "cas-peer"] {
            assert!(
                !brief.contains(id),
                "{id} is not this worker's open work: {brief}"
            );
        }
        let newest = &received[8];
        let previous = &received[9];
        assert!(
            brief.contains(&received[9].1),
            "received answer survives recycle: {brief}"
        );
        assert!(
            brief.contains(&format!("#{}", newest.0)),
            "retain the original notification id"
        );
        assert!(
            brief.find(&newest.1).unwrap() < brief.find(&previous.1).unwrap(),
            "newest received decision first"
        );
        assert!(!brief.contains(&received[0].1), "history is bounded");
        for excluded in [
            "PRE_CHECKPOINT",
            "OTHER_WORKER",
            "OTHER_SESSION",
            "UNRECEIVED",
            "SPOOFED_SUPERVISOR",
            "UNBOUND_TASK",
            "STALE_FOR_BOUND_TASK",
            "TRANSPORT_ONLY",
            "OLDEST_OVER_LIMIT",
        ] {
            assert!(!brief.contains(excluded), "exclude {excluded}: {brief}");
        }
        assert!(!brief.contains("OVERSIZED_ANSWER"));
        assert!(
            brief.contains(&format!("notification_id={}", received[7].0)),
            "oversized answers retain an exact retrieval reference"
        );
        assert!(
            brief.len() < 16_000,
            "the resume brief has a bounded history"
        );
        // pin: the original unread body must remain available to the ordinary inbox drain.
        assert!(
            queue
                .peek_unseen_for_recipient("wise-raven", Some("session-a622"), 100)
                .unwrap()
                .iter()
                .any(|row| row.prompt == "UNRECEIVED"),
            "building a brief never consumes pending mail"
        );
        // Charge the section intro as well as entries. This payload fits the
        // old entry-only cap, but not the whole history section.
        let receive = |body: &str, at: &str| {
            let id = queue
                .enqueue_urgent_with_outcome(
                    "zen-condor",
                    "wise-raven",
                    body,
                    Some("session-a622"),
                    None,
                    None,
                    false,
                    Some(&cas_store::QueueOrigin::RegisteredAgent {
                        agent_id: supervisor.id.clone(),
                    }),
                )
                .unwrap()
                .id();
            queue
                .record_recipient_surfaced(id, "wise-raven", cas_store::SurfacingSource::InboxPoll)
                .unwrap();
            queue.mark_processed(id).unwrap();
            queue.ack(id).unwrap();
            conn.execute(
                "UPDATE prompt_queue_recipient_seen SET seen_at = ? WHERE prompt_id = ?",
                rusqlite::params![at, id],
            )
            .unwrap();
            id
        };
        receive(&"λ".repeat(5950), "2026-10-20T12:40:00Z");
        let capped = recycle_resume_brief(&cas_dir, "wise-raven", "session-a622").unwrap();
        let history_bytes =
            capped.len() - capped.find("Recent received supervisor messages").unwrap();
        // More than a raw query page of newer, ineligible replies must not
        // hide an older answer. Cover both task and per-task checkpoint filters.
        for index in 0..65 {
            let (body, at) = if index % 2 == 0 {
                (
                    format!("<cas-assignment task_id=\"cas-peer\"/> FILTERED_{index}"),
                    "2026-10-20T12:50:00Z",
                )
            } else {
                (
                    format!("cas-96c0 FILTERED_STALE_{index}"),
                    "2026-10-20T12:19:00Z",
                )
            };
            receive(&body, at);
        }
        let filtered = recycle_resume_brief(&cas_dir, "wise-raven", "session-a622").unwrap();
        assert_eq!(
            (history_bytes <= 12_000, filtered.contains(&received[9].1)),
            (true, true),
            "history bytes including intro={history_bytes}; eligible answer must survive >64 filtered rows"
        );
        assert!(!filtered.contains("FILTERED_"));
    }

    #[test]
    fn a_worker_with_no_open_work_gets_no_resume_brief_cas_a622() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let mut untouched = "existing brief".to_string();
        append_recycle_received_messages(
            &cas_dir,
            "idle-worker",
            "session-a622",
            &[],
            &[],
            &mut untouched,
        );
        assert_eq!(
            untouched, "existing brief",
            "empty held-task history is a no-op"
        );
        assert_eq!(
            enqueue_recycle_resume_brief(&cas_dir, "zen-condor", "session-a622", "idle-worker")
                .unwrap(),
            None
        );
        assert!(
            crate::store::open_prompt_queue_store(&cas_dir)
                .unwrap()
                .peek_all(10)
                .unwrap()
                .is_empty()
        );
    }

    // --- cas-7a94: shutdown / cancel must release pre-assigns -----------

    /// Inverse bug: Open pre-assign to a dead worker must clear on release so
    /// the next worker can transfer/start without a manual reset.
    #[test]
    fn release_worker_task_bindings_clears_open_preassign() {
        let (_temp, cas_dir) = seeded_cas_dir();
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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

        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
        let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
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
