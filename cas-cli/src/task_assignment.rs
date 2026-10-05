//! Durable assignment dispatch shared by task mutations and the director.

use cas_types::{AgentRole, Task, TaskStatus, TaskType};
use std::path::Path;

/// Queue the assignment once for the current live worker registration. This
/// does not depend on the task appearing in the director's focused snapshot.
pub(crate) fn enqueue(cas_dir: &Path, task: &Task) -> crate::Result<Option<i64>> {
    let prompts = crate::config::Config::load(cas_dir)?
        .orchestration()
        .auto_prompt;
    if !prompts.enabled || !prompts.on_task_assigned {
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
    let text = format!(
        "You have been assigned a new task:\nTask ID: {}\nTitle: {}\n\n\
         View full details: {prefix}task action=show id={}\n\
         Start working: {prefix}task action=start id={} brief=true\n\
         Successful task action=start is authoritative assignment acceptance; no prose ACK is required.\n\
         While working, post progress notes with {prefix}task action=notes.\n\
         If blocked, set status=blocked and send {prefix}coordination action=message target=supervisor blocker=true summary=\"...\" message=\"...\".",
        task.id, task.title, task.id, task.id,
    );
    let key = format!("task-assignment:{}:{}", task.id, worker.id);
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
