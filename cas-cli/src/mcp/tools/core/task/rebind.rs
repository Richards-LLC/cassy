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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::CasService;
    use crate::store::{init_cas_dir, open_agent_store, open_task_store};
    use crate::test_env_guard::TestEnvGuard;
    use cas_types::{Agent, AgentStatus, Task, TaskDepth, WorkTarget};
    use rmcp::handler::server::wrapper::Parameters;

    /// Exercise the daemon's real force-release/preassign seam, then the MCP
    /// recovery route through park and merge. No PTY or host factory is needed.
    #[tokio::test]
    async fn recycled_worker_takeover_rebinds_and_delivers_cas_d6b1() {
        let mut env = TestEnvGuard::temp_home();
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let git = |root: &Path, args: &[&str]| {
            let output = Command::new("git").arg("-C").arg(root).args(args).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&project, &["init", "-b", "epic/takeover"]);
        git(&project, &["config", "user.name", "Takeover fixture"]);
        git(&project, &["config", "user.email", "takeover@example.test"]);
        std::fs::write(project.join(".gitignore"), ".cas/\n").unwrap();
        git(&project, &["add", ".gitignore"]);
        git(&project, &["commit", "-qm", "base"]);
        let base = git(&project, &["rev-parse", "HEAD"]);
        let root = init_cas_dir(&project).unwrap();
        std::fs::write(root.join("config.toml"), concat!(
            "[project]\ncanonical_id = 'takeover-fixture'\n",
            "[verification]\nenabled = false\n",
            "[qa]\nmode = 'batch'\nindependent_pass = false\nevidence_gate = false\n",
        )).unwrap();
        let origin = root.join("origin.git");
        git(&project, &["init", "--bare", origin.to_str().unwrap()]);
        git(&project, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&project, &["push", "-q", "origin", "epic/takeover"]);
        let old_branch = "factory/retired-worker-cas-d6b1";
        git(&project, &["switch", "-c", old_branch]);
        std::fs::write(project.join("delivery.txt"), "preserved original work\n").unwrap();
        git(&project, &["add", "delivery.txt"]);
        git(&project, &["commit", "-qm", "cas-d6b1: original delivery"]);
        let anchor = git(&project, &["rev-parse", "HEAD"]);
        git(&project, &["push", "-q", "origin", old_branch]);
        git(&project, &["switch", "epic/takeover"]);
        let branch = "factory/replacement-worker-cas-d6b1";
        let checkout = root.join("worktrees/replacement-worker");
        git(&project, &["worktree", "add", "-b", branch, checkout.to_str().unwrap(), old_branch]);
        let agents = open_agent_store(&root).unwrap();
        for (id, name, role, gone) in [
            ("supervisor-id", "supervisor", AgentRole::Supervisor, false),
            ("retired-id", "retired-worker", AgentRole::Worker, true),
            ("replacement-id", "replacement-worker", AgentRole::Worker, false),
        ] {
            let mut agent = Agent::new_with_role(id.into(), name.into(), role);
            agent.factory_session = Some("takeover-session".into());
            if gone {
                agent.status = AgentStatus::Shutdown;
                agent.last_heartbeat -= chrono::Duration::hours(1);
            }
            if name == "replacement-worker" {
                agent.metadata.insert("clone_path".into(), checkout.display().to_string());
            }
            agents.register(&agent).unwrap();
        }
        let tasks = open_task_store(&root).unwrap();
        let mut task = Task::new("cas-d6b1".into(), "Recover retired worker delivery".into());
        task.depth = TaskDepth::Light;
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some("retired-worker".into());
        let target = WorkTarget { repo_selector: "project:takeover-fixture".into(), target_branch: "epic/takeover".into() };
        task.deliverables.work_target = Some(target.clone());
        task.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: target, assignee: "retired-worker".into(), branch: old_branch.into(), base_sha: base.clone(),
        });
        task.deliverables.parked_branch = Some(old_branch.into());
        task.deliverables.factory_branch_anchor = Some(anchor.clone());
        tasks.add(&task).unwrap();
        assert!(agents.try_claim(&task.id, "retired-id", 600, None).unwrap().is_success());
        assert!(crate::ui::factory::app::render_and_ops::epic_workers::assign_task_to_new_worker(
            &root, &task.id, "replacement-worker",
        ));
        let reassigned = tasks.get(&task.id).unwrap();
        assert_eq!(reassigned.status, TaskStatus::Open);
        assert!(reassigned.notes.contains("force-released"));
        assert!(!agents.get_lease(&task.id).unwrap().unwrap().is_valid());
        assert_eq!(reassigned.deliverables.factory_branch_start, task.deliverables.factory_branch_start);
        let service = |id: &str| {
            let core = CasCore::with_daemon(root.clone(), None, None);
            core.set_agent_id_for_testing(id.into());
            CasService::new(core, None)
        };
        let worker = service("replacement-id");
        let supervisor = service("supervisor-id");
        let request = |value| Parameters(serde_json::from_value(value).unwrap());
        env.set_current_dir(&checkout);
        worker.task(request(serde_json::json!({"action": "start", "id": task.id}))).await.unwrap();
        supervisor.task(request(serde_json::json!({
            "action": "rebind_branch", "id": task.id, "branch": branch,
            "reason": "Original worker retired; approve replacement delivery",
        }))).await.expect("supervisor can approve the new assignee's preserved delivery");
        let rebound = tasks.get(&task.id).unwrap();
        let start = rebound.deliverables.factory_branch_start.as_ref().unwrap();
        assert_eq!(start.assignee, "replacement-worker");
        assert_eq!(start.branch, branch);
        assert_eq!(start.base_sha, base);
        assert!(rebound.deliverables.handoff_branches.contains(&old_branch.into()));
        assert!(rebound.deliverables.historical_factory_branch_anchors.contains(&anchor));
        let audit = rebound.notes.lines().find_map(|line| line.split_once("DELIVERY BRANCH REBOUND: "))
            .map(|(_, json)| serde_json::from_str::<serde_json::Value>(json).unwrap()).unwrap();
        assert_eq!(audit["previous_start"], serde_json::to_value(task.deliverables.factory_branch_start).unwrap());
        assert_eq!(audit["previous_tip"], anchor);
        // The replacement can now continue, park and merge under its own name.
        std::fs::write(checkout.join("recovery.txt"), "replacement work\n").unwrap();
        git(&checkout, &["add", "recovery.txt"]);
        git(&checkout, &["commit", "-qm", "cas-d6b1: complete takeover"]);
        let tip = git(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["push", "-q", "origin", branch]);
        worker.task(request(serde_json::json!({"action": "close", "id": task.id, "reason": "Takeover validated"}))).await.unwrap();
        assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::AwaitingMerge);
        env.set_current_dir(&project);
        let merged = supervisor.coordination(Parameters(serde_json::from_value(serde_json::json!({
            "action": "worktree_merge", "id": branch, "task_id": task.id, "cleanup": false,
        })).unwrap())).await.expect("recorded replacement source resolves and merges");
        assert_ne!(merged.is_error, Some(true), "{merged:?}");
        assert_eq!(git(&project, &["show", "epic/takeover:recovery.txt"]), "replacement work");
        git(&project, &["merge-base", "--is-ancestor", &tip, "epic/takeover"]);
        assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::Closed, "{merged:?}");
    }
}
