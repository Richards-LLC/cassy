//! Owner-addressed factory notices. Worker commands keep their session-scoped
//! resolver; recovery notices may reach another live supervisor or the operator.
use std::path::Path;

use cas_store::{NotificationPriority, NotifyIdempotentResult, QueueOrigin};
use cas_types::{Agent, AgentRole, TaskType};

use crate::store::{
    open_agent_store, open_prompt_queue_store, open_supervisor_queue_store, open_task_store,
};

pub(crate) fn is_live(agent: &Agent) -> bool {
    agent.role == AgentRole::Supervisor
        && agent.is_alive()
        && (chrono::Utc::now() - agent.last_heartbeat).num_seconds()
            <= crate::factory_supervisor_overlap::SUPERVISOR_LIVE_SECS
}

/// A destination carries its own session, never the ended owner's session.
#[derive(Debug, Clone)]
pub(crate) struct Recipient {
    pub id: String,
    pub name: String,
    pub session: Option<String>,
}

impl Recipient {
    pub(crate) fn operator(session: &str) -> Self {
        Self {
            id: "operator".into(),
            name: "operator".into(),
            session: Some(session.to_string()),
        }
    }
    pub(crate) fn agent(agent: &Agent) -> Self {
        Self {
            id: agent.id.clone(),
            name: agent.name.clone(),
            session: agent.factory_session.clone(),
        }
    }
}

/// Same-name successor, then project-local live peer, then operator. `owner`
/// may be an id or name. All rows came from this project's own agent store.
pub(crate) fn resolve(agents: &[Agent], owner: Option<&str>, session: Option<&str>) -> Recipient {
    let former = agents
        .iter()
        .filter(|a| a.role == AgentRole::Supervisor)
        .filter(|a| {
            owner.is_some_and(|owner| a.id == owner || a.name == owner)
                || (owner.is_none() && a.factory_session.as_deref() == session)
        })
        .max_by_key(|a| a.registered_at);
    let name = former.map(|a| a.name.as_str()).or(owner);
    let mut live: Vec<_> = agents.iter().filter(|a| is_live(a)).collect();
    live.sort_by(|a, b| {
        let rank = |a: &Agent| {
            if owner == Some(a.id.as_str()) {
                0
            } else if name == Some(a.name.as_str()) {
                1
            } else if session.is_some() && a.factory_session.as_deref() == session {
                2
            } else {
                3
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.registered_at.cmp(&a.registered_at))
            .then(a.id.cmp(&b.id))
    });
    live.first()
        .map(|a| Recipient::agent(a))
        .unwrap_or_else(|| Recipient::operator(session.or(owner).unwrap_or("supervisor-recovery")))
}

/// Durably enqueue one CAS-authored action. A retry reuses both queue rows.
pub(crate) fn enqueue(
    root: &Path,
    recipient: &Recipient,
    kind: &str,
    detail: &str,
    occurrence: &str,
) -> Result<(), crate::error::CasError> {
    let sq = open_supervisor_queue_store(root)?;
    let pq = open_prompt_queue_store(root)?;
    let key = format!("supervisor-notice:{kind}:{occurrence}:{}", recipient.id);
    let payload = serde_json::json!({ "kind": kind, "detail": detail, "recipient": recipient.name, "factory_session": recipient.session }).to_string();
    let id = match sq.notify_idempotent(
        &recipient.id,
        "supervisor_notice",
        &payload,
        NotificationPriority::High,
        &key,
    )? {
        NotifyIdempotentResult::Created(id) | NotifyIdempotentResult::AlreadyExists { id, .. } => {
            id
        }
    };
    let body = format!(
        "<cas-supervisor-notice kind=\"{kind}\" notification_id=\"{id}\">\n{detail}\n</cas-supervisor-notice>"
    );
    pq.enqueue_idempotent(
        &format!("supervisor-notice:{id}"),
        &recipient.name,
        &body,
        recipient.session.as_deref(),
        Some(kind),
        Some(NotificationPriority::High),
        &key,
        Some(&QueueOrigin::Daemon),
    )?;
    sq.mark_prompt_delivered(id)?;
    crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(root);
    Ok(())
}

/// Explicit orphaning avoids assigning a peer new work without its acceptance.
/// Queue the notice before clearing ownership or deleting the ending identity.
pub(crate) fn orphan_epics(root: &Path, ending: &Agent) -> Result<(), crate::error::CasError> {
    if ending.role != AgentRole::Supervisor {
        return Ok(());
    }
    let tasks = open_task_store(root)?;
    let mut owned: Vec<_> = tasks
        .list(None)?
        .into_iter()
        .filter(|task| task.task_type == TaskType::Epic && !task.is_terminal())
        .filter(|task| {
            task.epic_verification_owner
                .as_deref()
                .is_some_and(|owner| owner == ending.id || owner == ending.name)
        })
        .collect();
    if owned.is_empty() {
        return Ok(());
    }
    owned.sort_by(|a, b| a.id.cmp(&b.id));
    let ids = owned
        .iter()
        .map(|t| t.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let detail = format!(
        "ORPHANED epics {ids}: supervisor {} ({}) ended. Ownership is cleared; a live supervisor must explicitly adopt these epics.",
        ending.name, ending.id
    );
    let agents = open_agent_store(root)?.list(None)?;
    for peer in agents.iter().filter(|a| a.id != ending.id && is_live(a)) {
        enqueue(
            root,
            &Recipient::agent(peer),
            "epic_orphaned",
            &detail,
            &ending.id,
        )?;
    }
    enqueue(
        root,
        &Recipient::operator(ending.factory_session.as_deref().unwrap_or(&ending.id)),
        "epic_orphaned",
        &detail,
        &ending.id,
    )?;
    for task in &mut owned {
        task.epic_verification_owner = None;
        task.notes = format!(
            "{}\n\n[{}] ⚠️ {detail}",
            task.notes,
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        );
        task.updated_at = chrono::Utc::now();
        tasks.update(task)?;
    }
    Ok(())
}

/// Only CAS-authored rows use this envelope; the daemon checks origin too.
pub(crate) fn is_action_envelope(body: &str) -> bool {
    let Some((tag, _)) = body.split_once('\n') else {
        return false;
    };
    let valid_kind = ["epic_orphaned", "integration_conflict"]
        .iter()
        .any(|kind| {
            tag.starts_with(&format!(
                "<cas-supervisor-notice kind=\"{kind}\" notification_id=\""
            ))
        });
    valid_kind
        && tag
            .rsplit_once("notification_id=\"")
            .and_then(|(_, id)| id.strip_suffix("\">"))
            .is_some_and(|id| id.parse::<i64>().is_ok())
        && body.ends_with("</cas-supervisor-notice>")
}
