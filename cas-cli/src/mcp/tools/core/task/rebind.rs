//! Supervisor approval of an existing replacement delivery lane. Git is read-only.

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use cas_types::{AgentRole, FactoryBranchStart, TaskStatus, TaskType};
use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ErrorCode};

use crate::bounded_process::{Deadline, run_command};
use crate::mcp::CasCore;

use super::{lifecycle::close_ops, repo_context, resolve_agent_identity};

// gate[G-152 rebind]: Delivery branch rebinding authority and evidence
fn refused(reason: impl Into<String>) -> McpError {
    CasCore::error(
        ErrorCode::INVALID_PARAMS,
        crate::gates::tag("G-152",format!("DELIVERY BRANCH REBIND REFUSED: {}", reason.into())),
    )
}

struct Git<'a> {
    root: &'a Path,
    deadline: Deadline,
}

impl Git<'_> {
    fn output(&self, args: &[&str]) -> Result<Output, String> {
        run_command(
            Command::new("git")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .arg("-C")
                .arg(self.root)
                .args(args),
            self.deadline,
            Duration::from_secs(5),
        )
        .map_err(|error| format!("cannot inspect Git: {error:?}"))
    }

    fn text(&self, args: &[&str]) -> Result<String, String> {
        let output = self.output(args)?;
        if !output.status.success() {
            return Err(format!(
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn tip(&self, branch: &str) -> Result<String, String> {
        self.text(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{branch}^{{commit}}"),
        ])
    }
}

/// The durable audit and ledger write shared by approved and safe automatic rebinds.
pub(super) struct BranchRebindAudit {
    actor: String,
    reason: String,
    previous_tip: Option<String>,
    tip: String,
    target_tip: String,
}

impl BranchRebindAudit {
    pub(super) fn persist(
        &self,
        cas_root: &Path,
        tasks: &dyn cas_store::TaskStore,
        expected: &cas_types::Task,
        rebound: &mut cas_types::Task,
    ) -> cas_store::Result<chrono::DateTime<chrono::Utc>> {
        let previous = expected
            .deliverables
            .factory_branch_start
            .as_ref()
            .expect("rebind has a previous start");
        let start = rebound
            .deliverables
            .factory_branch_start
            .as_ref()
            .expect("rebind has a replacement start");
        let branch = start.branch.clone();
        let audit = serde_json::json!({
            "actor": self.actor, "reason": self.reason, "previous_start": previous,
            "previous_tip": self.previous_tip,
            "previous_anchor": expected.deliverables.factory_branch_anchor,
            "start": start, "tip": self.tip, "target_tip": self.target_tip,
        });
        rebound
            .deliverables
            .retain_factory_branch_anchor_as_history();
        rebound.deliverables.parked_branch = None;
        rebound.deliverables.record_handoff_branch(&previous.branch);
        rebound.deliverables.green_merge_commit = None;
        rebound.deliverables.parked_close_reason = None;
        rebound.deliverables.merge_conflicted = false;
        rebound.updated_at = chrono::Utc::now();
        rebound.notes.push_str(&format!(
            "\n\n[{}] DELIVERY BRANCH REBOUND: {audit}",
            rebound.updated_at.format("%Y-%m-%d %H:%M")
        ));
        // Use the guarded entry implemented by SyncingTaskStore, too.
        let updated_at = tasks.update_if_current(rebound, expected)?;
        crate::quality_ledger::intervention(
            cas_root,
            cas_store::InterventionKind::Rebind,
            format!("rebind|{}|{branch}|{}", rebound.id, self.tip),
            Some(&rebound.id),
            Some(&self.actor),
            Some(&self.reason),
        );
        Ok(updated_at)
    }
}

/// Missing or unreadable refs cannot prove safe succession. Leave the recorded
/// start intact so task start reports the existing supervisor recovery action.
pub(super) fn reassigned_branch_start(
    cas_root: &Path,
    checkout: &Path,
    task: &cas_types::Task,
    worker: &cas_types::Agent,
    branch: &str,
    queued_start: bool,
) -> Option<(FactoryBranchStart, BranchRebindAudit)> {
    let previous = task.deliverables.factory_branch_start.as_ref()?;
    let target = task.deliverables.work_target.as_ref()?;
    if previous.work_target.repo_selector != target.repo_selector {
        return None;
    }
    let git = Git {
        root: checkout,
        deadline: Deadline::after(Duration::from_secs(20)),
    };
    if git.text(&["symbolic-ref", "--short", "HEAD"]).ok()? != branch {
        return None;
    }
    let tip = git.tip("HEAD").ok()?;
    let mut previous_tips = Vec::new();
    for namespace in ["refs/heads", "refs/remotes/origin"] {
        let reference = format!("{namespace}/{}", previous.branch);
        let exists = git
            .output(&["show-ref", "--verify", "--quiet", &reference])
            .ok()?;
        match exists.status.code() {
            Some(0) => previous_tips.push(git.tip(&reference).ok()?),
            Some(1) => {}
            _ => return None,
        }
    }
    let previous_tip = previous_tips.first()?.clone();
    let target_ref = close_ops::preferred_diff_target_ref_for_delivery(
        checkout,
        &target.target_branch,
        task.delivery_mode,
    );
    let target_tip = git.tip(&target_ref).ok()?;
    // GH #1760: a queued task may have waited through a retarget without
    // delivering anything. Its old lane can contain an unrelated inherited
    // base; preserve that ref and bind the untouched successor at the target.
    // Missing refs or delivery evidence never establish an empty old lane.
    let empty_successor = queued_start
        && branch == crate::factory_isolation::worker_task_branch(&worker.name, &task.id)
        && tip == target_tip
        && task.deliverables.parked_branch.is_none()
        && task.deliverables.factory_branch_anchor.is_none()
        && task.deliverables.historical_factory_branch_anchors.is_empty()
        && cas_store::get_latest_worker_delivery(cas_root, &task.id)
            .is_ok_and(|delivery| delivery.is_none())
        && git.text(&["status", "--porcelain"]).ok()?.is_empty();
    for old_tip in &previous_tips {
        if !(empty_successor && *old_tip == previous.base_sha)
            && !git.output(&["merge-base", "--is-ancestor", old_tip, &tip])
                .ok()?.status.success()
        {
            return None;
        }
    }
    if (queued_start || previous.work_target != *target) && !empty_successor {
        return None;
    }
    let base_sha = git.text(&["merge-base", "--all", &tip, &target_tip]).ok()?;
    if base_sha.lines().count() != 1 {
        return None;
    }
    Some((
        FactoryBranchStart {
            work_target: target.clone(),
            assignee: task.assignee.clone().unwrap_or_else(|| worker.name.clone()),
            branch: branch.to_string(),
            base_sha,
        },
        BranchRebindAudit {
            actor: worker.id.clone(),
            reason: if empty_successor {
                "Automatic queued reassignment before first delivery: prior tips are untouched or inherited".into()
            } else {
                "Automatic reassignment: every previous branch tip is inherited".into()
            },
            previous_tip: Some(previous_tip),
            tip,
            target_tip,
        },
    ))
}

impl CasCore {
    pub(crate) fn rebind_task_branch(
        &self,
        id: &str,
        branch: &str,
        reason: &str,
    ) -> Result<CallToolResult, McpError> {
        let supervisor = self.resolve_live_supervisor_authority().map_err(|_| {
            refused("only a live registered supervisor may approve a replacement branch")
        })?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(refused("a non-empty approval reason is required"));
        }
        if let Some(error) = super::caller_note_marker_rejection(reason) {
            return Err(refused(error));
        }
        let tasks = self.open_task_store()?;
        let agents = self.open_agent_store()?;
        let expected = tasks.get(id).map_err(|error| refused(error.to_string()))?;
        if expected.task_type == TaskType::Epic
            || !matches!(
                expected.status,
                TaskStatus::Open | TaskStatus::InProgress | TaskStatus::Blocked
            )
            || (expected.status == TaskStatus::Blocked
                && (expected.deliverables.parked_branch.is_some()
                    || expected.deliverables.factory_branch_anchor.is_some()))
        {
            return Err(refused(
                "only an unparked active task can rebind; use request_changes before replacing a parked delivery",
            ));
        }
        let previous = expected
            .deliverables
            .factory_branch_start
            .as_ref()
            .ok_or_else(|| refused("no recorded start boundary; start the assigned task first"))?;
        let target = expected
            .deliverables
            .work_target
            .as_ref()
            .ok_or_else(|| refused("the task has no declared work target"))?;
        // Reassignment preserves the old start as evidence. Explicit approval
        // binds the current assignee's lane; the prior owner stays in the audit.
        // Retargeting still cannot move the delivery to another repository.
        if previous.work_target.repo_selector != target.repo_selector {
            return Err(refused(
                "repository differs from the recorded start; restore the original repository before rebinding",
            ));
        }
        let assignee = expected.assignee.as_deref().ok_or_else(|| {
            refused("assign the task to a live registered worker before rebinding")
        })?;
        let worker = resolve_agent_identity(agents.as_ref(), assignee)
            .filter(|worker| worker.role == AgentRole::Worker && worker.is_alive())
            .ok_or_else(|| refused("the assignee must be a live registered worker"))?;
        if supervisor.factory_session != worker.factory_session {
            return Err(refused(
                "the supervisor and assigned worker belong to different factory sessions",
            ));
        }
        if agents
            .get_lease(id)
            .map_err(|error| refused(error.to_string()))?
            .is_some_and(|lease| lease.is_valid() && lease.agent_id != worker.id)
        {
            return Err(refused(
                "a different agent holds the task lease; resolve ownership first",
            ));
        }
        let branch = branch.trim();
        let task_branch = crate::factory_isolation::worker_task_branch(&worker.name, id);
        if branch != crate::factory_isolation::expected_worker_branch(&worker.name)
            && branch != task_branch
            && !branch
                .strip_prefix(&format!("{task_branch}-"))
                .is_some_and(|suffix| !suffix.is_empty())
        {
            return Err(refused(
                "the replacement must be this worker's branch for this task, optionally with a recovery suffix",
            ));
        }
        let checkout = worker
            .metadata
            .get("clone_path")
            .filter(|path| !path.trim().is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| close_ops::resolve_system_b_worktree_path(&self.cas_root, &worker.name))
            .ok_or_else(|| refused("the assigned worker has no registered checkout"))?;
        let context =
            repo_context::resolve_repo_context(&self.cas_root, target).map_err(refused)?;
        let (_, common) = repo_context::git_layout(&checkout).map_err(refused)?;
        if common != context.git_common_dir {
            return Err(refused(
                "the worker checkout belongs to a different repository",
            ));
        }
        let git = Git {
            root: &checkout,
            deadline: Deadline::after(Duration::from_secs(20)),
        };
        git.text(&["check-ref-format", "--branch", branch])
            .map_err(refused)?;
        if git
            .text(&["symbolic-ref", "--short", "HEAD"])
            .map_err(refused)?
            != branch
            || !git
                .text(&["status", "--porcelain"])
                .map_err(refused)?
                .is_empty()
        {
            return Err(refused(
                "the worker must have the replacement branch checked out with a clean worktree",
            ));
        }
        // A branch already reserved by another delivery is not this task's lane.
        for task in tasks
            .list(None)
            .map_err(|error| refused(error.to_string()))?
        {
            if task.id != id
                && !task.is_terminal()
                && (task
                    .deliverables
                    .factory_branch_start
                    .as_ref()
                    .is_some_and(|start| start.branch == branch)
                    || task.deliverables.parked_branch.as_deref() == Some(branch))
            {
                return Err(refused(format!(
                    "replacement branch is already bound to task {}",
                    task.id
                )));
            }
        }
        let tip = git.tip(&format!("refs/heads/{branch}")).map_err(refused)?;
        let target_ref = close_ops::preferred_diff_target_ref_for_delivery(
            &checkout,
            &target.target_branch,
            expected.delivery_mode,
        );
        let target_tip = git.tip(&target_ref).map_err(refused)?;
        let previous_tip = git
            .tip(&format!("refs/heads/{}", previous.branch))
            .or_else(|_| git.tip(&format!("refs/remotes/origin/{}", previous.branch)))
            .ok();
        let same_branch = previous.branch == branch;
        // Recovery can rewrite a lane without changing its name. Attribute
        // the replacement against the target, never an obsolete start base.
        let base = git
            .text(&["merge-base", "--all", &tip, &target_tip])
            .map_err(refused)?;
        if base.lines().count() != 1 {
            return Err(refused("replacement and target need a unique merge base"));
        }
        let mut rebound = expected.clone();
        rebound
            .deliverables
            .retain_factory_branch_anchor_as_history();
        rebound.deliverables.parked_branch = None;
        rebound.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: target.clone(),
            assignee: assignee.to_string(),
            branch: branch.to_string(),
            base_sha: base.clone(),
        });
        // The common root was validated above. A linked checkout need not
        // duplicate its project configuration, and shares these same refs.
        // GH #1673: after a reviewed retarget, an untouched lane has no
        // delivery to attribute yet. Approve its replacement at the new
        // target without requiring the worker to commit on an unbound ref.
        let empty_retarget = previous.work_target != *target
            && previous.assignee == assignee
            && previous_tip.as_deref() == Some(previous.base_sha.as_str())
            && tip == target_tip
            && expected.deliverables.parked_branch.is_none()
            && expected.deliverables.factory_branch_anchor.is_none()
            && expected.deliverables.historical_factory_branch_anchors.is_empty();
        if !empty_retarget {
            let delivery = close_ops::task_attribution::started_delivery(&context.repo_root, &rebound)
                .map_err(refused)?
                .ok_or_else(|| refused("replacement has no unmerged task delivery"))?;
            if delivery.commits.is_empty() {
                return Err(refused("replacement has no attributable commits"));
            }
        }
        let anchor_still_reachable = match expected.deliverables.factory_branch_anchor.as_deref() {
            Some(anchor) => git
                .output(&["merge-base", "--is-ancestor", anchor, &tip])
                .map_err(refused)?
                .status
                .success(),
            None => true,
        };
        if same_branch
            && previous.assignee == assignee
            && previous.work_target == *target
            && previous.base_sha == base
            && anchor_still_reachable
        {
            return Ok(Self::success(format!(
                "Task {id} already records delivery branch {branch}; its base and audit are unchanged."
            )));
        }
        if git.tip(&format!("refs/heads/{branch}")).map_err(refused)? != tip
            || git.tip(&target_ref).map_err(refused)? != target_tip
            || (empty_retarget
                && git.tip(&format!("refs/heads/{}", previous.branch))
                    .or_else(|_| git.tip(&format!("refs/remotes/origin/{}", previous.branch)))
                    .ok() != previous_tip)
            || git
                .text(&["symbolic-ref", "--short", "HEAD"])
                .map_err(refused)?
                != branch
            || !git
                .text(&["status", "--porcelain"])
                .map_err(refused)?
                .is_empty()
        {
            return Err(refused(
                "Git refs or worktree changed during approval; inspect the replacement and retry",
            ));
        }
        let audit = BranchRebindAudit {
            actor: supervisor.id.clone(),
            reason: reason.to_string(),
            previous_tip: previous_tip.clone(),
            tip: tip.clone(),
            target_tip,
        };
        audit
            .persist(&self.cas_root, tasks.as_ref(), &expected, &mut rebound)
            .map_err(|error| refused(error.to_string()))?;
        Ok(Self::success(format!(
            "Task {id} delivery branch rebound to {branch} at {tip}.\nPrevious branch {} at {} is retained in the audit.\nAssignment, target, leases and normal close gates are unchanged.",
            previous.branch,
            previous_tip.as_deref().unwrap_or("unavailable")
        )))
    }
}
