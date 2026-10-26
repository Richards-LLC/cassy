//! Structured provenance for non-dependency task holds.

use cas_store::TaskStore;
use cas_types::{Task, TaskBlockedHold};

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
    let owner = crate::store::open_agent_store(cas_dir)
        .ok()?
        .get(&dispatch.owner_agent_id)
        .ok()?;
    if owner.role != AgentRole::Supervisor || (recipient != owner.id && recipient != owner.name) {
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
