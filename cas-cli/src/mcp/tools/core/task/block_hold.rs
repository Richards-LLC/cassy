//! Structured provenance for non-dependency task holds.

use cas_store::TaskStore;
use cas_types::{Task, TaskBlockedHold};

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
