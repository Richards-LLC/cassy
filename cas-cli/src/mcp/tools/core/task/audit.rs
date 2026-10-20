//! Canonical lifecycle audit writers. Replication consumes these same prefixes.
use cas_types::{Agent, TaskStatus};
use chrono::{DateTime, Utc};

pub(crate) const REOPENED: &str = "Reopened:";
pub(crate) const REVIEW_SCOPE_RESET: &str = "Review scope reset:";
pub(crate) const UNBLOCKED: &str = "Unblocked:";
pub(crate) const PROOF_SCOPE_FIX: &str = "DECISION: proof scope corrected by supervisor ";
pub(crate) const MERGE_RECOVERY: &str = "Decision: resume from awaiting_merge for merge recovery. ";
pub(crate) const CANCELLED: &str = "CANCELLED: supervisor ";
pub(crate) const RESET: &str = "reset: force-transitioned ";
pub(crate) const PARKED: &str = "Close rejected:";
pub(crate) const PARKED_STATE: &str = "Task parked as awaiting_merge;";
pub(crate) const CLOSED: &str = "Closed:";

pub(crate) fn reopen_note(
    at: DateTime<Utc>,
    old: TaskStatus,
    fresh_scope: bool,
    actor: &Agent,
    reason: &str,
) -> String {
    let prefix = if fresh_scope && old != TaskStatus::Closed {
        REVIEW_SCOPE_RESET
    } else if old == TaskStatus::Blocked {
        UNBLOCKED
    } else {
        REOPENED
    };
    let actor = format!("{} ({})", actor.name, actor.id).replace(['\n', '\r'], " ");
    let reason = reason.replace(['\n', '\r'], " ");
    format!(
        "[{}] {prefix} actor={actor} reason={reason}",
        at.format("%Y-%m-%d %H:%M")
    )
}

pub(crate) fn proof_scope_fix_note(
    at: DateTime<Utc>,
    actor: &Agent,
    reason: &str,
    target: &str,
) -> String {
    format!(
        "[{}] {PROOF_SCOPE_FIX}{} ({}): {} {} The prior exact proof cycle was invalidated; no failed-review verdict was recorded.",
        at.format("%Y-%m-%d %H:%M"),
        actor.name,
        actor.id,
        reason,
        target
    )
}

pub(crate) fn merge_recovery_note(at: DateTime<Utc>, branch: &str) -> String {
    format!(
        "[{}] {MERGE_RECOVERY}{branch} was flagged with a merge conflict or its conflict preflight could not be evaluated, so the task is back in_progress for the assigned worker to inspect from inside the worker worktree. Rebase {branch} onto the current integration target tip, push the rebased factory branch, and re-park it with merge_request=true.",
        at.format("%Y-%m-%d %H:%M")
    )
}

pub(crate) fn cancel_note(
    at: DateTime<Utc>,
    actor: &Agent,
    reason: &str,
    superseded_by: Option<&str>,
) -> String {
    let pointer = superseded_by
        .map(|value| format!(" Superseded by: {value}."))
        .unwrap_or_default();
    format!(
        "[{}] {CANCELLED}{} ({}) ended this task without delivery. Reason: {}{}",
        at.format("%Y-%m-%d %H:%M"),
        actor.name,
        actor.id,
        reason,
        pointer
    )
}

pub(crate) fn reset_note(
    at: DateTime<Utc>,
    prior: TaskStatus,
    assignee: Option<&str>,
    lease_released: bool,
    recovery: &str,
) -> String {
    format!(
        "[{}] {RESET}{prior:?}→Open (prior assignee: {}, lease released: {lease_released}) — {recovery}",
        at.format("%Y-%m-%d %H:%M"),
        assignee.unwrap_or("<none>")
    )
}

pub(crate) fn parked_note(at: DateTime<Utc>, reason: &str) -> String {
    format!(
        "[{}] {PARKED} {reason}. {PARKED_STATE} worker lease released until supervisor merge completes.",
        at.format("%Y-%m-%d %H:%M")
    )
}

pub(crate) fn closed_note(at: DateTime<Utc>, reason: &str) -> String {
    format!("[{}] {CLOSED} {reason}", at.format("%Y-%m-%d %H:%M"))
}
