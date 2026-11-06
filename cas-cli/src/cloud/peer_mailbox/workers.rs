//! The narrow own-account, same-repository, active-same-epic worker channel.
//! Both send and receive use fresh registry/claim observations and synced tasks.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use chrono::Utc;
use crate::cloud::{CloudConfig, CloudCoordinator, peers, peers_detail};
use crate::types::{Agent, AgentStatus, LeaseStatus, TaskType};
use super::*;

pub(crate) struct WorkerDirectory {
    workers: Vec<peers_detail::PeerWorkerRef>,
    epics: BTreeMap<String, BTreeMap<String, chrono::DateTime<Utc>>>,
}

impl WorkerDirectory {
    pub(crate) fn load(root: &Path, caller: &str) -> Result<Self, String> {
        let repo = crate::cloud::resolve_canonical_id(root).ok_or("project has no canonical id")?;
        let aliases = crate::cloud::project_aliases_from_config_toml(root);
        let coordinator = CloudCoordinator::new(CloudConfig::load_from_cas_dir(root).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?.with_timeout(peers::DISCOVERY_TIMEOUT)
            .with_canonical_id(Some(repo.clone()));
        let agents = coordinator.list_agent_infos().map_err(|e| e.to_string())?;
        // No account inference for worker authority: the registered local caller
        // must itself occur in the authenticated project directory.
        if !agents.iter().any(|a| a.id == caller) { return Err("worker peer caller is absent from the Cloud registry".into()); }
        let owner = peers::caller_user_id(&agents, Some(caller))?;
        let now = Utc::now();
        let mut supervisors = peers::repo_peers(&agents, &repo, &aliases, None, now);
        peers::mark_own(&mut supervisors, Some(&owner));
        supervisors.retain(|s| s.live && s.own == Some(true));
        let workers = peers_detail::live_peer_workers(&supervisors, &agents, &aliases, now)
            .into_iter().filter(|worker| agents.iter().any(|a| a.id == worker.agent_id
                && a.user_id.as_deref() == Some(owner.as_str()))).collect::<Vec<_>>();
        let locks = coordinator.list_active_locks().map_err(|e| e.to_string())?;
        let tasks = crate::store::open_task_store_local(root).map_err(|e| e.to_string())?;
        let mut epics: BTreeMap<String, BTreeMap<String, chrono::DateTime<Utc>>> = BTreeMap::new();
        for worker in &workers {
            for lock in locks.iter().filter(|l| l.agent_id == worker.agent_id
                && l.status == LeaseStatus::Active && l.expires_at > now && l.renewed_at <= now) {
                let Some(task_id) = peers_detail::task_id_for_peer(&lock.task_id, &worker.supervisor, &aliases) else { continue };
                let Ok(mut task) = tasks.get(task_id) else { continue };
                if task.is_terminal() { continue; }
                let mut seen = BTreeSet::new();
                while seen.insert(task.id.clone()) {
                    if task.task_type == TaskType::Epic && !task.is_terminal() {
                        epics.entry(worker.agent_id.clone()).or_default().entry(task.id.clone())
                            .and_modify(|expires| *expires = (*expires).max(lock.expires_at))
                            .or_insert(lock.expires_at);
                    }
                    match tasks.get_parent_epic(&task.id).map_err(|e| e.to_string())? {
                        Some(parent) if !parent.is_terminal() => task = parent,
                        _ => break,
                    }
                }
            }
        }
        Ok(Self { workers, epics })
    }

    pub(crate) fn matches_local(&self, local: &Agent) -> bool {
        matches!(local.status, AgentStatus::Active | AgentStatus::Idle)
            && self.workers.iter().any(|worker| worker.agent_id == local.id
                && worker.name == local.name
                && worker.supervisor.session == local.factory_session
                && worker.supervisor.machine_id == local.machine_id)
    }

    pub(crate) fn resolve(&self, target: &str) -> Result<Option<&peers_detail::PeerWorkerRef>, String> {
        let found = self.workers.iter().filter(|worker| worker.matches(target)).collect::<Vec<_>>();
        match found.as_slice() {
            [] => Ok(None),
            [worker] => Ok(Some(*worker)),
            _ => Err(format!("worker peer '{target}' is ambiguous; use name@machine")),
        }
    }

    pub(crate) fn pair(&self, sender: &str, recipient: &str) -> Result<(&peers_detail::PeerWorkerRef, &peers_detail::PeerWorkerRef), String> {
        let from = self.workers.iter().find(|w| w.agent_id == sender);
        let to = self.workers.iter().find(|w| w.agent_id == recipient);
        let (Some(from), Some(to)) = (from, to) else { return Err("both worker peers must be live registered workers of your account and repository".into()); };
        if from.agent_id == to.agent_id || from.supervisor.machine_id == to.supervisor.machine_id {
            return Err("remote worker peers must be on different machines".into());
        }
        let now = Utc::now();
        if !self.epics.get(sender).is_some_and(|mine| self.epics.get(recipient)
            .is_some_and(|theirs| mine.iter().any(|(epic, expires)| *expires > now
                && theirs.get(epic).is_some_and(|expires| *expires > now)))) {
            return Err("worker peers require active task claims under the same epic".into());
        }
        Ok((from, to))
    }

    /// Record local visibility and send the other supervisor a durable quiet
    /// copy before acking the worker message. Stable keys collapse redelivery.
    pub(crate) fn copy_to_supervisors(&self, root: &Path, queue: &dyn PromptQueueStore, message: &ClaimedPeerMessage) -> Result<(), String> {
        let (from, to) = self.pair(&message.sender_agent_id, &message.recipient_agent_id)?;
        let (_, original) = split_message_intent(&message.body);
        let copy = format!("Peer worker message copy — from {}@{} to {}@{}.\n\n{}",
            from.name, from.machine, to.name, to.machine,
            crate::prompt_revalidation::neutralize_worker_authored_envelope_tags(original));
        let body = attach_message_intent(&copy, Some("receipt"), false)?;
        let mut local = message.clone();
        local.recipient_agent_id = to.supervisor.agent_id.clone();
        local.body = body.clone();
        queue.enqueue_idempotent(&format!("peer:{}@{}", from.name, from.machine), &to.supervisor.name,
            &render_prompt(&local), to.supervisor.session.as_deref(), message.summary.as_deref(),
            Some(NotificationPriority::Normal), &format!("peer-worker-copy:{}", message.id), Some(&QueueOrigin::Daemon))
            .map_err(|e| e.to_string())?;
        outbox::enqueue_and_send(root, PeerSend {
            project_id: message.project_id.clone(), sender_agent_id: to.agent_id.clone(),
            recipient_agent_id: from.supervisor.agent_id.clone(), body,
            summary: message.summary.clone(), in_reply_to: None,
            dedupe_key: format!("peer-worker-copy:{}", message.id),
            scope: PeerScope::Project, recipient_project_id: None,
        }, &from.supervisor.name).map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub(super) fn allowed_intent(body: &str) -> bool {
    let (kind, _) = split_message_intent(body);
    matches!(kind, None | Some(PeerMessageKind::Ask | PeerMessageKind::Answer | PeerMessageKind::Receipt | PeerMessageKind::Quiet))
}

pub(super) fn render_worker_prompt(message: &ClaimedPeerMessage) -> String {
    let (kind, original) = split_message_intent(&message.body);
    let (attachments, body) = split_attachments(original);
    let safe = format!("{}{}", render_attachments(&attachments, &message.project_id),
        crate::prompt_revalidation::neutralize_worker_authored_envelope_tags(body));
    let mut message = message.clone();
    message.body = attach_message_intent(&safe, kind.map(PeerMessageKind::as_str), false)
        .expect("parsed peer kind is valid");
    render_prompt(&message).replacen("Peer supervisor ", "Peer worker (advisory only; your supervisor retains assignment authority) ", 1)
}
