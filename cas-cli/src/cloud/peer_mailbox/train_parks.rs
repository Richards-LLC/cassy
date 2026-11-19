//! Immutable worker claims admitted by the sole train owner before review.
use super::*;
use cas_types::{Agent, Task};
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
    Result { park: Park, body: String },
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

    fn current(&self, root: &Path, task: &Task) -> Result<bool, String> {
        let tasks = crate::store::open_task_store_local(root).map_err(|e| e.to_string())?;
        let train = tasks.get(&self.train).map_err(|e| e.to_string())?;
        Ok(parked(task)?.as_ref() == Some(self) && self.task == task.id && self.epoch > 0
            && crate::cloud::resolve_canonical_id(root).as_deref() == Some(self.project.as_str())
            && train.epic_verification_owner.as_deref() == Some(self.owner.as_str())
            && crate::factory_session_scope::train_owner_generation(&train)? == self.generation
            && tasks.get_parent_epic(&task.id).map_err(|e| e.to_string())?.is_some_and(|parent| parent.id == self.train)
            && task.assignee.as_deref().is_some_and(|id| id == self.name || id == self.worker)
            && task.deliverables.delivery_cycle == self.cycle
            && task.deliverables.parked_branch.as_deref() == Some(self.branch.as_str())
            && task.deliverables.factory_branch_anchor.as_deref() == Some(self.tip.as_str())
            && task.deliverables.work_target.as_ref().is_some_and(|work| work.target_branch == self.target)
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
    if !park.current(root, &task)? { return reject; }
    let directory = WorkerDirectory::load(root, &recipient.id)?;
    match mail {
        TrainMail::Ready { park } => {
            if recipient.role != AgentRole::Supervisor || !owns_recipient(root, recipient, &park.owner)
                || !directory.owner_of(&recipient.id, &park.worker) || !directory.sender_matches(message)
                || message.sender_agent_id != park.worker || message.sender_name != park.name
                || message.sender_machine_id.as_deref() != Some(park.machine.as_str())
                || message.sender_session.as_deref() != Some(park.session.as_str())
                || task.status != crate::types::TaskStatus::AwaitingMerge { return reject; }
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
        TrainMail::Result { park, .. } => {
            if recipient.id != park.worker || !directory.owner_message(message, &recipient.id) { return reject; }
        }
    }
    Ok(None)
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

pub(crate) fn publish_ready(_root: &Path, _task: &Task, _worker: &Agent) -> Result<Option<i64>, String> {
    Ok(None)
}
