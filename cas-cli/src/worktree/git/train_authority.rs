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
}

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub reference: String,
    pub tip: String,
    pub record: Record,
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

    pub(crate) fn publish(&self, git: &GitOperations, _actor: &Identity, head: &str) -> Result<TargetPushOutcome, String> {
        // Initial public seam preserves the existing real publication path;
        // the regression demonstrates the missing atomic authority update.
        Ok(git.publish_pinned_source_to_origin(&self.record.target, head, TIMEOUT))
    }
}

#[cfg(test)]
#[path = "train_authority_tests.rs"]
mod tests;
