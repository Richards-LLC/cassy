//! Structured provenance for non-dependency task holds.

use cas_store::TaskStore;
use cas_types::{Task, TaskAnsweredBlocker as AnsweredBlocker, TaskBlockedHold};

/// Bind the active worker question without copying its prose into resume state.
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
        .map(|_| ()).map_err(|e| e.to_string())
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
        .map(|_| ()).map_err(|e| e.to_string())
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
    if row.origin != Some(cas_store::QueueOrigin::Daemon) { return false; }
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
    owned_verification_wait_at(cas_dir, task_id, recipient, chrono::Utc::now())
}

pub(crate) fn owned_verification_wait_at(
    cas_dir: &std::path::Path,
    task_id: &str,
    recipient: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    use cas_types::{AgentRole, VerificationDispatchState};
    let dispatch = cas_store::get_latest_verification_dispatch(cas_dir, task_id).ok()??;
    if !matches!(
        dispatch.state,
        VerificationDispatchState::Pending | VerificationDispatchState::Claimed
    ) || dispatch.deadline_at <= now
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
    blocked_only_by_verification_wait(cas_dir, task_id, &dispatch.id).then_some(dispatch.id)
}

/// A blocker episode survives activity writes, supervisor turns and daemon
/// restarts. A new hold, status transition, assignee or verification cycle is a
/// new episode. Legacy tasks without a hold use their durable status event.
pub(crate) fn alert_episode(
    cas_dir: &std::path::Path,
    task: &Task,
) -> Result<(chrono::DateTime<chrono::Utc>, String), String> {
    let tasks = crate::store::open_task_store_local(cas_dir).map_err(|e| e.to_string())?;
    let hold = read(tasks.as_ref(), &task.id)?;
    let dispatch = cas_store::get_latest_verification_dispatch(cas_dir, &task.id)
        .map_err(|e| e.to_string())?
        .filter(|dispatch| blocked_only_by_verification_wait(cas_dir, &task.id, &dispatch.id));
    let start = if let Some(hold) = hold {
        hold.when
    } else if let Some(dispatch) = &dispatch {
        dispatch.requested_at
    } else {
        let events = crate::store::open_event_store(cas_dir).map_err(|e| e.to_string())?;
        events.list_for_entity(cas_types::EventEntityType::Task, &task.id, 256)
            .map_err(|e| e.to_string())?.into_iter()
            .find(|event| event.event_type == cas_types::EventType::TaskBlocked)
            .map(|event| event.created_at).unwrap_or(task.created_at)
    };
    let key = serde_json::json!([
        task.assignee,
        start.to_rfc3339(),
        dispatch.map(|dispatch| dispatch.id)
    ])
    .to_string();
    Ok((start, key))
}

/// cas-7fc3: whether `task_id` is Blocked by nothing but the exact
/// verification-wait note for `dispatch_id`. Dependency blockers, external
/// dependencies, an independent hold or any other blocker note all mean a
/// different gate holds the task.
pub(crate) fn blocked_only_by_verification_wait(
    cas_dir: &std::path::Path,
    task_id: &str,
    dispatch_id: &str,
) -> bool {
    blocked_only_by_verification_wait_inner(cas_dir, task_id, dispatch_id).unwrap_or(false)
}

fn blocked_only_by_verification_wait_inner(
    cas_dir: &std::path::Path,
    task_id: &str,
    dispatch_id: &str,
) -> Option<bool> {
    use cas_types::TaskStatus;
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
        return Some(false);
    }
    // cas-16e1: a wait is the exact system-authored sentence for this
    // dispatch, never any text that merely mentions it.
    let wait = verification_wait_note(dispatch_id);
    let is_wait = |reason: &str| reason.trim() == wait;
    let hold = read(tasks.as_ref(), task_id).ok()?;
    if hold.as_ref().is_some_and(|hold| {
        hold.reason != "Task explicitly set to status=blocked."
            && !is_any_verification_wait_note(&hold.reason)
    }) {
        return Some(false);
    }
    // A new park starts a delivery cycle; old blocker notes cannot hold it.
    let notes = task.notes.rsplit("Task parked as awaiting_merge").next()?;
    // cas-7fc3: only blocker notes written in this dispatch's cycle can hold
    // it. An earlier, answered blocker ("need credentials") and a wait note
    // for an earlier dispatch both belong to finished cycles. Note stamps are
    // UTC minutes, so the bound is the request minute.
    let since = cas_store::get_verification_dispatch(cas_dir, dispatch_id)
        .ok()
        .map(|dispatch| dispatch.requested_at.format("%Y-%m-%d %H:%M").to_string());
    let blockers: Vec<String> = blocker_note_entries(notes)
        .into_iter()
        .filter(|(stamp, _)| since.as_deref().is_none_or(|since| stamp.as_str() >= since))
        .map(|(_, reason)| reason)
        .filter(|reason| is_wait(reason) || !is_any_verification_wait_note(reason))
        .collect();
    Some(
        !(blockers.iter().any(|reason| !is_wait(reason))
            || (!blockers.iter().any(|reason| is_wait(reason))
                && !hold.as_ref().is_some_and(|hold| is_wait(&hold.reason)))),
    )
}

/// cas-16e1: the one note a worker records to wait, blocked, on a pending
/// verification dispatch without waking its supervisor.
pub(crate) fn verification_wait_note(dispatch_id: &str) -> String {
    format!("Verification required: exact dispatch {dispatch_id} must record a verdict before close.")
}

/// Whether `reason` is exactly a [`verification_wait_note`] for some single
/// dispatch id, as opposed to free text that merely mentions one.
fn is_any_verification_wait_note(reason: &str) -> bool {
    reason
        .trim()
        .strip_prefix("Verification required: exact dispatch ")
        .and_then(|rest| rest.strip_suffix(" must record a verdict before close."))
        .is_some_and(|id| !id.is_empty() && !id.contains(char::is_whitespace))
}

/// The full text of every blocker note, across all of its lines. A note
/// entry starts at a `[YYYY-MM-DD HH:MM] ` line and runs to the next one.
/// Each entry is returned with its `YYYY-MM-DD HH:MM` stamp.
fn blocker_note_entries(notes: &str) -> Vec<(String, String)> {
    let starts_entry = |line: &str| {
        let bytes = line.as_bytes();
        bytes.len() > 19 && bytes[0] == b'[' && bytes[17] == b']' && bytes[18] == b' '
            && line[1..17].chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | ' ' | ':'))
    };
    let mut entries: Vec<String> = Vec::new();
    for line in notes.lines() {
        match entries.last_mut() {
            Some(entry) if !starts_entry(line) => {
                entry.push('\n');
                entry.push_str(line);
            }
            _ => entries.push(line.to_string()),
        }
    }
    entries.into_iter()
        .filter_map(|entry| {
            let body = entry.get(19..)?.strip_prefix("🚫 BLOCKER")?.trim().to_string();
            Some((entry.get(1..17)?.to_string(), body))
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use cas_store::TaskStore;
    use cas_types::{Agent, AgentRole, TaskStatus};

    /// cas-16e1: only the exact wait sentence, read across the whole note,
    /// keeps a verification wait quiet; a blocker that mentions the dispatch
    /// while asking for something else keeps alerts armed.
    #[test]
    fn only_the_whole_wait_note_is_a_quiet_verification_wait_cas_16e1() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let repo = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        crate::store::open_task_store_local(&cas_dir).unwrap()
            .add(&cas_types::Task::new("cas-9efb-test".into(), "verification wait".into())).unwrap();
        let lead = Agent::new_with_role("lead-id".into(), "lead".into(), AgentRole::Supervisor);
        crate::store::open_agent_store(&cas_dir).unwrap().register(&lead).unwrap();
        let dispatch = cas_store::create_verification_dispatch(
            &cas_dir, "cas-9efb-test", "merge-w", "lead-id",
            chrono::Utc::now() + chrono::Duration::minutes(10),
        ).unwrap();
        let store = crate::store::open_task_store_local(&cas_dir).unwrap();
        let wait = super::verification_wait_note(&dispatch.id);
        for (note, quiet) in [
            (wait.clone(), true),
            (format!("{wait}\nAlso restore the staging database credentials."), false),
            (format!("Verification of {} is stuck; I need the staging credentials.", dispatch.id), false),
        ] {
            let mut task = store.get("cas-9efb-test").unwrap();
            task.status = TaskStatus::Blocked;
            // cas-7fc3: notes count from the dispatch's request minute on.
            task.notes = format!(
                "[{}] 🚫 BLOCKER {note}",
                dispatch.requested_at.format("%Y-%m-%d %H:%M")
            );
            store.update(&task).unwrap();
            assert_eq!(
                super::owned_verification_wait(&cas_dir, "cas-9efb-test", "lead-id").is_some(), quiet,
                "{note}"
            );
        }
    }
}
