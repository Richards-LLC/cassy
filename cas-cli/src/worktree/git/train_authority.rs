//! Train publication history. Git is the shared authority; task rows are projections.
use std::io::{Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{GitOperations, TargetPushOutcome};

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
    pub owner: String,
    pub incarnation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase { Active, Draining, Quiesced, RecoveryHeld }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Transfer {
    pub id: String,
    pub successor: Identity,
    pub prior_generation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Record {
    pub protocol: u32,
    pub project: String,
    pub train: String,
    pub target: String,
    pub identity: Identity,
    pub epoch: u64,
    pub generation: String,
    pub phase: Phase,
    pub target_sha: String,
    pub operation: String,
    pub transfer: Option<Transfer>,
    #[serde(default)]
    pub quiescence: Option<serde_json::Value>,
    #[serde(default)]
    pub accepted: Option<Acceptance>,
    #[serde(default)]
    pub operator_claim: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Acceptance {
    pub transfer_id: String,
    pub prior_generation: String,
    pub generation: String,
    pub successor: Identity,
}

#[path = "train_transfer.rs"]
mod transfer;
pub(crate) use transfer::TransferChecks;
#[path = "train_recovery.rs"]
mod recovery;
pub(crate) use recovery::OperatorClaim;

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub reference: String,
    pub tip: String,
    pub record: Record,
}

pub(super) struct AtomicUpdate {
    pub reference: String,
    pub expected: String,
    pub next: String,
}

fn command(repo: &Path, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo).env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Cassy").env("GIT_AUTHOR_EMAIL", "cassy@localhost")
        .env("GIT_COMMITTER_NAME", "Cassy").env("GIT_COMMITTER_EMAIL", "cassy@localhost");
    let stdin = if let Some(input) = input {
        let mut file = tempfile::tempfile().map_err(|e| e.to_string())?;
        file.write_all(input).map_err(|e| e.to_string())?;
        file.rewind().map_err(|e| e.to_string())?;
        Stdio::from(file)
    } else { Stdio::null() };
    let output = crate::bounded_process::run_command_with_stdin(
        &mut cmd, crate::bounded_process::Deadline::after(TIMEOUT), TIMEOUT, stdin,
    ).map_err(|e| format!("TRAIN_TRANSFER_FENCE_UNAVAILABLE: {e:?}"))?;
    if !output.status.success() {
        return Err(format!("TRAIN_TRANSFER_FENCE_UNAVAILABLE: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn reference(train: &str) -> Result<String, String> {
    if !train.starts_with("cas-") || !train[4..].bytes().all(|c| c.is_ascii_alphanumeric()) || train.len() == 4 {
        return Err("TRAIN_TRANSFER_FENCE_UNAVAILABLE: invalid train id".into());
    }
    Ok(format!("refs/cas/train-owner/{train}"))
}

fn record_commit(repo: &Path, parent: Option<&str>, record: &Record) -> Result<String, String> {
    let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
    let blob = command(repo, &["hash-object", "-w", "--stdin"], Some(&bytes))?;
    let tree = command(repo, &["mktree"], Some(format!("100644 blob {blob}\tauthority.json\n").as_bytes()))?;
    let mut args = vec!["commit-tree", &tree];
    if let Some(parent) = parent { args.extend(["-p", parent]); }
    command(repo, &args, Some(format!("Train authority: {}\n", record.operation).as_bytes()))
}

impl Snapshot {
    fn observed_target(&self, repo: &Path) -> Result<String, String> {
        let target = format!("refs/heads/{}", self.record.target);
        command(repo, &["check-ref-format", &target], None)?;
        // Read both refs in one advertisement. A proper concurrent publication
        // changes authority too; it is a CAS conflict, not an unfenced writer.
        let remote = command(repo, &["ls-remote", "--refs", "origin", &self.reference, &target], None)?;
        let refs: std::collections::BTreeMap<&str, &str> = remote.lines().filter_map(|line| {
            line.split_once('\t').map(|(sha, name)| (name, sha))
        }).collect();
        if refs.get(self.reference.as_str()).copied() != Some(self.tip.as_str()) {
            return Err("TRAIN_TRANSFER_CONFLICT: remote authority moved during observation".into());
        }
        refs.get(target.as_str()).map(|sha| sha.to_string())
            .ok_or_else(|| "TRAIN_TRANSFER_FENCE_UNAVAILABLE: target missing".into())
    }

    pub(crate) fn read(repo: &Path, train: &str) -> Result<Self, String> {
        let reference = reference(train)?;
        let remote = command(repo, &["ls-remote", "--refs", "origin", &reference], None)?;
        let tip = remote.split_whitespace().next().ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authority missing")?.to_string();
        command(repo, &["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "origin", &reference], None)?;
        let record: Record = serde_json::from_str(&command(repo, &["show", &format!("{tip}:authority.json")], None)?)
            .map_err(|e| format!("TRAIN_TRANSFER_FENCE_UNAVAILABLE: invalid record: {e}"))?;
        if record.protocol != 1 || record.train != train {
            return Err("TRAIN_TRANSFER_FENCE_UNAVAILABLE: record scope/protocol mismatch".into());
        }
        Ok(Self { reference, tip, record })
    }

    pub(crate) fn publish(&self, git: &GitOperations, actor: &Identity, head: &str) -> Result<TargetPushOutcome, String> {
        let live = Self::read(&git.repo_root, &self.record.train)?;
        if live.record.generation != self.record.generation || live.record.identity != *actor {
            return Err("TRAIN_OWNER_GENERATION_CHANGED: owner/incarnation/generation changed".into());
        }
        if live.record.phase != Phase::Active {
            return Err("TRAIN_OWNER_DRAINING: train publications are held".into());
        }
        if live.observed_target(&git.repo_root)? != live.record.target_sha {
            return Err("TRAIN_TRANSFER_TARGET_DIVERGED: adopt the exact remote SHA through operator recovery".into());
        }
        if live.tip != self.tip {
            return Err("TRAIN_TRANSFER_CONFLICT: admitted authority snapshot moved".into());
        }
        let head = git.resolve_commit(head).ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: source missing")?;
        if !git.is_ancestor(&live.record.target_sha, &head) {
            return Err("TRAIN_TRANSFER_CONFLICT: candidate does not preserve the recorded target".into());
        }
        let mut next = live.record.clone();
        next.target_sha = head.clone();
        next.operation = format!("publish:{}", uuid::Uuid::new_v4());
        let next_tip = record_commit(&git.repo_root, Some(&self.tip), &next)?;
        let update = AtomicUpdate { reference: self.reference.clone(), expected: self.tip.clone(), next: next_tip.clone() };
        let outcome = git.publish_pinned_source_with_lease(
            &next.target, &head, Some(&live.record.target_sha), false, Some(&update), TIMEOUT,
        );
        if outcome.is_published() {
            let confirmed = Self::read(&git.repo_root, &next.train)?;
            if !git.is_ancestor(&next_tip, &confirmed.tip)
                || confirmed.observed_target(&git.repo_root)? != confirmed.record.target_sha {
                return Err("TRAIN_TRANSFER_TARGET_DIVERGED: publication readback disagrees with authority history".into());
            }
        }
        Ok(outcome)
    }
}

#[cfg(test)]
#[path = "train_authority_tests.rs"]
mod tests;
