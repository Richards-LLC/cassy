//! Durable, supervisor-authorized retries of parked merges refused only for CI.
//! The daemon runs these off its control loop. A retry re-enters worktree_merge
//! with the original options, and never grants an override or relaxes its gates.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::bounded_process::{Deadline, run_command};
use crate::mcp::CasCore;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retry {
    pub task: String,
    pub worktree: String,
    pub branch: String,
    pub sha: String,
    pub ci_sha: String,
    pub target: String,
    pub repo: PathBuf,
    pub supervisor: String,
    pub session: String,
    pub force: bool,
    pub allow_trunk: bool,
    pub cleanup: Option<bool>,
}

fn directory(root: &Path) -> PathBuf {
    root.join("merge-ci-retries")
}

pub(crate) fn arm(root: &Path, request: &Retry) -> Result<()> {
    anyhow::ensure!(
        request
            .task
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid retry task id"
    );
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", request.task));
    let mut temporary = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(std::io::Write::by_ref(&mut temporary), request)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Minimal result shared with the normal CI admission classifier.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CiStatus {
    Green,
    Red,
    Pending,
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.current_dir(repo).args(args);
    let timeout = Duration::from_secs(5);
    let output = run_command(&mut command, Deadline::after(timeout), timeout)
        .map_err(|e| anyhow::anyhow!("retry Git lookup failed: {e:?}"))?;
    anyhow::ensure!(
        output.status.success(),
        "retry Git lookup failed: {}",
        output.status
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn source_matches(request: &Retry) -> Result<bool> {
    let remote = git(
        &request.repo,
        &[
            "ls-remote",
            "origin",
            &format!("refs/heads/{}", request.branch),
        ],
    )?;
    if remote.split_whitespace().next() != Some(request.sha.as_str()) {
        return Ok(false);
    }
    // A live worker can move its local branch before pushing. Both identities
    // must still name the authorized commit; an absent local ref is recovered
    // by worktree_merge's normal pushed-delivery path.
    if let Ok(local) = git(
        &request.repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", request.branch),
        ],
    ) {
        return Ok(local == request.sha);
    }
    Ok(true)
}

fn notice(root: &Path, request: &Retry, detail: &str, red: bool) -> Result<()> {
    let key = if red {
        // Share the existing CI watcher's key: polling order and daemon restarts
        // must not produce a second red-CI supervisor turn for the same lane SHA.
        format!("ci-red-run:{}:{}", request.branch, request.sha)
    } else {
        format!("merge-ci-retry:{}:{}", request.task, request.sha)
    };
    let worker = request
        .branch
        .strip_prefix("factory/")
        .unwrap_or(&request.branch);
    let body = crate::prompt_revalidation::attach_blocker_envelope(
        &format!(
            "Automatic merge stopped for {} at {}: {detail}. No automatic override was granted.",
            request.task, request.sha
        ),
        worker,
        Some(&request.task),
    );
    crate::store::open_prompt_queue_store(root)?.enqueue_idempotent(
        &format!("lifecycle-wake:{key}"),
        "supervisor",
        &body,
        Some(&request.session),
        Some("Parked merge requires attention"),
        Some(cas_store::NotificationPriority::High),
        &key,
        Some(&cas_store::QueueOrigin::Daemon),
    )?;
    Ok(())
}

/// Re-enter the real merge handler for one durable session's ready requests.
/// Public only for the integration harness; this is not a stable library API.
#[doc(hidden)]
pub async fn retry_pending(root: &Path, session: &str) -> Result<()> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("runner.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let request: Retry = serde_json::from_slice(&std::fs::read(&path)?)?;
        if request.session != session {
            continue;
        }
        match attempt(root, &request).await {
            Ok(false) => {}
            Ok(true) => {
                // A newer supervisor request may have replaced this file while
                // the merge was running. Never consume that new intent.
                let current: Retry = serde_json::from_slice(&std::fs::read(&path)?)?;
                if current == request {
                    std::fs::remove_file(&path)?;
                }
            }
            Err(error) => {
                // Transport/store outages retain intent; they are not a CI
                // result and must not turn a temporary outage into a merge.
                tracing::warn!(task=%request.task, %error, "CI merge retry deferred");
            }
        }
    }
    Ok(())
}

async fn attempt(root: &Path, request: &Retry) -> Result<bool> {
    let agents = crate::store::open_agent_store(root)?;
    let supervisor = agents.get(&request.supervisor)?;
    if supervisor.role != cas_types::AgentRole::Supervisor
        || !supervisor.is_alive()
        || supervisor.factory_session.as_deref() != Some(&request.session)
    {
        notice(
            root,
            request,
            "the authorizing supervisor session is no longer live",
            false,
        )?;
        return Ok(true);
    }
    let tasks = crate::store::open_task_store(root)?;
    let task = tasks.get(&request.task)?;
    if task.is_terminal() {
        return Ok(true);
    }
    if let Some((receipt, transaction)) =
        cas_store::get_latest_worker_delivery(root, &request.task)?
        && receipt.commit_sha == request.sha
        && matches!(
            transaction.state,
            cas_types::WorkerDeliveryState::Merged
                | cas_types::WorkerDeliveryState::CloseReady
                | cas_types::WorkerDeliveryState::Delivered
        )
    {
        return Ok(true);
    }
    if task.status != cas_types::TaskStatus::AwaitingMerge
        || task.deliverables.factory_branch_anchor.as_deref() != Some(&request.sha)
        || !source_matches(request)?
    {
        notice(
            root,
            request,
            "the parked lane moved or was reopened; re-park and request its new SHA",
            false,
        )?;
        return Ok(true);
    }
    use crate::mcp::tools::core::workflow::retry_ci_status;
    match retry_ci_status(
        &request.sha,
        &request.target,
        &request.repo,
        &request.ci_sha,
    ) {
        CiStatus::Pending => return Ok(false),
        CiStatus::Red => {
            notice(root, request, "CI completed red", true)?;
            return Ok(true);
        }
        CiStatus::Green => {}
    }
    let core = CasCore::with_daemon(root.to_path_buf(), None, None);
    core.bind_agent_identity(
        request.supervisor.clone(),
        crate::mcp::AgentIdentitySource::ServerInternal,
    )
    .map_err(|e| anyhow::anyhow!("retry identity binding failed: {e}"))?;
    let result = core
        .worktree_merge_checked(
            &request.worktree,
            request.force,
            Some(&request.task),
            request.allow_trunk,
            request.cleanup,
            false,
            None,
            Some(request),
        )
        .await;
    match result {
        // CI may change again between the poll and admission. Leave that
        // refused request queued, preserving the fresh exact-SHA check.
        Err(error) if error.message.contains("CI PENDING") => Ok(false),
        Err(error) => {
            notice(
                root,
                request,
                &format!("merge admission refused: {}", error.message),
                false,
            )?;
            Ok(true)
        }
        Ok(result) if result.is_error == Some(true) => {
            notice(
                root,
                request,
                &format!("merge handler refused: {:?}", result.content),
                false,
            )?;
            Ok(true)
        }
        Ok(_) => {
            tasks
                .append_note(
                    &request.task,
                    &format!(
                        "Automatic CI merge retry completed for {} on {}.",
                        request.sha, request.branch
                    ),
                )
                .context("record automatic retry outcome")?;
            Ok(true)
        }
    }
}
