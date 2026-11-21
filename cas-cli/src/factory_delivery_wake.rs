//! Supervisor-owned completion edges for parked deliveries. The existing
//! reminder observers own probing/firing; this module only persists watches.
use std::path::Path;

use anyhow::{Context, Result};
use cas_store::{ReminderStatus, ReminderTriggerType};
use cas_types::{Agent, Task, TaskStatus};
use fs2::FileExt;
use serde_json::{Value, json};

use crate::mcp::tools::core::task::lifecycle::{close_ops, supervisor_push};
use crate::ui::factory::daemon::runtime::ci_watch;

pub(crate) const MISSING_WAKE: &str = "DELIVERY_WAKE_MISSING";

pub(crate) fn full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn owner(root: &Path, task: &Task, actor: &str) -> Result<Option<Agent>> {
    let agents = crate::store::open_agent_store(root)?;
    let registered = agents.list(None)?;
    let actor = task.assignee.as_deref().and_then(|assignee| registered.iter()
        .find(|agent| agent.id == assignee || agent.name == assignee))
        .or_else(|| registered.iter().find(|agent| agent.id == actor || agent.name == actor));
    let Some(actor) = actor else { return Ok(None) };
    if let Some(supervisor) = crate::factory_session_scope::adopted_worker_supervisor(root, actor) {
        return Ok(Some(supervisor));
    }
    let Some(supervisor) = supervisor_push::resolve_worker_supervisor(
        agents.as_ref(), actor.factory_session.as_deref(),
    ) else { return Ok(None) };
    Ok(Some(agents.get(&supervisor.agent_id)?))
}

/// Serialize competing park/message/CI observations before pending-row dedup.
pub(crate) fn arm_event(
    root: &Path, supervisor: &Agent, task: &str, event: &str, filter: &Value, rearm: bool,
) -> Result<()> {
    let mut filter = filter.clone();
    filter["delivery_wake"] = Value::Bool(true);
    let lock = std::fs::OpenOptions::new().create(true).truncate(false)
        .read(true).write(true).open(root.join("delivery-wakes.lock"))?;
    lock.lock_exclusive()?;
    let reminders = crate::store::open_reminder_store(root)?;
    if reminders.list_for_task(task)?.iter().any(|row| {
        row.owner_id == supervisor.id && row.target_id == supervisor.id
            && row.trigger_event.as_deref() == Some(event)
            && row.trigger_filter.as_ref() == Some(&filter)
            && (row.status == ReminderStatus::Pending || (!rearm && row.status == ReminderStatus::Fired))
    }) { return Ok(()); }
    reminders.create_with_scope(
        &supervisor.id, None,
        &format!("Delivery completion for {task}: {event} {filter}. Inspect the authoritative delivery and act on this result."),
        ReminderTriggerType::Event, None, Some(event), Some(&filter), 0,
        supervisor.factory_session.as_deref(), Some(&supervisor.id), true, Some(task),
    )?;
    Ok(())
}

/// cas-e37f: a parked delivery's completion edges, decided from store reads
/// and git; [`arm_plan`] only persists them, so a caller on the store writer
/// can decide off it.
#[derive(Debug, Clone)]
pub(crate) struct ArmPlan {
    supervisor: Agent,
    pub(crate) task: String,
    sha: String,
    target: String,
    ci: bool,
}

pub(crate) fn plan_task(root: &Path, task: &Task, actor: &str) -> Result<Option<ArmPlan>> {
    if task.status != TaskStatus::AwaitingMerge { return Ok(None); }
    let Some(supervisor) = owner(root, task, actor)? else { return Ok(None) };
    let sha = task.deliverables.factory_branch_anchor.as_deref()
        .filter(|sha| full_sha(sha)).context("parked delivery has no full commit SHA")?;
    let tasks = crate::store::open_task_store_local(root)?;
    let epic = tasks.get_parent_epic(&task.id)?;
    let target = if let Some(target) = close_ops::effective_close_work_target(task, epic.as_ref()) {
        target.target_branch
    } else {
        let worktree_parent = task.worktree_id.as_deref().and_then(|id|
            crate::store::open_worktree_store(root).ok()?.get(id).ok().map(|worktree| worktree.parent_branch));
        close_ops::resolve_close_parent_branch(
            worktree_parent, epic.as_ref().and_then(|parent| parent.branch.clone()),
            epic.as_ref().and_then(|parent| parent.deliverables.work_target.as_ref().map(|target| target.target_branch.clone())),
            root.parent().unwrap_or(root),
        ).map_err(anyhow::Error::msg)?
    };
    let target = ci_watch::normalize_external_target_branch(&target).to_string();
    // cas-be96: a CI watch on a commit already in its target, or for a
    // workflow the repository does not define, polls GitHub forever. The
    // containment watch still reports the landing.
    let project = root.parent().unwrap_or(root);
    let ci = !contained(project, sha, &target) && ci_watch::local_workflow_defined(project, "CI");
    Ok(Some(ArmPlan { supervisor, task: task.id.clone(), sha: sha.to_string(), target, ci }))
}

pub(crate) fn arm_plan(root: &Path, plan: &ArmPlan) -> Result<()> {
    arm_event(root, &plan.supervisor, &plan.task, "branch_contained_in",
        &json!({"commit": plan.sha, "target_branch": plan.target}), false)?;
    if !plan.ci { return Ok(()); }
    arm_event(root, &plan.supervisor, &plan.task, "ci_completed",
        &json!({"sha": plan.sha, "workflow": "CI"}), false)
}

pub(crate) fn arm_task(root: &Path, task: &Task, actor: &str) -> Result<()> {
    match plan_task(root, task, actor)? {
        Some(plan) => arm_plan(root, &plan),
        None => Ok(()),
    }
}

/// The commit is already on the target, locally or on its fetched remote.
fn contained(project: &Path, sha: &str, target: &str) -> bool {
    [format!("refs/remotes/origin/{target}"), format!("refs/heads/{target}")].iter().any(|reference| {
        let args = ["merge-base", "--is-ancestor", sha, reference.as_str()].map(String::from);
        ci_watch::run_external_git_command(project, &args).is_ok_and(|output| output.success)
    })
}

/// A retried close has committed a new delivery boundary. Establish its
/// watches before retiring automatic watches for the superseded boundary;
/// explicit manual reminders keep their own lifecycle.
pub(crate) fn advance_task(root: &Path, task: &Task, actor: &str, previous: Option<&str>) -> Result<()> {
    arm_task(root, task, actor)?;
    let Some(previous) = previous.filter(|old| Some(*old) != task.deliverables.factory_branch_anchor.as_deref()) else { return Ok(()) };
    let lock = std::fs::OpenOptions::new().create(true).truncate(false)
        .read(true).write(true).open(root.join("delivery-wakes.lock"))?;
    lock.lock_exclusive()?;
    let reminders = crate::store::open_reminder_store(root)?;
    for row in reminders.list_for_task(&task.id)? {
        if matches!(row.status, ReminderStatus::Pending | ReminderStatus::Suspended)
            && matches!(row.trigger_event.as_deref(), Some("ci_completed" | "branch_contained_in"))
            && row.trigger_filter.as_ref().is_some_and(|filter| filter["delivery_wake"] == true
                && filter.get("sha").or_else(|| filter.get("commit")).and_then(Value::as_str) == Some(previous))
        {
            match reminders.cancel(row.id, &row.owner_id) {
                Ok(()) | Err(cas_store::StoreError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

pub(crate) fn arm_task_id(root: &Path, task: &str, actor: &str) {
    let result = (|| -> Result<()> {
        let task = crate::store::open_task_store_local(root)?.get(task)?;
        arm_task(root, &task, actor)
    })();
    if let Err(error) = result {
        tracing::error!(%task, %error, "delivery completion wake not armed");
    }
}

/// A completion already observed on the target needs local follow-up, not a
/// new external edge. Pending CI or containment watches cover future work.
pub(crate) fn watched(root: &Path, task: &Task, supervisor: &Agent) -> Result<bool> {
    let Some(sha) = task.deliverables.factory_branch_anchor.as_deref() else { return Ok(false) };
    let agents = crate::store::open_agent_store(root)?;
    Ok(crate::store::open_reminder_store(root)?.list_for_task(&task.id)?.iter().any(|row| {
        row.owner_id == row.target_id && row.cross_session
            && (row.owner_id == supervisor.id || (agents.get(&row.owner_id).is_ok_and(|owner| owner.role == cas_types::AgentRole::Supervisor)
                && supervisor_push::resolve_notice_recipient(agents.as_ref(), row.session_id.as_deref()).id == supervisor.id))
            && row.trigger_filter.as_ref().is_some_and(|filter| {
                filter.get("sha").or_else(|| filter.get("commit")).and_then(Value::as_str) == Some(sha)
            })
            && match row.trigger_event.as_deref() {
                Some("ci_completed" | "branch_contained_in") if row.status == ReminderStatus::Pending => true,
                Some("branch_contained_in") if row.status == ReminderStatus::Fired => true,
                _ => false,
            }
    }))
}
