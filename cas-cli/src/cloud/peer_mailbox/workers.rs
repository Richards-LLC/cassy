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
    owners: BTreeMap<String, BTreeSet<String>>,
    agents: Vec<crate::cloud::AgentInfo>,
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
        supervisors.retain(|s| s.own == Some(true));
        let workers = peers_detail::live_peer_workers(&supervisors, &agents, &aliases, now)
            .into_iter().filter(|worker| agents.iter().any(|a| a.id == worker.agent_id
                && a.user_id.as_deref() == Some(owner.as_str()))).collect::<Vec<_>>();
        let locks = coordinator.list_active_locks().map_err(|e| e.to_string())?;
        let tasks = crate::store::open_task_store_local(root).map_err(|e| e.to_string())?;
        let mut epics: BTreeMap<String, BTreeMap<String, chrono::DateTime<Utc>>> = BTreeMap::new();
        for worker in &workers {
            for lock in locks.iter().filter(|l| l.agent_id == worker.agent_id
                && l.status == LeaseStatus::Active && l.expires_at > now) {
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
        let mut owners = BTreeMap::new();
        for worker in &workers {
            let agent = agents.iter().find(|a| a.id == worker.agent_id).unwrap();
            let Some(train) = agent.metadata.get("train").and_then(|id| tasks.get(id).ok()) else { continue };
            let Some(owner) = train.epic_verification_owner.as_deref() else { continue };
            if train.is_terminal() || train.task_type != TaskType::Epic
                || owner != worker.supervisor.agent_id
                || agent.metadata.get("train_owner").map(String::as_str) != Some(owner)
                || agent.metadata.get("train_owner_generation") != crate::factory_session_scope::train_owner_generation(&train).ok().as_ref()
            { continue; }
            let mut ids = BTreeSet::from([owner.to_string()]);
            if let peers::SlotHolder::Successor { peer, .. } = peers::slot_holder(&supervisors, &worker.supervisor, !worker.supervisor.live) {
                ids.insert(peer.agent_id.clone());
            }
            owners.insert(worker.agent_id.clone(), ids);
        }
        Ok(Self { workers, owners, agents, epics })
    }

    pub(crate) fn matches_local(&self, local: &Agent) -> bool {
        matches!(local.status, AgentStatus::Active | AgentStatus::Idle)
            && self.workers.iter().any(|worker| worker.agent_id == local.id
                && worker.name == local.name
                && worker.supervisor.session == local.factory_session
                && worker.machine_id == local.machine_id)
    }

    /// Lifecycle authority survives a released claim, but never an owner-generation change.
    pub(crate) fn owner_of(&self, owner: &str, worker: &str) -> bool {
        self.owners.get(worker).is_some_and(|ids| ids.contains(owner))
    }

    pub(crate) fn owner_message(&self, message: &ClaimedPeerMessage, worker: &str) -> bool {
        self.owner_of(&message.sender_agent_id, worker)
            && self.agents.iter().any(|agent| agent.id == message.sender_agent_id
                && agent.machine_id == message.sender_machine_id
                && agent.metadata.get(peers::META_FACTORY_SESSION) == message.sender_session.as_ref())
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
        self.pair_for_receipt(sender, recipient)?
            .ok_or_else(|| "worker peers require active task claims under the same epic".into())
    }

    /// A missing claim observation is retryable on receipt, never permission.
    pub(crate) fn pair_for_receipt(&self, sender: &str, recipient: &str) -> Result<Option<(&peers_detail::PeerWorkerRef, &peers_detail::PeerWorkerRef)>, String> {
        let from = self.workers.iter().find(|w| w.agent_id == sender);
        let to = self.workers.iter().find(|w| w.agent_id == recipient);
        let (Some(from), Some(to)) = (from, to) else { return Err("both worker peers must be live registered workers of your account and repository".into()); };
        if from.agent_id == to.agent_id || from.machine_id == to.machine_id {
            return Err("remote worker peers must be on different machines".into());
        }
        let now = Utc::now();
        if !self.epics.get(sender).is_some_and(|mine| self.epics.get(recipient)
            .is_some_and(|theirs| mine.iter().any(|(epic, expires)| *expires > now
                && theirs.get(epic).is_some_and(|expires| *expires > now)))) {
            return Ok(None);
        }
        Ok(Some((from, to)))
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
