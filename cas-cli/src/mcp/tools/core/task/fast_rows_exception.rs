//! Supervisor-issued exceptions shared by push and park. A skip reason is
//! untrusted; issuing this receipt requires live registered supervisor authority.
//! Only the historical-draft row has complete per-file failure scope today.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use cas_types::Task;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PREFIX: &str = "fast-rows-approved: ";
const CHECK: &str = "release-notes-draft";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    head: String,
    check: String,
    files: Vec<String>,
    reason: String,
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    schema: u32,
    task_id: String,
    head: String,
    check: String,
    files: Vec<String>,
    approver: String,
    reason: String,
    failure_sha256: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn full_sha(head: &str) -> bool {
    head.len() == 40
        && head
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn draft_path(path: &str) -> bool {
    path.strip_prefix("docs/release-notes/")
        .is_some_and(|name| {
            name.strip_suffix("-slack.md")
                .or_else(|| name.strip_suffix("-thread.md"))
                .is_some_and(|stem| !stem.is_empty())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}

fn scope(text: &str, head: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    let header = lines.next()?;
    if !full_sha(head)
        || text.lines().any(|line| line.starts_with("UNSCOPED "))
        || !header.starts_with(&format!("fast-rows: FAIL {head} "))
        || header
            .split_whitespace()
            .filter(|part| part.starts_with("rows="))
            .collect::<Vec<_>>()
            != ["rows=release-notes-draft"]
    {
        return None;
    }
    let files = lines
        .filter_map(|line| line.strip_prefix("SCOPE release-notes-draft "))
        .collect::<BTreeSet<_>>();
    let complete = format!("scope complete count={}", files.len());
    if text.lines().last() != Some(complete.as_str())
        || text.lines().filter(|line| line.starts_with("scope complete count=")).count() != 1
        || text.lines().filter(|line| line.starts_with("SCOPE release-notes-draft ")).count() != files.len()
    {
        return None;
    }
    (!files.is_empty() && files.iter().all(|file| draft_path(file)))
        .then(|| files.into_iter().map(str::to_owned).collect())
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Park reads only: the original failed result remains authoritative, with
/// approval of precisely those failed draft files recorded separately.
pub(super) fn approved(directory: &Path, task_id: &str, head: &str, failure: &str) -> bool {
    let Some(files) = scope(failure, head) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let expected_hash = digest(failure.as_bytes());
    entries
        .flatten()
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.starts_with(&format!("{head}.")) && name.ends_with(".exception.json")
            })
        })
        .any(|entry| {
            std::fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok())
                .is_some_and(|receipt| {
                    receipt.schema == 1
                        && receipt.task_id == task_id
                        && receipt.head == head
                        && receipt.check == CHECK
                        && receipt.files == files
                        && receipt.failure_sha256 == expected_hash
                        && !receipt.approver.trim().is_empty()
                        && !receipt.reason.trim().is_empty()
                })
        })
}

pub(super) struct Prepared {
    path: PathBuf,
    pub(super) audit: String,
}

impl Prepared {
    pub(super) fn publish(self) -> Result<(), String> {
        let parent = self.path.parent().ok_or("missing exception directory")?;
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        use std::io::Write;
        file.write_all(self.audit.as_bytes())
            .map_err(|error| error.to_string())?;
        file.persist(&self.path)
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// Called only by the note mutation, never by parsing existing note text or
/// overrides.log. Request JSON cannot provide its own approver identity.
pub(super) fn prepare(
    core: &crate::mcp::CasCore,
    task: &Task,
    note_type: &str,
    note: &str,
) -> Result<Option<Prepared>, String> {
    let Some(json) = note.trim().strip_prefix(PREFIX) else {
        return Ok(None);
    };
    let supervisor = core.resolve_live_supervisor_authority().map_err(|_| {
        "FAST ROW EXCEPTION REJECTED: only a live registered supervisor can approve".to_string()
    })?;
    if !note_type.eq_ignore_ascii_case("decision") {
        return Err("FAST ROW EXCEPTION REJECTED: use note_type=decision".into());
    }
    let request: Request = serde_json::from_str(json)
        .map_err(|error| format!("invalid fast-row approval: {error}"))?;
    if request.check != CHECK || !full_sha(&request.head) || request.reason.trim().is_empty() {
        return Err("FAST ROW EXCEPTION REJECTED: requires exact head, check=release-notes-draft and non-empty reason".into());
    }
    let agents = core.open_agent_store().map_err(|error| error.to_string())?;
    let worker = task
        .assignee
        .as_deref()
        .and_then(|assignee| super::resolve_agent_identity(agents.as_ref(), assignee))
        .ok_or("FAST ROW EXCEPTION REJECTED: task has no registered assignee")?;
    if worker.role != cas_types::AgentRole::Worker
        || worker.factory_session != supervisor.factory_session
    {
        return Err(
            "FAST ROW EXCEPTION REJECTED: supervisor and worker factory sessions differ".into(),
        );
    }
    let target = task
        .deliverables
        .work_target
        .as_ref()
        .ok_or("task has no work target")?;
    let context = super::repo_context::resolve_repo_context(&core.cas_root, target)?;
    let fallback = format!("factory/{}", worker.name);
    let branch = task
        .deliverables
        .factory_branch_start
        .as_ref()
        .map_or(fallback.as_str(), |start| start.branch.as_str());
    if git(
        &context.repo_root,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )
    .as_deref()
        != Some(request.head.as_str())
    {
        return Err(
            "FAST ROW EXCEPTION REJECTED: head must be the task's current delivery branch tip"
                .into(),
        );
    }
    let directory = context.git_common_dir.join("cas/fast-rows");
    let failed = std::fs::read_to_string(directory.join(format!("{}.pass", request.head)))
        .map_err(|_| "FAST ROW EXCEPTION REJECTED: run fast rows first; no failed receipt")?;
    let files = scope(&failed, &request.head).ok_or("FAST ROW EXCEPTION REJECTED: requires a failed draft-only receipt with complete file scope")?;
    let requested = request
        .files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if requested != files || requested.len() != request.files.len() {
        return Err(
            "FAST ROW EXCEPTION REJECTED: files must name exactly every failed draft".into(),
        );
    }
    if task.id.is_empty()
        || !task
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("invalid task id for exception receipt".into());
    }
    let path = directory.join(format!("{}.{}.exception.json", request.head, task.id));
    let receipt = Receipt {
        schema: 1,
        task_id: task.id.clone(),
        head: request.head,
        check: request.check,
        files,
        approver: supervisor.id,
        reason: request.reason,
        failure_sha256: digest(failed.as_bytes()),
    };
    let audit = serde_json::to_string(&receipt).map_err(|error| error.to_string())?;
    Ok(Some(Prepared { path, audit }))
}
