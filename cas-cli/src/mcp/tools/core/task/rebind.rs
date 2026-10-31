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

fn refused(reason: impl Into<String>) -> McpError {
    CasCore::error(
        ErrorCode::INVALID_PARAMS,
        format!("DELIVERY BRANCH REBIND REFUSED: {}", reason.into()),
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
        // A supervisor may retarget this delivery to another lane in the
        // same repository. Validate its new base below without changing owner.
        if previous.work_target.repo_selector != target.repo_selector
            || expected.assignee.as_deref() != Some(previous.assignee.as_str())
        {
            return Err(refused(
                "assignment or repository differs from the recorded start; resolve that recovery first",
            ));
        }
        let worker = resolve_agent_identity(agents.as_ref(), &previous.assignee)
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
            assignee: previous.assignee.clone(),
            branch: branch.to_string(),
            base_sha: base.clone(),
        });
        // The common root was validated above. A linked checkout need not
        // duplicate its project configuration, and shares these same refs.
        let delivery = close_ops::task_attribution::started_delivery(&context.repo_root, &rebound)
            .map_err(refused)?
            .ok_or_else(|| refused("replacement has no unmerged task delivery"))?;
        if delivery.commits.is_empty() {
            return Err(refused("replacement has no attributable commits"));
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
        let audit = serde_json::json!({
            "actor": supervisor.id, "reason": reason, "previous_start": previous,
            "previous_tip": previous_tip, "previous_anchor": expected.deliverables.factory_branch_anchor,
            "start": rebound.deliverables.factory_branch_start, "tip": tip, "target_tip": target_tip,
        });
        rebound.deliverables.record_handoff_branch(&previous.branch);
        rebound.deliverables.green_merge_commit = None;
        rebound.deliverables.parked_close_reason = None;
        rebound.deliverables.merge_conflicted = false;
        rebound.updated_at = chrono::Utc::now();
        rebound.notes.push_str(&format!(
            "\n\n[{}] DELIVERY BRANCH REBOUND: {audit}",
            rebound.updated_at.format("%Y-%m-%d %H:%M")
        ));
        // cas-b403: the guarded write every caller uses. A logged-in store
        // (SyncingTaskStore) implements only this entry: it stages the sync
        // intent and binds it as the receipt of the inner store's guarded
        // write. The receipt variant is the inner store's API, and reached
        // through the wrapper it failed closed.
        tasks
            .update_if_current(&rebound, &expected)
            .map_err(|error| refused(error.to_string()))?;
        Ok(Self::success(format!(
            "Task {id} delivery branch rebound to {branch} at {tip}.\nPrevious branch {} at {} is retained in the audit.\nAssignment, target, leases and normal close gates are unchanged.",
            previous.branch,
            previous_tip.as_deref().unwrap_or("unavailable")
        )))
    }
}
