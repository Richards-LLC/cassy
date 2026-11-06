//! The existing daemon CI scan consumes train receipts; it never runs tests.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub(crate) fn directory(root: &Path, train: &str) -> Result<PathBuf> {
    let factory = crate::config::Config::load(root)?.factory();
    Ok(
        crate::config::resolved_factory_artifact_paths(root, factory.artifacts_root.as_deref())
            .task_dirs(train)[0]
            .join("train-ci"),
    )
}

/// A known red holds an existing queue even while its repair's tip is pending.
/// An override belongs to one observed red run, never to a future failure.
pub(crate) fn guard(
    core: &crate::mcp::CasCore,
    train: &str,
    branch: &str,
    prior_override: Option<&str>,
    supervisor_override: bool,
    reason: Option<&str>,
) -> Result<Option<String>> {
    ensure!(
        supervisor_override || reason.is_none(),
        "train_merge reason requires supervisor_override=true"
    );
    if supervisor_override {
        ensure!(
            reason.is_some_and(|reason| !reason.trim().is_empty()),
            "train suite override requires a nonempty reason"
        );
    }
    let path = directory(&core.cas_root, train)?.join("state.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let state: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        state["branch"].as_str() == Some(branch),
        "train suite receipt belongs to another branch; refresh CI"
    );
    if state["red_since"].as_str().is_none() {
        return Ok(None);
    }
    let token = format!(
        "{}:{}:{}",
        state["run_id"]
            .as_str()
            .context("red receipt has no run id")?,
        state["run_attempt"]
            .as_u64()
            .context("red receipt has no run attempt")?,
        state["red_head"]
            .as_str()
            .context("red receipt has no tested head")?
    );
    if prior_override == Some(&token) {
        return Ok(Some(token));
    }
    if supervisor_override {
        let actor = core.resolve_live_supervisor_authority().map_err(|_| {
            anyhow::anyhow!("train suite override requires a live registered supervisor")
        })?;
        let reason = reason.unwrap().trim().replace(['\n', '\r'], " ");
        core.open_task_store()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
            .append_note(
                train,
                &format!(
                    "TRAIN SUITE OVERRIDE: supervisor {} accepted red {token}; reason: {reason}",
                    actor.id
                ),
            )?;
        return Ok(Some(token));
    }
    anyhow::bail!(
        "TRAIN SUITE RED: {branch} is held at tested merge {} (run {}). Failing tests: {}. Complete log: {}. Repair until green, or use supervisor_override=true with a visible reason.",
        state["red_head"].as_str().unwrap_or("unknown"),
        state["run_id"].as_str().unwrap_or("unknown"),
        state["failures"],
        state["complete_log"].as_str().unwrap_or("see CI artifact")
    )
}

pub(crate) fn notify(root: &Path, epic: &cas_types::Task, state: &Value) -> Result<()> {
    if state["outcome"] != "FAIL" {
        return Ok(());
    }
    let introducing = state["introducing_merge"]
        .as_str()
        .map(|sha| format!("Introducing merge: {sha}."))
        .unwrap_or_else(|| {
            format!(
                "Introducing merge unproven; last-green→red merge/task range: {}.",
                state["merge_range"]
            )
        });
    let detail = format!(
        "Train {} is red at tested merge {}. {introducing}\nFailing tests: {}\nComplete log: {}. CI artifact {} at {}. train_merge is held until green or a visible supervisor override.",
        epic.id,
        state["head"].as_str().unwrap_or("unknown"),
        state["failures"],
        state["complete_log"].as_str().unwrap_or("unavailable"),
        state["artifact"],
        state["run_url"]
    );
    crate::supervisor_notices::release_action(
        root,
        epic,
        crate::supervisor_notices::ReleaseNoticePhase::RustRed,
        &format!("{}:{}", state["run_id"], state["run_attempt"]),
        &detail,
    )?;
    Ok(())
}

pub(crate) fn observe(root: &Path, session: &str) -> Result<()> {
    let tasks = crate::store::open_task_store(root)?;
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    for epic in tasks
        .list(None)?
        .into_iter()
        .filter(|task| !task.is_terminal())
    {
        let Some(branch) = crate::train_guardrails::train_lane(&epic) else {
            continue;
        };
        if crate::mcp::tools::core::task::release_train::is_cut_train(&epic) {
            continue;
        }
        if let Some(owner) = epic.epic_verification_owner.as_deref()
            && let Some(agent) = agents
                .iter()
                .find(|agent| agent.id == owner || agent.name == owner)
            && agent.factory_session.as_deref() != Some(session)
        {
            continue;
        }
        let result = (|| -> Result<()> {
            let repo = crate::train_guardrails::train_repo_root(root, &epic)?;
            let git = crate::worktree::GitOperations::new(repo.clone());
            let head = git
                .fetch_merge_target(branch)?
                .or_else(|| git.resolve_commit(branch))
                .context("train target missing")?;
            let timeout = Duration::from_secs(90);
            let mut command = Command::new("python3");
            command
                .arg(repo.join("scripts/train-ci.py"))
                .args(["observe", "--repo"])
                .arg(&repo)
                .args(["--branch", branch, "--head", &head, "--out"])
                .arg(directory(root, &epic.id)?)
                .current_dir(&repo);
            let output = crate::bounded_process::run_command(
                &mut command,
                crate::bounded_process::Deadline::after(timeout),
                timeout,
            )
            .map_err(|error| anyhow::anyhow!("train CI observation failed: {error:?}"))?;
            ensure!(
                output.status.success(),
                "train CI observation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let state: Value = serde_json::from_slice(&output.stdout)?;
            notify(root, &epic, &state)
        })();
        if let Err(error) = result {
            tracing::warn!(train=%epic.id, %error, "train CI scan deferred");
        }
    }
    Ok(())
}
