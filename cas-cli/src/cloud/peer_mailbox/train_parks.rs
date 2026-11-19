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

pub(crate) fn ingest(_root: &Path, _recipient: &Agent, _message: &ClaimedPeerMessage) -> Result<Option<PeerAckOutcome>, String> {
    Ok(None)
}

pub(crate) fn admitted(_root: &Path, _task: &Task, _branch: &str, _tip: &str) -> bool {
    false
}
