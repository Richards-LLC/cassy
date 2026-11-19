//! Immutable worker claims admitted by the sole train owner before review.
use super::*;
use cas_types::{Agent, Task, TaskStatus};
use sha2::{Digest, Sha256};

pub(crate) const NOTE: &str = "TRAIN_PARK: ";
pub(crate) const TAG: &str = "CAS_TRAIN_V1 ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Park {
    pub project: String,
    pub train: String,
    pub owner: String,
    pub generation: String,
    pub worker: String,
    pub name: String,
    pub machine: String,
    pub session: String,
    pub task: String,
    pub epoch: u64,
    pub cycle: u32,
    pub branch: String,
    pub tip: String,
    pub target: String,
    pub evidence: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TrainMail {
    Ready { park: Park },
    Result { park: Park, cycle: u32, status: TaskStatus, occurrence: String, body: String },
}

pub(crate) fn evidence_key(task: &Task) -> Result<String, String> {
    let manifest = task.notes.lines().rev().find_map(|line| line.strip_prefix("EVIDENCE_SYNC: "))
        .ok_or("Remote park requires a complete evidence manifest")?;
    serde_json::from_str::<serde_json::Value>(manifest).map_err(|e| e.to_string())?;
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&(manifest, &task.deliverables.test_receipts)).map_err(|e| e.to_string())?)))
}

pub(crate) fn queue_key(message: &ClaimedPeerMessage) -> Option<String> {
    let (_, body) = split_message_intent(&message.body);
    let mail: TrainMail = serde_json::from_str(body.strip_prefix(TAG)?).ok()?;
    Some(format!("train:{}", hex::encode(Sha256::digest(serde_json::to_vec(&mail).ok()?))))
}

pub(crate) fn parked(task: &Task) -> Result<Option<Park>, String> {
    task.notes.lines().rev().find_map(|line| line.strip_prefix(NOTE))
        .map(|line| serde_json::from_str(line).map_err(|e| e.to_string())).transpose()
}

impl Park {
    fn path(&self, root: &Path) -> Result<std::path::PathBuf, String> {
        let key = hex::encode(Sha256::digest(serde_json::to_vec(self).map_err(|e| e.to_string())?));
        Ok(root.join("train-parks").join(format!("{key}.json")))
    }

    fn scope(&self, root: &Path, task: &Task) -> Result<bool, String> {
        let tasks = crate::store::open_task_store_local(root).map_err(|e| e.to_string())?;
        let train = tasks.get(&self.train).map_err(|e| e.to_string())?;
        Ok(parked(task)?.as_ref() == Some(self) && self.task == task.id && self.epoch > 0
            && crate::cloud::resolve_canonical_id(root).as_deref() == Some(self.project.as_str())
            && train.epic_verification_owner.as_deref() == Some(self.owner.as_str())
            && crate::factory_session_scope::train_owner_generation(&train)? == self.generation
            && tasks.get_parent_epic(&task.id).map_err(|e| e.to_string())?.is_some_and(|parent| parent.id == self.train)
            && task.assignee.as_deref().is_some_and(|id| id == self.name || id == self.worker)
            && task.deliverables.work_target.as_ref().is_some_and(|work| work.target_branch == self.target))
    }

    fn current(&self, root: &Path, task: &Task) -> Result<bool, String> {
        Ok(self.scope(root, task)? && task.deliverables.delivery_cycle == self.cycle
            && task.deliverables.parked_branch.as_deref() == Some(self.branch.as_str())
            && task.deliverables.factory_branch_anchor.as_deref() == Some(self.tip.as_str())
            && evidence_key(task)? == self.evidence)
    }
}

pub(crate) fn ingest(root: &Path, recipient: &Agent, message: &ClaimedPeerMessage) -> Result<Option<PeerAckOutcome>, String> {
    let (_, body) = split_message_intent(&message.body);
    let Some(body) = body.strip_prefix(TAG) else { return Ok(None) };
    let reject = Ok(Some(PeerAckOutcome::Rejected));
    let Ok(mail) = serde_json::from_str::<TrainMail>(body) else { return reject };
    let park = match &mail { TrainMail::Ready { park } | TrainMail::Result { park, .. } => park };
    if message.scope != PeerScope::Project || message.project_id != park.project { return reject; }
    let task = crate::store::open_task_store_local(root).map_err(|e| e.to_string())?
        .get(&park.task).map_err(|e| e.to_string())?;
    if parked(&task)?.is_none() { return Err("Remote park has not synced yet; admission will retry".into()); }
    if !park.scope(root, &task)? { return reject; }
    let directory = WorkerDirectory::load(root, &recipient.id)?;
    match mail {
        TrainMail::Ready { park } => {
            if recipient.role != AgentRole::Supervisor || !owns_recipient(root, recipient, &park.owner)
                || !directory.owner_of(&recipient.id, &park.worker) || !directory.sender_matches(message)
                || message.sender_agent_id != park.worker || message.sender_name != park.name
                || message.sender_machine_id.as_deref() != Some(park.machine.as_str())
                || message.sender_session.as_deref() != Some(park.session.as_str())
                || task.status != TaskStatus::AwaitingMerge || !park.current(root, &task)? { return reject; }
            if directory.claimed(&park.project, &park.task) {
                return Err("TRAIN_PARK_CLAIM_ACTIVE: worker claim is still active; admission will retry after release".into());
            }
            crate::artifacts::proof::materialize_peer_evidence(root, &task)?;
            let path = park.path(root)?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let temporary = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
            std::fs::write(&temporary, serde_json::to_vec(&park).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
        }
        TrainMail::Result { park, cycle, status, .. } => {
            if recipient.id != park.worker || !directory.owner_message(message, &recipient.id)
                || task.deliverables.delivery_cycle > cycle || cycle < park.cycle
                || (task.is_terminal() && task.status != status) { return reject; }
            if task.deliverables.delivery_cycle != cycle || task.status != status {
                return Err("TRAIN_RESULT_STATE_PENDING: owner result awaits replicated task state".into());
            }
            if task.deliverables.factory_branch_anchor.as_deref() != Some(park.tip.as_str())
                && !task.deliverables.historical_factory_branch_anchors.contains(&park.tip) { return reject; }
        }
    }
    Ok(None)
}

pub(crate) fn prompt(message: &ClaimedPeerMessage) -> Option<String> {
    let (kind, body) = split_message_intent(&message.body);
    let mail: TrainMail = serde_json::from_str(body.strip_prefix(TAG)?).ok()?;
    let (body, ready) = match mail {
        TrainMail::Ready { park } => (format!("Worker {}@{} parked {} at {} for {}.", park.name, park.machine, park.task, park.tip, park.target), Some(park)),
        TrainMail::Result { body, .. } => (body, None),
    };
    let mut rendered = message.clone();
    rendered.body = attach_message_intent(&body, kind.map(PeerMessageKind::as_str), false).ok()?;
    let prompt = render_prompt(&rendered);
    Some(match ready {
        Some(park) => crate::prompt_revalidation::attach_worker_action_envelope(&prompt,
            crate::prompt_revalidation::WorkerActionKind::Ready, &park.name, Some(&park.task)),
        None => prompt,
    })
}

pub(crate) fn ready_wake(source: &str, prompt: &str) -> bool {
    source.starts_with("lifecycle-wake:train:")
        && crate::prompt_revalidation::parse_worker_action_envelope(prompt).is_some()
}

pub(crate) fn admitted(root: &Path, task: &Task, branch: &str, tip: &str) -> bool {
    let Ok(Some(park)) = parked(task) else { return false };
    let fresh = crate::store::open_task_store_local(root).ok().and_then(|tasks| tasks.get(&task.id).ok());
    task.status == crate::types::TaskStatus::AwaitingMerge
        && fresh.is_some_and(|task| task.status == crate::types::TaskStatus::AwaitingMerge && park.current(root, &task).unwrap_or(false))
        && park.branch == branch && park.tip == tip && park.current(root, task).unwrap_or(false)
        && park.path(root).ok().and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Park>(&bytes).ok()).as_ref() == Some(&park)
}

pub(crate) fn host_worker(root: &Path, task: &Task) -> Option<Agent> {
    let agents = crate::store::open_agent_store(root).ok()?;
    let assignee = task.assignee.as_deref()?;
    agents.get(assignee).ok().or_else(|| agents.list(None).ok()?.into_iter()
        .filter(|agent| agent.name.eq_ignore_ascii_case(assignee)).max_by_key(|agent| agent.registered_at))
        .filter(|agent| agent.role == AgentRole::Worker && agent.metadata.contains_key("train_owner"))
}

pub(crate) fn owner_train(root: &Path, task: &Task) -> Result<Option<Task>, String> {
    Ok(crate::store::open_task_store_local(root).map_err(|e| e.to_string())?
        .get_parent_epic(&task.id).map_err(|e| e.to_string())?.filter(|train| train.epic_verification_owner.is_some()))
}

pub(crate) fn authorize(root: &Path, task: &Task, actor: &Agent) -> Result<(), String> {
    // Release parent bookkeeping already binds independent acceptance and
    // shipped children in a task-local context; it is not worker publication.
    if task.task_type == cas_types::TaskType::Epic
        && crate::qa_batch::release_close::parent_authority(root,&task.id).is_some() { return Ok(()); }
    if let Some(train) = owner_train(root, task)? { crate::factory_session_scope::require_train_owner(root, &train, actor)?; }
    Ok(())
}

pub(crate) fn require_admitted(root: &Path, task: &Task) -> Result<(), String> {
    if let Some(park) = parked(task)? {
        if !admitted(root, task, &park.branch, &park.tip) {
            return Err(crate::gates::tag("G-113", "Remote delivery is not a current admitted owner park"));
        }
        crate::artifacts::proof::materialize_peer_evidence(root, task)?;
    }
    Ok(())
}

pub(crate) fn publish_ready(root: &Path, task: &Task, worker: &Agent) -> Result<Option<i64>, String> {
    if worker.role != AgentRole::Worker || !worker.metadata.contains_key("train_owner") { return Ok(None); }
    let host = crate::factory_session_scope::WorkerHost::current(root, worker.factory_session.as_deref())?
        .ok_or("Worker host has no current train binding")?;
    if worker.metadata.get("train_owner") != Some(&host.owner) || worker.metadata.get("train_owner_generation") != Some(&host.generation) {
        return Err("Worker registration belongs to a different owner generation".into());
    }
    let tasks = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let task = tasks.get(&task.id).map_err(|e| e.to_string())?;
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    if task.status != crate::types::TaskStatus::AwaitingMerge || agents.get_lease(&task.id).map_err(|e| e.to_string())?.is_some() {
        return Err("Ready publication requires a parked delivery and a released local claim".into());
    }
    let history = agents.get_lease_history(&task.id,Some(1)).map_err(|e| e.to_string())?;
    let epoch = history.first().filter(|entry| entry.agent_id == worker.id).ok_or("Park lease identity is unavailable")?.epoch;
    let park = Park { project:crate::cloud::resolve_canonical_id(root).ok_or("Project has no canonical identity")?,
        train:host.train, owner:host.owner, generation:host.generation, worker:worker.id.clone(), name:worker.name.clone(),
        machine:worker.machine_id.clone().ok_or("Worker machine identity is missing")?, session:host.session,
        task:task.id.clone(), epoch, cycle:task.deliverables.delivery_cycle,
        branch:task.deliverables.parked_branch.clone().ok_or("Park branch is missing")?,
        tip:task.deliverables.factory_branch_anchor.clone().ok_or("Park tip is missing")?,
        target:task.deliverables.work_target.as_ref().ok_or("Park target is missing")?.target_branch.clone(), evidence:evidence_key(&task)? };
    let note = format!("{NOTE}{}",serde_json::to_string(&park).map_err(|e| e.to_string())?);
    let mut sealed = task.clone(); sealed.notes.push_str(&format!("\n{note}"));
    if !park.current(root,&sealed)? { return Err("Park binding no longer matches the worker delivery".into()); }
    if parked(&task)?.as_ref() != Some(&park) { tasks.append_trusted_note(&task.id,&note).map_err(|e| e.to_string())?; }
    let encoded = serde_json::to_string(&TrainMail::Ready { park:park.clone() }).map_err(|e| e.to_string())?;
    let message = PeerSend { project_id:park.project, sender_agent_id:worker.id.clone(), recipient_agent_id:park.owner,
        body:attach_message_intent(&format!("{TAG}{encoded}"),Some("ready"),false)?, summary:Some(format!("Parked delivery: {}",task.id)),
        in_reply_to:None, dedupe_key:format!("train:{}",hex::encode(Sha256::digest(encoded.as_bytes()))), scope:PeerScope::Project, recipient_project_id:None };
    outbox::enqueue_notice(root,message,&host.name).map(Some)
}
