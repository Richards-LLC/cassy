//! Structured provenance for non-dependency task holds.

use cas_store::TaskStore;
use cas_types::{Task, TaskAnsweredBlocker as AnsweredBlocker, TaskBlockedHold};

/// A supervisor owns this unchanged wait, even if verification briefly reopened
/// the task. Times on holds/idle episodes are observations, not wait identity.
pub(crate) fn record_blocker_ask(
    cas_dir: &std::path::Path, worker: &cas_types::Agent, task_id: &str, ask: &str,
) -> Result<(), String> {
    let tasks = crate::store::open_task_store_local(cas_dir).map_err(|e| e.to_string())?;
    let task = tasks.get(task_id).map_err(|e| e.to_string())?;
    if worker.role != cas_types::AgentRole::Worker
        || ![Some(worker.name.as_str()), Some(worker.id.as_str())].contains(&task.assignee.as_deref())
    { return Ok(()); }
    use sha2::{Digest, Sha256};
    let fingerprint = format!("{:x}", Sha256::digest(ask.as_bytes()));
    tasks.patch_execution_state(task_id, &serde_json::json!({"blocker_ask": fingerprint}))
        .map_err(|e| e.to_string())
}

/// Called only after a real registered supervisor's direct message commits.
/// An unbound message cannot guess among multiple blocked tasks.
pub(crate) fn record_supervisor_answer(
    cas_dir: &std::path::Path, supervisor: &cas_types::Agent, worker: &cas_types::Agent,
    task_id: Option<&str>, message_id: i64,
) -> Result<(), String> {
    if supervisor.role != cas_types::AgentRole::Supervisor || worker.role != cas_types::AgentRole::Worker
        || supervisor.factory_session != worker.factory_session
    { return Ok(()); }
    let tasks = crate::store::open_task_store_local(cas_dir).map_err(|e| e.to_string())?;
    let blocked = tasks.list(Some(cas_types::TaskStatus::Blocked)).map_err(|e| e.to_string())?;
    let candidates: Vec<_> = blocked.iter().filter(|task| {
        task_id.is_none_or(|id| task.id == id)
            && [Some(worker.name.as_str()), Some(worker.id.as_str())].contains(&task.assignee.as_deref())
    }).collect();
    let [task] = candidates.as_slice() else { return Ok(()); };
    let Some(hold) = read(tasks.as_ref(), &task.id)? else { return Ok(()); };
    let state = tasks.get_execution_state(&task.id).map_err(|e| e.to_string())?.unwrap_or_default();
    let receipt = AnsweredBlocker {
        worker: worker.id.clone(), supervisor: supervisor.id.clone(), reason: hold.reason,
        ask: state.get("blocker_ask").and_then(serde_json::Value::as_str).map(str::to_owned), message_id,
    };
    tasks.patch_execution_state(&task.id, &serde_json::json!({"answered_blocker": receipt}))
        .map_err(|e| e.to_string())
}

/// Positive, durable evidence only. New questions/holds, dependencies, changed
/// owners and non-blocked task states retain their normal wake policy.
pub(crate) fn answered_blocker_wait(
    cas_dir: &std::path::Path, task_id: &str, recipient: &str,
) -> Option<i64> {
    let tasks = crate::store::open_task_store_local(cas_dir).ok()?;
    let task = tasks.get(task_id).ok()?;
    if task.status != cas_types::TaskStatus::Blocked || !tasks.get_blockers(task_id).ok()?.is_empty()
        || !cas_store::ExternalTaskDependencyStore::open(cas_dir).ok()?
            .list_blocking_for_task(task_id).ok()?.is_empty()
    { return None; }
    let state = tasks.get_execution_state(task_id).ok()??;
    let receipt: AnsweredBlocker = serde_json::from_value(state.get("answered_blocker")?.clone()).ok()?;
    let agents = crate::store::open_agent_store(cas_dir).ok()?;
    let supervisor = agents.get(&receipt.supervisor).ok()?;
    let worker = agents.get(&receipt.worker).ok()?;
    if supervisor.role != cas_types::AgentRole::Supervisor || worker.role != cas_types::AgentRole::Worker
        || ![supervisor.name.as_str(), supervisor.id.as_str(), "supervisor"].contains(&recipient)
        || supervisor.factory_session != worker.factory_session
        || ![Some(worker.name.as_str()), Some(worker.id.as_str())].contains(&task.assignee.as_deref())
        || read(tasks.as_ref(), task_id).ok()??.reason != receipt.reason
        || state.get("blocker_ask").and_then(serde_json::Value::as_str) != receipt.ask.as_deref()
    { return None; }
    Some(receipt.message_id)
}

pub(crate) fn worker_has_answered_blocker(
    cas_dir: &std::path::Path, worker: &str, recipient: &str,
) -> bool {
    let Some(agents) = crate::store::open_agent_store(cas_dir).ok() else { return false; };
    let Some(worker) = super::resolve_agent_identity(agents.as_ref(), worker) else { return false; };
    let Some(tasks) = crate::store::open_task_store_local(cas_dir).ok() else { return false; };
    let Some(assigned) = tasks.ids_by_assignees_and_statuses(&[&worker.name, &worker.id], &[
        cas_types::TaskStatus::Open, cas_types::TaskStatus::InProgress, cas_types::TaskStatus::Blocked,
        cas_types::TaskStatus::AwaitingMerge,
    ]).ok() else { return false; };
    !assigned.is_empty() && assigned.iter().all(|task| answered_blocker_wait(cas_dir, task, recipient).is_some())
}

/// Re-check queued alerts as well as newly emitted events. An answered row
/// remains readable; only its wake is withdrawn.
pub(crate) fn answered_blocker_alert(cas_dir: &std::path::Path, row: &cas_store::QueuedPrompt) -> bool {
    if let Some(envelope) = crate::prompt_revalidation::parse_lifecycle_envelope(&row.prompt) {
        return envelope.new_status == cas_types::TaskStatus::Blocked
            && answered_blocker_wait(cas_dir, &envelope.task_id, &row.target).is_some();
    }
    if cas_types::parse_worker_attention_envelope(&row.prompt).is_some()
        && let Some(tag) = row.prompt.split('>').next()
        && tag.contains("kind=\"worker_idle\"")
        && let Some((_, worker)) = tag.split_once("worker=\"")
        && let Some(worker) = worker.split('"').next()
    {
        return worker_has_answered_blocker(cas_dir, worker, &row.target);
    }
    false
}

/// Positive evidence that this recipient already owns the only recorded wait.
/// Unknown state, independent holds and dependency blockers retain the alert.
pub(crate) fn owned_verification_wait(
    cas_dir: &std::path::Path,
    task_id: &str,
    recipient: &str,
) -> Option<String> {
    use cas_types::{AgentRole, TaskStatus, VerificationDispatchState};
    let dispatch = cas_store::get_latest_verification_dispatch(cas_dir, task_id).ok()??;
    if !matches!(
        dispatch.state,
        VerificationDispatchState::Pending | VerificationDispatchState::Claimed
    ) || dispatch.deadline_at <= chrono::Utc::now()
    {
        return None;
    }
    let agents = crate::store::open_agent_store(cas_dir).ok()?;
    let recipient = agents.get(recipient).ok().or_else(|| {
        agents
            .list(None)
            .ok()?
            .into_iter()
            .filter(|agent| agent.name == recipient && crate::supervisor_notices::is_live(agent))
            .max_by_key(|agent| agent.registered_at)
    })?;
    if recipient.role != AgentRole::Supervisor || recipient.id != dispatch.owner_agent_id {
        return None;
    }
    let tasks = crate::store::open_task_store_local(cas_dir).ok()?;
    let task = tasks.get(task_id).ok()?;
    if task.status != TaskStatus::Blocked
        || !tasks.get_blockers(task_id).ok()?.is_empty()
        || !cas_store::ExternalTaskDependencyStore::open(cas_dir)
            .ok()?
            .list_blocking_for_task(task_id)
            .ok()?
            .is_empty()
    {
        return None;
    }
    let is_wait = |reason: &str| {
        reason.contains(&dispatch.id) && reason.to_ascii_lowercase().contains("verif")
    };
    let hold = read(tasks.as_ref(), task_id).ok()?;
    if hold.as_ref().is_some_and(|hold| {
        hold.reason != "Task explicitly set to status=blocked." && !is_wait(&hold.reason)
    }) {
        return None;
    }
    // A new park starts a delivery cycle; old blocker notes cannot hold it.
    let notes = task.notes.rsplit("Task parked as awaiting_merge").next()?;
    let blockers: Vec<_> = notes
        .lines()
        .filter_map(|line| line.split_once("🚫 BLOCKER").map(|(_, body)| body))
        .collect();
    if blockers.iter().any(|reason| !is_wait(reason))
        || (!blockers.iter().any(|reason| is_wait(reason))
            && !hold.as_ref().is_some_and(|hold| is_wait(&hold.reason)))
    {
        return None;
    }
    Some(dispatch.id)
}

/// Read the hold that controls whether dependency auto-unblock may reopen a task.
pub(crate) fn read(
    task_store: &dyn TaskStore,
    task_id: &str,
) -> Result<Option<TaskBlockedHold>, String> {
    let Some(state) = task_store
        .get_execution_state(task_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some(hold) = state.get("blocked_hold") else {
        return Ok(None);
    };
    serde_json::from_value(hold.clone())
        .map(Some)
        .map_err(|error| format!("invalid structured blocked hold: {error}"))
}

/// Preserve a human-readable reason while keeping actor and time in state.
pub(crate) fn explicit(reason: Option<&str>, who: &str) -> TaskBlockedHold {
    TaskBlockedHold {
        reason: reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("Task explicitly set to status=blocked.")
            .to_string(),
        who: if who.trim().is_empty() {
            "unknown".to_string()
        } else {
            who.to_string()
        },
        when: chrono::Utc::now(),
    }
}

/// Auto-unblock notes are audit only; they never determine hold state.
pub(crate) fn record_auto_unblock_skipped(task: &mut Task, reason: &str) {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let note = format!(
        "[{timestamp}] ⏸ Auto-unblock skipped: dependency blockers cleared; task remains blocked because {reason}."
    );
    if task.notes.is_empty() {
        task.notes = note;
    } else {
        task.notes.push_str("\n\n");
        task.notes.push_str(&note);
    }
    task.updated_at = chrono::Utc::now();
}
