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
    // The ending owner may have no live daemon. Keep operator notices in the
    // durable project outbox until a connected daemon adopts its own session.
    if recipient.id == "operator" {
        crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(root);
        return Ok(());
    }
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

pub(crate) fn operator_prompt(
    message: &str,
    summary: &str,
) -> Result<String, crate::error::CasError> {
    Ok(serde_json::to_string(
        &crate::ui::factory::OperatorReplyPayload {
            schema_version: 2,
            reply_to: None,
            message: message.to_string(),
            summary: summary.to_string(),
            device_id: "*".into(),
            operator_label: None,
            kind: crate::ui::factory::OperatorTurnKind::Blocker,
            options: Vec::new(),
            attachments: Vec::new(),
            reply_to_session: None,
        },
    )?)
}

/// Called by a daemon with a connected Commander, without network or discovery.
/// Global idempotency admits one destination even if two daemons race. A crash
/// before the stamps simply retries the same prompt row.
pub(crate) fn adopt_operator_notices(
    root: &Path,
    session: &str,
) -> Result<(), crate::error::CasError> {
    let sq = open_supervisor_queue_store(root)?;
    let pq = open_prompt_queue_store(root)?;
    for notice in sq.list_pending_prompt_outbox("operator", "supervisor_notice", 10)? {
        let payload: serde_json::Value = serde_json::from_str(&notice.payload)?;
        let Some(kind) = payload.get("kind").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(detail) = payload.get("detail").and_then(|v| v.as_str()) else {
            continue;
        };
        pq.enqueue_idempotent(
            &format!("supervisor-notice:{}", notice.id),
            "operator",
            &operator_prompt(detail, kind)?,
            Some(session),
            Some(kind),
            Some(NotificationPriority::High),
            &format!("supervisor-notice-operator:{}", notice.id),
            Some(&QueueOrigin::Daemon),
        )?;
        sq.mark_prompt_delivered(notice.id)?;
        sq.ack(notice.id)?;
    }
    Ok(())
}

/// Explicit orphaning avoids assigning a peer new work without its acceptance.
/// Queue the notice before clearing ownership or deleting the ending identity.
pub(crate) fn orphan_epics(root: &Path, ending: &Agent) -> Result<(), crate::error::CasError> {
    if ending.role != AgentRole::Supervisor {
        return Ok(());
    }
    crate::supervisor_continuity::record_end(root, &ending.id, None)
        .map_err(|error| crate::error::CasError::Other(error.to_string()))?;
    let tasks = open_task_store(root)?;
    let agents = open_agent_store(root)?.list(None)?;
    let named_owner = agents
        .iter()
        .filter(|a| a.role == AgentRole::Supervisor && a.name == ending.name)
        .max_by_key(|a| a.registered_at);
    let mut owned: Vec<_> = tasks
        .list(None)?
        .into_iter()
        .filter(|task| task.task_type == TaskType::Epic && !task.is_terminal())
        .filter(|task| {
            task.epic_verification_owner
                .as_deref()
                .is_some_and(|owner| {
                    owner == ending.id
                        || (owner == ending.name && named_owner.is_some_and(|a| a.id == ending.id))
                })
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
    let occurrence =
        serde_json::json!([ending.id, ids, chrono::Utc::now().to_rfc3339()]).to_string();
    for peer in agents.iter().filter(|a| a.id != ending.id && is_live(a)) {
        enqueue(
            root,
            &Recipient::agent(peer),
            "epic_orphaned",
            &detail,
            &occurrence,
        )?;
    }
    enqueue(
        root,
        &Recipient::operator(ending.factory_session.as_deref().unwrap_or(&ending.id)),
        "epic_orphaned",
        &detail,
        &occurrence,
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
    let valid_kind = ["epic_orphaned", "integration_conflict", "capacity_wait", "epic_ready", "reminder_expiring", "reminder_cancelled"]
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

/// Called after a real focus change, from both the MCP and Commander facade.
pub(crate) fn integration_overlap(
    root: &Path,
    focused: &cas_types::Task,
    session: &str,
) -> Result<(), crate::error::CasError> {
    use crate::mcp::tools::core::task::repo_context::inherited_work_target_from_epic;
    let Some(lane) = inherited_work_target_from_epic(focused) else {
        return Ok(());
    };
    let agents = open_agent_store(root)?.list(None)?;
    let first = resolve(
        &agents,
        focused.epic_verification_owner.as_deref(),
        Some(session),
    );
    for other in open_task_store(root)?
        .list(None)?
        .into_iter()
        .filter(|t| t.task_type == TaskType::Epic && !t.is_terminal() && t.id != focused.id)
    {
        let Some(other_lane) = inherited_work_target_from_epic(&other) else {
            continue;
        };
        let same_repo = lane.repo_selector == other_lane.repo_selector;
        let stacked = focused
            .deliverables
            .work_target
            .as_ref()
            .is_some_and(|t| t.target_branch == other_lane.target_branch)
            || other
                .deliverables
                .work_target
                .as_ref()
                .is_some_and(|t| t.target_branch == lane.target_branch);
        if !same_repo || !(lane.target_branch == other_lane.target_branch || stacked) {
            continue;
        }
        let Some(owner) = other.epic_verification_owner.as_deref() else {
            continue;
        };
        // Skip one supervisor's own lanes, but do not drop an ended owner's
        // conflict merely because recovery resolves both readers to one peer.
        if focused.epic_verification_owner.as_deref() == Some(owner)
            || agents
                .iter()
                .any(|a| a.id == first.id && (a.id == owner || a.name == owner))
        {
            continue;
        }
        let second = resolve(&agents, Some(owner), None);
        let detail = format!(
            "Integration lanes overlap: {} ({}) and {} ({}) use {} / {} in {}. Coordinate merges before changing either lane.",
            focused.id,
            first.name,
            other.id,
            second.name,
            lane.target_branch,
            other_lane.target_branch,
            lane.repo_selector
        );
        // Ownership/lane changes create a new episode; merely focusing again does not.
        let mut lanes = [
            format!(
                "{}:{}:{}",
                focused.id,
                focused.epic_verification_owner.as_deref().unwrap_or(""),
                lane.target_branch
            ),
            format!("{}:{}:{}", other.id, owner, other_lane.target_branch),
        ];
        lanes.sort();
        let occurrence = serde_json::json!([lane.repo_selector, lanes]).to_string();
        enqueue(root, &first, "integration_conflict", &detail, &occurrence)?;
        enqueue(root, &second, "integration_conflict", &detail, &occurrence)?;
    }
    Ok(())
}

/// The claim event already supplied the remote holder and its liveness. No
/// additional discovery is done on a daemon tick or SessionEnd. HTTP is bounded
/// by MAILBOX_TIMEOUT and runs on a blocking task, outside the factory loop.
pub(crate) fn remote_focus_conflict(
    root: &Path,
    sender: &str,
    epic: &str,
    hold: &crate::cloud::peer_claims::PeerHold,
) -> tokio::task::JoinHandle<()> {
    let root = root.to_path_buf();
    let sender = sender.to_string();
    let epic = epic.to_string();
    let hold = hold.clone();
    tokio::spawn(async move {
        remote_focus_conflict_delivery(&root, &sender, &epic, &hold).await;
    })
}

async fn remote_focus_conflict_delivery(
    root: &Path,
    sender: &str,
    epic: &str,
    hold: &crate::cloud::peer_claims::PeerHold,
) {
    use crate::cloud::peer_mailbox::{PeerMailbox, PeerSend, http_mailbox};
    use sha2::{Digest, Sha256};
    let label = serde_json::json!([sender, epic, hold.agent_id]).to_string();
    let detail = format!(
        "Epic focus conflict: supervisor {sender} also focused {epic}, held by {}. Coordinate ownership and integration lanes before proceeding.",
        hold.describe()
    );
    let root_owned = root.to_path_buf();
    let sender_id = sender.to_string();
    let hold = hold.clone();
    let body = detail.clone();
    let event = label.clone();
    // The HTTP client's timeout bounds this background work. Await the actual
    // send result: an outer timeout cannot cancel spawn_blocking and would
    // permit a late peer success after an operator fallback.
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        if hold.live != Some(true) || hold.is_expired(chrono::Utc::now()) {
            return Err("remote focus holder is not confirmed live".into());
        }
        let project = crate::cloud::resolve_canonical_id(&root_owned)
            .ok_or("project identity unavailable")?;
        let mailbox = http_mailbox(&root_owned).ok_or("Cassy Cloud mailbox unavailable")?;
        let key = format!(
            "focus-conflict:{:x}",
            Sha256::digest(serde_json::json!([project, event]).to_string())
        );
        mailbox
            .send(&PeerSend {
                project_id: project,
                sender_agent_id: sender_id,
                recipient_agent_id: hold.agent_id,
                body: crate::cloud::peer_mailbox::attach_message_intent(&body, Some("ask"), false),
                summary: Some("epic focus conflict".into()),
                in_reply_to: None,
                dedupe_key: key,
                scope: Default::default(),
                recipient_project_id: None,
            })
            .map(|_| ())
    })
    .await;
    let failure = match result {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(error) => error.to_string(),
    };
    let agents = open_agent_store(root)
        .ok()
        .and_then(|s| s.list(None).ok())
        .unwrap_or_default();
    let session = agents
        .iter()
        .find(|a| a.id == sender)
        .and_then(|a| a.factory_session.as_deref())
        .unwrap_or("supervisor-recovery");
    if let Err(error) = enqueue(
        root,
        &Recipient::operator(session),
        "integration_conflict",
        &format!("{detail}\nRemote notice failed: {failure}. Operator intervention is required."),
        &label,
    ) {
        tracing::error!(%error, "focus-conflict operator fallback failed");
    }
}

#[cfg(test)]
#[path = "supervisor_notices_tests.rs"]
mod tests;
