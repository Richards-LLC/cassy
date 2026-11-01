//! Durable assignment dispatch shared by task mutations and the director.

use cas_types::{AgentRole, Task, TaskStatus, TaskType};
use std::path::Path;

/// The one-line form of cas-tdd's "Fewest tests" principle. A macro so the
/// assignment policy below can `concat!` it.
macro_rules! test_principle {
    () => {
        "Elegant tests, not copious: write the fewest tests that fail without the change, one per behaviour, \
         through the real path; extend an existing test before adding one, and fix or delete a flaky test \
         instead of retrying it (cas-tdd, Fewest tests)."
    };
}

/// Carried by every assignment, spawn brief and task start (cas-5a52).
pub(crate) const TEST_PRINCIPLE: &str = test_principle!();

pub(crate) const WORKER_ASSIGNMENT_TEST_POLICY: &str = concat!(
    test_principle!(),
    " Assignment authorizes and requires those tests and running the change's affected targeted tests (",
    "`node --test`, `vitest`, `tsc`, or capped Cargo tests per CONTRIBUTING). ",
    "Factory workers run the targeted tests for their change themselves. ",
    "For Rust, use one package, a named-test `-E` filter, and the capped runner; ",
    "JS, TS, and Python tests run directly in the worker. ",
    "The merge queue runs the full suite once; never run it locally. ",
    "Only `--full` Playwright and the Rust full suite are reserved for the merge queue. ",
    "Docs-only and no-code tasks prove their acceptance check instead (e.g. ",
    "markdownlint, `cas codemap status`, a published artifact); don't add code tests to them. ",
    "Workers close their own tasks; never ask the supervisor to close your task on their behalf."
);

/// Queue the assignment once for the current live worker registration. This
/// does not depend on the task appearing in the director's focused snapshot.
pub(crate) fn enqueue(cas_dir: &Path, task: &Task) -> crate::Result<Option<i64>> {
    enqueue_with_queue_occurrence(cas_dir, task, None, false)
}

pub(crate) fn enqueue_queued(cas_dir: &Path, task: &Task, occurrence: u64, initial: bool) -> crate::Result<Option<i64>> {
    enqueue_with_queue_occurrence(cas_dir, task, Some(occurrence), initial)
}

fn enqueue_with_queue_occurrence(cas_dir: &Path, task: &Task, occurrence: Option<u64>, initial: bool) -> crate::Result<Option<i64>> {
    let prompts = crate::config::Config::load(cas_dir)?
        .orchestration()
        .auto_prompt;
    if occurrence.is_none() && (!prompts.enabled || !prompts.on_task_assigned) {
        return Ok(None);
    }
    if !matches!(task.status, TaskStatus::Open | TaskStatus::InProgress)
        || matches!(task.task_type, TaskType::Epic | TaskType::Gate)
    {
        return Ok(None);
    }
    let Some(assignee) = task.assignee.as_deref() else {
        return Ok(None);
    };
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let Some(worker) = agents
        .iter()
        .filter(|agent| {
            agent.name.eq_ignore_ascii_case(assignee) || agent.id.eq_ignore_ascii_case(assignee)
        })
        .max_by_key(|agent| agent.registered_at)
        .filter(|agent| {
            agent.role == AgentRole::Worker
                && agent.is_alive()
                && chrono::Utc::now()
                    .signed_duration_since(agent.last_heartbeat)
                    .num_seconds()
                    <= cas_types::DEFAULT_HEARTBEAT_TIMEOUT_SECS
        })
    else {
        return Ok(None);
    };
    let Some(session) = worker.factory_session.as_deref() else {
        return Ok(None);
    };
    // Respect hard start gates and external blockers through the canonical
    // ready policy. Ordinary blocks edges allow parallel preparation.
    if task.status == TaskStatus::Open
        && !crate::store::open_task_store(cas_dir)?
            .list_ready()?
            .iter()
            .any(|ready| ready.id == task.id)
    {
        return Ok(None);
    }
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(worker);
    let prefix = cli.backend().capabilities().tool_prefix;
    let mut text = format!(
        "You have been assigned a new task:\nTask ID: {}\nTitle: {}\n\n\
         View full details: {prefix}task action=show id={}\n\
         Start working: {prefix}task action=start id={} brief=true\n\
         Successful task action=start is authoritative assignment acceptance; no prose ACK is required.\n\
         {}\n\
         While working, post progress notes with {prefix}task action=notes.\n\
         If blocked, set status=blocked and send {prefix}coordination action=message target=supervisor blocker=true summary=\"...\" message=\"...\".",
        task.id,
        task.title,
        task.id,
        task.id,
        WORKER_ASSIGNMENT_TEST_POLICY,
    );
    if occurrence.is_some() && task.status == TaskStatus::InProgress {
        let branch = task.deliverables.factory_branch_start.as_ref()
            .map(|start| start.branch.as_str()).unwrap_or("see task details");
        text = format!("You have been assigned a new task:\nTask ID: {}\nTitle: {}\n\nCassy started this task automatically from your queue. Branch: {branch}. No assignment ACK is needed.\n\nView full details: {prefix}task action=show id={}\n\n{}\n\nWhile working, post progress notes with {prefix}task action=notes id={}.", task.id, task.title, task.id, WORKER_ASSIGNMENT_TEST_POLICY, task.id);
    }
    if occurrence.is_some() && task.status == TaskStatus::InProgress {
        text = crate::prompt_revalidation::attach_queue_continuation(&text, &task.id);
    }
    let key = match occurrence {
        Some(occurrence) if !initial => format!("task-queue-assignment:{}:{}:{occurrence}", task.id, worker.id),
        _ => assignment_key(session, &worker.name, &task.id),
    };
    let queue = crate::store::open_prompt_queue_store(cas_dir)?;
    let result = queue.enqueue_idempotent(
        "task-assignment",
        &worker.name,
        &text,
        Some(session),
        Some(&format!("Assigned task: {}", task.id)),
        Some(cas_store::NotificationPriority::High),
        &key,
        Some(&cas_store::QueueOrigin::Daemon),
    )?;
    if matches!(result, cas_store::EnqueueIdempotentResult::Created(_)) {
        if let Err(error) = cas_factory::notify_daemon(cas_dir) {
            tracing::debug!(%error, "assignment queued; daemon wake datagram unavailable");
        }
    }
    Ok(Some(match result {
        cas_store::EnqueueIdempotentResult::Created(id)
        | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
    }))
}

/// Startup, registration repair and ordinary assignment share one durable row.
/// A queue successor uses its own occurrence key, so a later continuation is
/// never mistaken for the original launch instruction.
pub(crate) fn assignment_key(session: &str, worker: &str, task: &str) -> String {
    format!("task-assignment:{session}:{worker}:{task}")
}

pub(crate) fn enqueue_spawn_message(
    cas_dir: &Path, session: &str, worker: &str, task: &str, message: &str,
) -> crate::Result<i64> {
    let result = crate::store::open_prompt_queue_store(cas_dir)?.enqueue_idempotent(
        "task-assignment", worker, message, Some(session),
        Some(&format!("Assigned task: {task}")), Some(cas_store::NotificationPriority::High),
        &assignment_key(session, worker, task), Some(&cas_store::QueueOrigin::Daemon),
    )?;
    Ok(match result {
        cas_store::EnqueueIdempotentResult::Created(id)
        | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
    })
}
