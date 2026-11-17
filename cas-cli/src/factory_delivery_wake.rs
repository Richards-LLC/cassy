//! Supervisor-owned completion edges for parked deliveries. The existing
//! reminder observers own probing/firing; this module only persists watches.
use std::path::Path;

use anyhow::{Context, Result};
use cas_store::{ReminderStatus, ReminderTriggerType};
use cas_types::{Agent, Task, TaskStatus};
use fs2::FileExt;
use serde_json::{Value, json};

use crate::mcp::tools::core::task::lifecycle::supervisor_push;

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

pub(crate) fn arm_task(root: &Path, task: &Task, actor: &str) -> Result<()> {
    if task.status != TaskStatus::AwaitingMerge { return Ok(()); }
    let Some(supervisor) = owner(root, task, actor)? else { return Ok(()) };
    let sha = task.deliverables.factory_branch_anchor.as_deref()
        .filter(|sha| full_sha(sha)).context("parked delivery has no full commit SHA")?;
    let target = task.deliverables.work_target.as_ref().map(|target| target.target_branch.as_str())
        .or(task.branch.as_deref()).context("parked delivery has no target branch")?;
    let target = crate::ui::factory::daemon::runtime::ci_watch::normalize_external_target_branch(target);
    arm_event(root, &supervisor, &task.id, "branch_contained_in",
        &json!({"commit": sha, "target_branch": target}), false)?;
    arm_event(root, &supervisor, &task.id, "ci_completed",
        &json!({"sha": sha, "workflow": "CI"}), false)
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
