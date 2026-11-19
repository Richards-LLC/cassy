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

/// Cut refusals `release.sh` records as a release red (GH #2027).
pub(crate) const RELEASE_RED_GATES: &[&str] = &["ZERO_WOULD_CLOSE", "REVIEW_MISSING", "LANE_CI_MISSING"];
const RELEASE_RED: &str = "release-red.json";

/// A Cassy gate refused the cut of `head`, a green tip of `train`'s `branch`:
/// the train is red for release, so G-100 admits a `release-repair` merge.
/// It never holds train_merge (G-114 reads only the suite receipt).
pub(crate) fn record_release_red(root: &Path, train: &str, branch: &str, head: &str, gate: &str) -> Result<PathBuf> {
    ensure!(RELEASE_RED_GATES.contains(&gate), "unknown cut refusal {gate}; expected one of {}", RELEASE_RED_GATES.join(", "));
    ensure!(head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()), "commit must be a full SHA");
    let task = crate::store::open_task_store(root)?.get(train)?;
    ensure!(
        crate::mcp::tools::core::task::release_train::is_release_train(&task)
            && task.effective_epic_branch() == Some(branch),
        "{train} is not the release train of {branch}"
    );
    let dir = directory(root, train)?;
    std::fs::create_dir_all(&dir)?;
    let receipt = serde_json::json!({
        "branch": branch, "head": head, "gate": gate, "recorded_at": chrono::Utc::now().to_rfc3339(),
    });
    let path = dir.join(RELEASE_RED);
    crate::cli::integrate::fs::atomic_write(&path, &serde_json::to_string_pretty(&receipt)?)?;
    Ok(path)
}

/// The latest recorded cut refusal of `train`, if any.
pub(crate) fn release_red(root: &Path, train: &str) -> Option<Value> {
    let bytes = std::fs::read(directory(root, train).ok()?.join(RELEASE_RED)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A known red holds an existing queue even while its repair's tip is pending.
/// An override belongs to one observed red run, never to a future failure.
// gate[G-114 train-merge as-of]: Train suite red hold
// gate-audit[supervisor_override]: bypass G-114 Train-merge producer tags only a newly accepted red-run token; replaying the same token adds no fact.
// gate-audit[prior_override]: selector Replays the identical stored CI exception; only a newly accepted token emits G-114.
// gate-audit[accepted_by]: train_merge
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
        crate::gates::tag("G-114", "train_merge reason requires supervisor_override=true")
    );
    if supervisor_override {
        ensure!(
            reason.is_some_and(|reason| !reason.trim().is_empty()),
            crate::gates::tag("G-114", "train suite override requires a nonempty reason")
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
        crate::gates::tag("G-114", "train suite receipt belongs to another branch; refresh CI")
    );
    if state["red_since"].as_str().is_none() {
        return Ok(None);
    }
    let token = format!(
        "{}:{}:{}",
        state["run_id"]
            .as_str()
            .context(crate::gates::tag("G-114", "red receipt has no run id"))?,
        state["run_attempt"]
            .as_u64()
            .context(crate::gates::tag("G-114", "red receipt has no run attempt"))?,
        state["red_head"]
            .as_str()
            .context(crate::gates::tag("G-114", "red receipt has no tested head"))?
    );
    if prior_override == Some(&token) {
        return Ok(Some(token));
    }
    if supervisor_override {
        let actor = core.resolve_live_supervisor_authority().map_err(|_| {
            anyhow::anyhow!(crate::gates::tag("G-114", "train suite override requires a live registered supervisor"))
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
    anyhow::bail!(crate::gates::tag("G-114", format!(
        "TRAIN SUITE RED: {branch} is held at tested merge {} (run {}). Failing tests: {}. Complete log: {}. Repair until green, or use supervisor_override=true with a visible reason.",
        state["red_head"].as_str().unwrap_or("unknown"),
        state["run_id"].as_str().unwrap_or("unknown"),
        state["failures"],
        state["complete_log"].as_str().unwrap_or("see CI artifact")
    )))
}

/// A green full set at a new tip after a red (or with no earlier verdict) is
/// the moment a release becomes cuttable: wake the supervisor once per tip,
/// through the same `release_cut_ready` path `notify_train_cuts` uses.
fn notify_green(root: &Path, epic: &cas_types::Task, previous: Option<&Value>, state: &Value) -> Result<()> {
    let Some(head) = state["head"].as_str().filter(|_| state["outcome"] == "PASS") else {
        return Ok(());
    };
    let after_red_or_nothing = previous.is_none_or(|previous| {
        previous["last_green"].as_str() != Some(head)
            && (previous["red_since"].is_string() || previous["last_green"].is_null())
    });
    // cas-e060: where auto-cut applies, the cut poll acts on this green and
    // wakes the supervisor only when it needs notes or declines.
    if !after_red_or_nothing || crate::mcp::tools::core::workflow::release_auto_cut::enabled(root, epic) {
        return Ok(());
    }
    crate::supervisor_notices::release_action(
        root,
        epic,
        crate::supervisor_notices::ReleaseNoticePhase::CutReady,
        head,
        &format!(
            "Train {} is green at {head}, run {}: cut it.",
            epic.id,
            state["run_id"].as_str().unwrap_or("unknown")
        ),
    )?;
    Ok(())
}

pub(crate) fn notify(
    root: &Path,
    epic: &cas_types::Task,
    previous: Option<&Value>,
    state: &Value,
) -> Result<()> {
    if state["outcome"] == "PASS" {
        return notify_green(root, epic, previous, state);
    }
    if state["outcome"] != "FAIL" {
        return Ok(());
    }
    let introducing = state["introducing_merge"]
        .as_str()
        .map(|sha| format!("Introducing merge: {sha}."))
        .unwrap_or_else(|| "Introducing merge unproven.".into());
    let detail = format!(
        "Train {} is red at tested merge {}. {introducing}\nLast tested green: {}. Observed merge/task range: {}.\nFailing tests: {}\nComplete log: {}. CI artifact {} at {}. train_merge is held until green or a visible supervisor override.",
        epic.id,
        state["head"].as_str().unwrap_or("unknown"),
        state["last_green"].as_str().unwrap_or("unknown"),
        state["merge_range"],
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
    // GH #1613: heal trains the old default opened, with no operator step.
    crate::mcp::tools::core::task::release_train::retire_disabled_trains(root, tasks.as_ref());
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    for epic in tasks
        .list(None)?
        .into_iter()
        .filter(|task| !task.is_terminal())
    {
        let Some(branch) = crate::train_guardrails::train_lane(&epic) else {
            continue;
        };
        if let Some(owner) = epic.epic_verification_owner.as_deref()
            && let Some(agent) = agents
                .iter()
                .find(|agent| agent.id == owner || agent.name == owner)
            && agent.factory_session.as_deref() != Some(session)
        {
            continue;
        }
        if crate::mcp::tools::core::task::release_train::is_cut_train(&epic)
            && std::fs::read(directory(root, &epic.id)?.join("state.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|state| {
                    let complete = state["queue_seconds"].is_number();
                    if complete {
                        record_timings(root, &epic.id, &state);
                    }
                    complete
                })
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
            let previous = std::fs::read(directory(root, &epic.id)?.join("state.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            // cas-9765: run the script the train tip carries. The project
            // checkout's working tree can trail the train by days; on Nov 10
            // it predated the script, so every scan failed and a green frozen
            // train woke nobody and was never auto-cut.
            let out = directory(root, &epic.id)?;
            std::fs::create_dir_all(&out)?;
            let script = out.join("train-ci.py");
            let source = Command::new("git")
                .current_dir(&repo)
                .args(["show", &format!("{head}:scripts/train-ci.py")])
                .output()?;
            ensure!(source.status.success(), "train tip {head} has no scripts/train-ci.py");
            std::fs::write(&script, &source.stdout)?;
            let timeout = Duration::from_secs(90);
            let mut command = Command::new("python3");
            command
                .arg(&script)
                .args(["observe", "--repo"])
                .arg(&repo)
                .args(["--branch", branch, "--head", &head, "--out"])
                .arg(&out)
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
            let mut state: Value = serde_json::from_slice(&output.stdout)?;
            // The script owns verdicts; retain the dispatch history even when
            // a completed run replaces its entire receipt.
            state["dispatches"] = previous.as_ref()
                .map(|state| state["dispatches"].clone())
                .filter(Value::is_object)
                .unwrap_or_else(|| serde_json::json!({}));
            if let Err(error) = dispatch_frozen(root, &epic, &repo, branch, &head, &mut state) {
                tracing::warn!(train=%epic.id, %error, "train suite dispatch deferred");
            }
            crate::cli::integrate::fs::atomic_write(
                &out.join("state.json"), &serde_json::to_string_pretty(&state)?,
            )?;
            scan_result(root, &epic, branch, previous.as_ref(), &mut state)
        })();
        if let Err(error) = result {
            tracing::warn!(train=%epic.id, %error, "train CI scan deferred");
        }
    }
    Ok(())
}

/// cas-0fc56: the train suite runs once per frozen tip, never per merge. Lane
/// CI proved each delivery; a frozen train accepts no merge that could
/// supersede the run. A repair or plumbing commit on a frozen train is a new
/// tip and gets its own single run.
fn dispatch_frozen(root: &Path, epic: &cas_types::Task, repo: &Path, branch: &str, head: &str, state: &mut Value) -> Result<Value> {
    use fs2::FileExt;
    let out = directory(root, &epic.id)?;
    std::fs::create_dir_all(&out)?;
    // Freeze and the daemon can reach this path concurrently. Keep accepted
    // requests outside state.json, which the Python observer also replaces.
    let lock = std::fs::OpenOptions::new().create(true).truncate(false)
        .read(true).write(true).open(out.join("dispatch.lock"))?;
    lock.lock_exclusive()?;
    let path = out.join("dispatches.json");
    state["dispatches"] = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            state["dispatches"].as_object().cloned().map(Value::Object)
                .unwrap_or_else(|| serde_json::json!({}))
        }
        Err(error) => return Err(error.into()),
    };
    let report = dispatch_frozen_locked(root, epic, repo, branch, head, state)
        .unwrap_or_else(|error| dispatch_report(head, None, Some("dispatch_failed"), &error.to_string()));
    state["dispatches"][head]["report"] = report.clone();
    crate::cli::integrate::fs::atomic_write(&path, &serde_json::to_string_pretty(&state["dispatches"])?)?;
    Ok(report)
}

fn dispatch_report(head: &str, run: Option<&Value>, reason: Option<&str>, detail: &str) -> Value {
    serde_json::json!({
        "head": head,
        "run_id": run.and_then(|run| run["id"].as_u64()).map(|id| id.to_string()),
        "run_url": run.and_then(|run| run["html_url"].as_str()),
        "status": run.map(|run| &run["status"]),
        "conclusion": run.map(|run| &run["conclusion"]),
        "skip_reason": reason, "detail": detail,
    })
}

/// Metadata establishes run identity only; it never creates a suite verdict.
fn frozen_run(repo: &Path, branch: &str, head: &str, dead: Option<&str>) -> Result<Option<Value>> {
    let endpoint = format!(
        "repos/{{owner}}/{{repo}}/actions/workflows/full-validation.yml/runs?branch={}&event=workflow_dispatch&per_page=100",
        urlencoding::encode(branch),
    );
    let response = gh_json(repo, &endpoint)?;
    let runs = response["workflow_runs"].as_array().context("train run lookup returned no workflow_runs")?;
    Ok(runs.iter().filter(|run| {
        run["head_sha"] == head && run["head_branch"] == branch && run["event"] == "workflow_dispatch"
            && !matches!(run["conclusion"].as_str(), Some("cancelled" | "skipped"))
            && run["id"].as_u64().is_some_and(|id| dead != Some(id.to_string().as_str()))
    }).max_by_key(|run| (run["id"].as_u64(), run["run_attempt"].as_u64())).cloned())
}

fn dispatch_frozen_locked(root: &Path, epic: &cas_types::Task, repo: &Path, branch: &str, head: &str, state: &mut Value) -> Result<Value> {
    if crate::mcp::tools::core::workflow::release_freeze::active(root, epic).is_none() {
        return Ok(dispatch_report(head, None, Some("not_frozen"), "Train is not frozen; no run dispatched."));
    }
    if state["outcome"] != "PENDING" {
        return Ok(serde_json::json!({
            "head": head, "run_id": state["run_id"], "run_url": state["run_url"],
            "status": "completed", "conclusion": if state["outcome"] == "PASS" { "success" } else { "failure" },
            "skip_reason": "receipt_available", "detail": "Train suite receipt already observed.",
        }));
    }
    let workflow = Command::new("git").current_dir(repo)
        .args(["cat-file", "-e", &format!("{head}:.github/workflows/full-validation.yml")]).output()?;
    if !workflow.status.success() {
        return Ok(dispatch_report(head, None, Some("workflow_missing"), "Frozen tip has no .github/workflows/full-validation.yml; no run dispatched."));
    }
    let history = state["dispatches"][head].clone();
    let dead = history["dead_run_id"].as_str();
    if dead.is_some() && state["pending_run_id"].as_str() == dead {
        state["pending_run_id"] = Value::Null;
    }
    let marker = directory(root, &epic.id)?.join("dispatched-head");
    let pending = frozen_run(repo, branch, head, dead)?;
    let mut replaced = None;
    if let Some(run) = pending.as_ref() {
        let run_id = run["id"].as_u64().context("train run has no id")?.to_string();
        state["dispatches"][head]["run_id"] = run_id.clone().into();
        // Adopt manual runs as well as Cassy's requests before considering a
        // new dispatch. A completed failed run needs a repair, not a retry.
        std::fs::write(&marker, head)?;
        let old = run["created_at"].as_str()
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
            .is_some_and(|created| chrono::Utc::now().signed_duration_since(created)
                >= chrono::Duration::seconds(90));
        let stalled = dead.is_none() && run["status"] == "queued" && old
            && gh_json(repo, &format!("repos/{{owner}}/{{repo}}/actions/runs/{run_id}/jobs?per_page=1"))?["total_count"].as_u64() == Some(0);
        if !stalled {
            return Ok(dispatch_report(head, Some(run), Some("existing_run"), "Adopted the existing full-validation.yml run at this tip."));
        }
        replaced = Some(run_id);
    } else if dead.is_some() || history["requested_at"].is_string()
        || std::fs::read_to_string(&marker).is_ok_and(|text| text.trim() == head)
    {
        return Ok(dispatch_report(head, None, Some("already_dispatched"), "A dispatch was already accepted for this tip, but its run is not visible; no duplicate dispatched."));
    }
    let mut command = Command::new(crate::github_repo::gh_binary());
    command.args(["workflow", "run", "full-validation.yml", "--ref", branch]).current_dir(repo);
    let timeout = Duration::from_secs(30);
    let output = crate::bounded_process::run_command(
        &mut command, crate::bounded_process::Deadline::after(timeout), timeout,
    ).map_err(|error| anyhow::anyhow!("train suite dispatch failed: {error:?}"))?;
    ensure!(output.status.success(), "train suite dispatch failed: {}", String::from_utf8_lossy(&output.stderr));
    if let Some(run_id) = replaced {
        state["dispatches"][head]["dead_run_id"] = run_id.into();
        state["dispatches"][head]["run_id"] = Value::Null;
        state["pending_run_id"] = Value::Null;
    }
    // Persist acceptance before the follow-up API call: GitHub may take time
    // to list a new run, or the lookup can fail after a successful dispatch.
    state["dispatches"][head]["requested_at"] = chrono::Utc::now().to_rfc3339().into();
    std::fs::write(&marker, head)?;
    let run = frozen_run(repo, branch, head, state["dispatches"][head]["dead_run_id"].as_str());
    let report = match run {
        Ok(Some(run)) => dispatch_report(head, Some(&run), None, "Dispatched full-validation.yml at the frozen tip."),
        Ok(None) => dispatch_report(head, None, Some("awaiting_run_visibility"), "Dispatch accepted; GitHub has not listed its run yet."),
        Err(error) => dispatch_report(head, None, Some("run_lookup_failed"), &format!("Dispatch accepted; {error}")),
    };
    tracing::info!(train=%epic.id, %head, "train suite dispatched at frozen tip");
    Ok(report)
}

/// Start/adopt the suite during freeze itself, without running the receipt
/// observer or waiting for its daemon session to scan this train.
pub(crate) fn freeze_dispatch(root: &Path, epic: &cas_types::Task) -> Value {
    let result = (|| -> Result<Value> {
        let branch = epic.effective_epic_branch().context("train has no lane")?;
        let repo = crate::train_guardrails::train_repo_root(root, epic)?;
        let git = crate::worktree::GitOperations::new(repo.clone());
        let head = git.fetch_merge_target(branch)?.or_else(|| git.resolve_commit(branch)).context("train tip missing")?;
        let mut state: Value = std::fs::read(directory(root, &epic.id)?.join("state.json"))
            .ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        state["outcome"] = "PENDING".into();
        dispatch_frozen(root, epic, &repo, branch, &head, &mut state)
    })();
    result.unwrap_or_else(|error| dispatch_report("", None, Some("dispatch_failed"), &error.to_string()))
}

/// The current lane tip's last dispatch observation, shared by freeze output
/// and the Commander card. Never carry an earlier tip's URL or ETA forward.
pub(crate) fn current_run(root: &Path, epic: &cas_types::Task) -> Option<Value> {
    let repo = crate::train_guardrails::train_repo_root(root, epic).ok()?;
    let branch = epic.effective_epic_branch()?;
    let git = crate::worktree::GitOperations::new(repo);
    let head = git.resolve_commit(&format!("refs/remotes/origin/{branch}"))
        .or_else(|| git.resolve_commit(branch))?;
    let bytes = std::fs::read(directory(root, &epic.id).ok()?.join("dispatches.json")).ok()?;
    let history: Value = serde_json::from_slice(&bytes).ok()?;
    Some(history[head]["report"].clone())
}

/// Read run metadata without ever cancelling a GitHub run.
fn gh_json(repo: &Path, endpoint: &str) -> Result<Value> {
    let mut command = Command::new(crate::github_repo::gh_binary());
    command.args(["api", "-X", "GET", endpoint]).current_dir(repo);
    let timeout = Duration::from_secs(30);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(timeout),
        timeout,
    ).map_err(|error| anyhow::anyhow!("train run lookup failed: {error:?}"))?;
    ensure!(output.status.success(), "train run lookup failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr));
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Everything after the CI script: tie merges to tasks, record timings, notify.
/// `previous` is `state.json` as it stood before this scan.
fn scan_result(
    root: &Path,
    epic: &cas_types::Task,
    branch: &str,
    previous: Option<&Value>,
    state: &mut Value,
) -> Result<()> {
    // Merge subjects are not guaranteed to carry task IDs. Use the
    // same durable merge observations that release tracking consumes.
    if let Some(range) = state["merge_range"].as_array_mut() {
        let observed = cas_store::observed_delivery_merges_into(root, branch)?;
        for merge in range {
            let sha = merge["sha"].as_str().unwrap_or_default().to_owned();
            if let Some(tasks) = merge["tasks"].as_array_mut() {
                for delivery in observed.iter().filter(|row| row.merge_commit_sha == sha) {
                    let task = Value::String(delivery.task_id.clone());
                    if !tasks.contains(&task) {
                        tasks.push(task);
                    }
                }
            }
        }
    }
    record_timings(root, &epic.id, state);
    notify(root, epic, previous, state)
}

/// Durations already measured by CI, recorded once per immutable run and train.
fn record_timings(root: &Path, train: &str, state: &Value) {
    use cas_store::{CheckOutcome, CheckStage};
    for (id, stage, seconds, run, attempt, head, link) in [
        (
            "train_full_gate",
            CheckStage::Lane,
            "elapsed_seconds",
            "run_id",
            "run_attempt",
            "head",
            "run_url",
        ),
        (
            "merge_queue",
            CheckStage::Queue,
            "queue_seconds",
            "queue_run_id",
            "queue_run_attempt",
            "queue_head",
            "queue_run_url",
        ),
    ] {
        let Some(seconds) = state[seconds]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
        else {
            continue;
        };
        let Some(run) = state[run].as_str().filter(|value| !value.is_empty()) else {
            continue;
        };
        if state["schema"] != 2 || state["scope"] != "full-set" {
            continue;
        }
        let outcome = match state["outcome"].as_str() {
            Some("PASS") => CheckOutcome::Pass,
            Some("FAIL") if id == "train_full_gate" => CheckOutcome::Fail,
            _ => continue,
        };
        crate::quality_ledger::check_run(
            root,
            crate::quality_ledger::Check {
                natural_key: format!(
                    "{id}|{train}|{run}|{}",
                    state[attempt].as_u64().unwrap_or(1)
                ),
                check_id: id,
                stage,
                outcome,
                task_id: Some(train),
                sha: state[head].as_str(),
                minutes: Some(seconds / 60.0),
                link: state[link].as_str(),
                actor: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nov 7: red at one tip, then the repair's green tip woke nobody.
    #[test]
    fn green_after_red_wakes_the_supervisor_once_per_tip() {
        use cas_types::{Agent, AgentRole};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let mut owner = Agent::new_with_role("sup".into(), "sup".into(), AgentRole::Supervisor);
        owner.factory_session = Some("session".into());
        crate::store::open_agent_store(&root).unwrap().register(&owner).unwrap();
        let mut epic = cas_types::Task::new("cas-train".into(), "Train".into());
        epic.epic_verification_owner = Some(owner.id.clone());
        let receipt = |outcome: &str, head: &str, run: &str, previous: Option<&Value>| {
            let mut state = serde_json::json!({"outcome": outcome, "head": head, "run_id": run, "run_attempt": 1,
                "merge_range": [], "failures": [], "last_green": previous.and_then(|p| p["last_green"].clone().as_str().map(String::from))});
            if outcome == "PASS" { state["last_green"] = head.into(); } else { state["red_since"] = head.into(); }
            scan_result(&root, &epic, "factory/train", previous, &mut state).unwrap();
            state
        };
        let wakes = || {
            crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap()
                .into_iter().filter(|row| row.prompt.contains("release_cut_ready")).collect::<Vec<_>>()
        };

        let first_green = receipt("PASS", "aaaa", "10", None);
        assert_eq!(wakes().len(), 1, "no earlier verdict: the first green wakes");
        let red = receipt("FAIL", "bbbb", "11", Some(&first_green));
        assert_eq!(wakes().len(), 1, "red is train_ci_red, not a cut notice");
        let green = receipt("PASS", "cccc", "12", Some(&red));
        assert_eq!(wakes().len(), 2);
        assert!(wakes()[1].prompt.contains("cccc") && wakes()[1].prompt.contains("run 12"), "{}", wakes()[1].prompt);
        receipt("PASS", "cccc", "12", Some(&green));
        let next_green = receipt("PASS", "dddd", "13", Some(&green));
        assert_eq!(wakes().len(), 2, "a repeat, or a green with no red before it, wakes nobody");
        receipt("PASS", "eeee", "14", Some(&next_green));
        assert_eq!(wakes().len(), 2);
    }

    /// cas-9765, Nov 10: the project checkout sat on a main older than
    /// scripts/train-ci.py. The train was frozen, a release-repair merged,
    /// CI went green, and every scan failed on the missing script: no wake.
    #[test]
    fn frozen_green_train_wakes_once_when_the_checkout_lacks_the_script() {
        use cas_types::{Agent, AgentRole, TaskType};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            let status = Command::new("git").current_dir(repo).args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@example.test")
                .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.test")
                .status().unwrap();
            assert!(status.success(), "git {args:?}");
        };
        let write = |path: &str, text: &str| {
            std::fs::create_dir_all(repo.join(path).parent().unwrap()).unwrap();
            std::fs::write(repo.join(path), text).unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        write("README.md", "old main\n");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "old main without the script"]);
        let lane = "epic/release-train-vnext-cas-train";
        git(&["switch", "-q", "-c", lane]);
        // The observer contract: print the receipt and keep it as state.json.
        write("scripts/train-ci.py", "import argparse, json, pathlib\n\
p = argparse.ArgumentParser(); p.add_argument('cmd')\n\
for flag in ('--repo', '--branch', '--head', '--out'): p.add_argument(flag)\n\
a = p.parse_args()\n\
state = {'outcome': 'PASS', 'head': a.head, 'run_id': '19', 'run_attempt': 1, 'branch': a.branch,\n\
         'merge_range': [], 'failures': [], 'last_green': a.head}\n\
pathlib.Path(a.out, 'state.json').write_text(json.dumps(state))\n\
print(json.dumps(state))\n");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "train work"]);
        git(&["switch", "-q", "-c", "factory/repair"]);
        write("changelog.d/cas-repair.md", "### Fixed\n\n- **Repair** — fixed.\n");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "release repair"]);
        git(&["switch", "-q", lane]);

        let root = crate::store::init_cas_dir(repo).unwrap();
        std::fs::write(root.join("config.toml"), "[factory]\nrelease_train = true\n").unwrap();
        let mut owner = Agent::new_with_role("sup".into(), "sup".into(), AgentRole::Supervisor);
        owner.factory_session = Some("session".into());
        crate::store::open_agent_store(&root).unwrap().register(&owner).unwrap();
        let mut epic = cas_types::Task::new("cas-train".into(), "Release train".into());
        epic.task_type = TaskType::Epic;
        epic.labels.push(crate::mcp::tools::core::task::release_train::RELEASE_TRAIN_LABEL.into());
        epic.branch = Some(lane.into());
        epic.epic_verification_owner = Some(owner.id.clone());
        crate::store::open_task_store(&root).unwrap().add(&epic).unwrap();
        crate::mcp::tools::core::workflow::release_freeze::freeze(&root, "cas-train", "sup", Some("cut")).unwrap();
        // The verifier auto-merges the release repair into the frozen train,
        // and the operator's checkout goes back to its old main.
        git(&["merge", "-q", "--no-ff", "-m", "auto-merge release repair", "factory/repair"]);
        git(&["switch", "-q", "main"]);
        assert!(!repo.join("scripts/train-ci.py").exists());

        let wakes = || crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap()
            .into_iter().filter(|row| row.prompt.contains("release_cut_ready")).count();
        observe(&root, "session").unwrap();
        assert_eq!(wakes(), 1, "the green frozen tip wakes its supervisor");
        observe(&root, "session").unwrap();
        assert_eq!(wakes(), 1, "once per tip");
    }

    /// cas-0fc56, Nov 12: each train merge started a full suite and the next
    /// merge cancelled it; the first green came about 9 h later. Merges into
    /// an unfrozen train now start nothing, the freeze starts exactly one run
    /// at its tip, and that run's red is the train_ci_red notice.
    /// cas-bac4, Nov 13 6:00 PM ET: the supervisor dispatched the frozen tip
    /// by hand and the daemon added its own run one scan later.
    #[cfg(unix)]
    #[test]
    fn train_suite_runs_once_at_freeze_not_per_merge() {
        use cas_types::{Agent, AgentRole, TaskType};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            let status = Command::new("git").current_dir(repo).args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@example.test")
                .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@example.test")
                .status().unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::write(
            repo.join("scripts/train-ci.py"),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/train-ci.py")),
        )
        .unwrap();
        std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
        std::fs::write(repo.join(".github/workflows/full-validation.yml"),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../.github/workflows/full-validation.yml"))).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "train CI observer"]);
        let lane = "epic/release-train-vnext-cas-train";
        git(&["switch", "-q", "-c", lane]);
        let root = crate::store::init_cas_dir(repo).unwrap();
        std::fs::write(root.join("config.toml"), "[factory]\nrelease_train = true\n").unwrap();
        let mut owner = Agent::new_with_role("sup".into(), "sup".into(), AgentRole::Supervisor);
        owner.factory_session = Some("session".into());
        crate::store::open_agent_store(&root).unwrap().register(&owner).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        core.bind_agent_identity(owner.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal).unwrap();
        let mut epic = cas_types::Task::new("cas-train".into(), "Release train".into());
        epic.task_type = TaskType::Epic;
        epic.labels.push(crate::mcp::tools::core::task::release_train::RELEASE_TRAIN_LABEL.into());
        epic.branch = Some(lane.into());
        epic.epic_verification_owner = Some(owner.id.clone());
        crate::store::open_task_store(&root).unwrap().add(&epic).unwrap();
        // GitHub: a dispatch queues a run at the branch tip, and the runs
        // list shows every run; a run that has no artifact cannot download.
        let fixtures = tempfile::tempdir().unwrap();
        let (gh, forge, calls, runs) = (
            fixtures.path().join("gh"),
            fixtures.path().join("forge.py"),
            fixtures.path().join("gh-calls"),
            fixtures.path().join("runs.json"),
        );
        let forge_script = r#"import json, pathlib, subprocess, sys
args, runs = sys.argv[1:], pathlib.Path('RUNS')
listed = json.loads(runs.read_text()) if runs.exists() else []
open('CALLS', 'a').write(' '.join(args) + '\n')
if args[:2] == ['repo', 'view']: print('fixture/repo')
elif args[:2] == ['workflow', 'run']:
    ref = args[args.index('--ref') + 1]
    tip = subprocess.check_output(['git', 'rev-parse', ref], text=True).strip()
    if pathlib.Path('FAIL_DISPATCH').exists(): sys.exit(1)
    run_id = max((run['id'] for run in listed), default=99) + 1
    listed.append(dict(id=run_id, run_attempt=1, head_sha=tip, head_branch=ref,
                       html_url=f'https://github.com/fixture/repo/actions/runs/{run_id}',
                       event='workflow_dispatch', status='queued', conclusion=None))
    runs.write_text(json.dumps(listed))
elif args[0] == 'api':
    endpoint = next(arg for arg in args if arg.startswith('repos/'))
    if '/actions/runs/' in endpoint:
        run = next(run for run in listed if str(run['id']) == endpoint.split('/actions/runs/')[1].split('/')[0])
        print(json.dumps({'total_count': run.get('job_count', 0), 'jobs': []}
                         if '/jobs' in endpoint else run))
    else: print(json.dumps({'workflow_runs': listed}))
else: sys.exit(1)
"#;
        let fail_dispatch = fixtures.path().join("fail-dispatch");
        std::fs::write(&forge, forge_script.replace("RUNS", &runs.display().to_string())
            .replace("CALLS", &calls.display().to_string())
            .replace("FAIL_DISPATCH", &fail_dispatch.display().to_string())).unwrap();
        crate::test_paths::warm_stub(&gh, &format!("#!/bin/sh\nexec python3 '{}' \"$@\"\n", forge.display()));
        env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
        env.set("GH_BIN", &gh);
        let dispatches = || std::fs::read_to_string(&calls).unwrap_or_default().lines()
            .filter(|call| call.starts_with("workflow run ")).count();

        for merge in ["merge one", "merge two", "merge three"] {
            git(&["commit", "-q", "--allow-empty", "-m", merge]);
            observe(&root, "session").unwrap();
        }
        assert_eq!(dispatches(), 0, "merges into an unfrozen train start no train suite");

        let frozen = core.train_freeze("cas-train", Some("release-freeze: cut"), false).unwrap();
        let expected = format!("workflow run full-validation.yml --ref {lane}");
        let actual: Vec<_> = std::fs::read_to_string(&calls).unwrap().lines()
            .filter(|call| call.starts_with("workflow run ")).map(str::to_owned).collect();
        assert_eq!(actual, vec![expected]);
        let frozen: Value = serde_json::from_str(&frozen).unwrap();
        assert_eq!(frozen["train_ci"]["run_url"], "https://github.com/fixture/repo/actions/runs/100");
        let repeated: Value = serde_json::from_str(&core.train_freeze(
            "cas-train", Some("release-freeze: same tip"), false,
        ).unwrap()).unwrap();
        assert_eq!(dispatches(), 1);
        assert_eq!(repeated["train_ci"]["run_url"], frozen["train_ci"]["run_url"]);
        observe(&root, "session").unwrap();
        observe(&root, "session").unwrap();
        assert_eq!(dispatches(), 1, "the run already under way at the frozen tip is its one run");

        git(&["commit", "-q", "--allow-empty", "-m", "release repair"]);
        // A manually started run for the repair is adopted, not duplicated.
        let by_hand = Command::new(&gh).current_dir(repo)
            .args(["workflow", "run", "full-validation.yml", "--ref", lane]).status().unwrap();
        assert!(by_hand.success());
        let adopted: Value = serde_json::from_str(&core.train_freeze(
            "cas-train", Some("release-freeze: repair tip"), false,
        ).unwrap()).unwrap();
        assert_eq!(adopted["train_ci"]["run_url"], "https://github.com/fixture/repo/actions/runs/101");
        assert_eq!(dispatches(), 2);
        observe(&root, "session").unwrap();
        observe(&root, "session").unwrap();
        assert_eq!(dispatches(), 2, "the repair tip gets exactly one run of its own");
        let repair = String::from_utf8(Command::new("git").current_dir(repo)
            .args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap();
        assert_eq!(std::fs::read_to_string(directory(&root, "cas-train").unwrap().join("dispatched-head")).unwrap(), repair.trim());

        let mut listed: Value = serde_json::from_slice(&std::fs::read(&runs).unwrap()).unwrap();
        listed[1]["status"] = "completed".into();
        listed[1]["conclusion"] = "failure".into();
        std::fs::write(&runs, listed.to_string()).unwrap();
        observe(&root, "session").unwrap();
        let red = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap()
            .into_iter().filter(|row| row.prompt.contains("train_ci_red")).count();
        assert_eq!(red, 1, "red at the frozen tip raises train_ci_red");
        assert_eq!(dispatches(), 2);

        // Nov 16: run 43884793330 stayed queued with no jobs. Cancelling it
        // was refused; a replacement at the same head started normally.
        git(&["commit", "-q", "--allow-empty", "-m", "next frozen tip"]);
        observe(&root, "session").unwrap();
        assert_eq!(dispatches(), 3);
        let mut listed: Value = serde_json::from_slice(&std::fs::read(&runs).unwrap()).unwrap();
        let stalled = listed.as_array_mut().unwrap().last_mut().unwrap();
        stalled["id"] = 43884793330_u64.into();
        let head = stalled["head_sha"].as_str().unwrap().to_owned();
        let state_path = directory(&root, "cas-train").unwrap().join("state.json");
        let read_state = || serde_json::from_slice::<Value>(&std::fs::read(&state_path).unwrap()).unwrap();
        // Age alone, queued alone, and zero jobs alone are insufficient.
        for (age, status, jobs, expected) in [
            (10, "queued", 0, 3),
            (120, "queued", 1, 3),
            (120, "in_progress", 0, 3),
            (120, "queued", 0, 4),
        ] {
            listed[2]["created_at"] = (chrono::Utc::now() - chrono::Duration::seconds(age)).to_rfc3339().into();
            listed[2]["status"] = status.into();
            listed[2]["job_count"] = jobs.into();
            std::fs::write(&runs, listed.to_string()).unwrap();
            observe(&root, "session").unwrap();
            assert_eq!(dispatches(), expected, "age={age}, status={status}, jobs={jobs}");
        }
        let state = read_state();
        assert_eq!(state["dispatches"][&head]["dead_run_id"], "43884793330");
        assert!(state["pending_run_id"].is_null());
        // GitHub may still list only the dead run after accepting dispatch.
        let with_replacement = std::fs::read(&runs).unwrap();
        std::fs::write(&runs, listed.to_string()).unwrap();
        observe(&root, "session").unwrap();
        assert_eq!(dispatches(), 4);
        assert!(read_state()["pending_run_id"].is_null());

        std::fs::write(&runs, with_replacement).unwrap();
        let mut listed: Value = serde_json::from_slice(&std::fs::read(&runs).unwrap()).unwrap();
        let replacement = listed.as_array_mut().unwrap().last_mut().unwrap();
        assert_eq!(replacement["head_sha"], head);
        assert_eq!(replacement["head_branch"], lane);
        replacement["created_at"] = (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339().into();
        std::fs::write(&runs, listed.to_string()).unwrap();
        observe(&root, "session").unwrap();
        observe(&root, "session").unwrap();
        assert_eq!(dispatches(), 4, "a stalled replacement cannot start a retry chain");
        assert_eq!(read_state()["dispatches"][&head]["run_id"], "43884793331");
        assert!(!std::fs::read_to_string(&calls).unwrap().lines().any(|call|
            call.starts_with("run cancel ") || call.contains("/cancel")));
        git(&["commit", "-q", "--allow-empty", "-m", "dispatch refusal"]);
        std::fs::write(&fail_dispatch, "refuse dispatch").unwrap();
        let failed: Value = serde_json::from_str(&core.train_freeze(
            "cas-train", Some("release-freeze: report dispatch failure"), false,
        ).unwrap()).unwrap();
        assert_eq!(failed["train_ci"]["skip_reason"], "dispatch_failed");
        assert!(failed["train_ci"]["run_url"].is_null());
        let train = crate::store::open_task_store(&root).unwrap().get("cas-train").unwrap();
        let card = crate::mcp::tools::core::workflow::release_freeze::card(&root, &train).unwrap();
        assert!(card["eta"].is_null());
        assert_eq!(card["train_ci"]["skip_reason"], "dispatch_failed");
        // A legacy accepted request without a visible run must explain the
        // skip, and must not create another dispatch after a restart.
        let head = String::from_utf8(Command::new("git").current_dir(repo)
            .args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap();
        std::fs::write(directory(&root, "cas-train").unwrap().join("dispatched-head"), head.trim()).unwrap();
        let before = dispatches();
        let skipped: Value = serde_json::from_str(&core.train_freeze(
            "cas-train", Some("release-freeze: accepted request"), false,
        ).unwrap()).unwrap();
        assert_eq!(dispatches(), before);
        assert_eq!(skipped["train_ci"]["skip_reason"], "already_dispatched");
        assert!(skipped["train_ci"]["run_url"].is_null());
    }

    #[test]
    fn full_gate_timings_reach_the_friction_ledger_once_cas_8413() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let mut state = serde_json::json!({
            "schema": 1, "scope": "full-set", "outcome": "PASS", "run_id": "101",
            "elapsed_seconds": 600, "head": "a".repeat(40), "run_url": "https://github.com/example/repo/actions/runs/101",
            "queue_seconds": 120, "queue_run_id": "102", "queue_head": "b".repeat(40),
            "queue_run_url": "https://github.com/example/repo/actions/runs/102"
        });
        record_timings(&root, "cas-fixture", &state);
        assert!(
            cas_store::quality_events_for_task(&root, "cas-fixture")
                .unwrap()
                .is_empty()
        );
        state["schema"] = 2.into();
        record_timings(&root, "cas-fixture", &state);
        record_timings(&root, "cas-fixture", &state);
        let rows = cas_store::quality_events_for_task(&root, "cas-fixture").unwrap();
        assert_eq!(rows.len(), 2);
        state["run_attempt"] = 2.into();
        record_timings(&root, "cas-fixture", &state);
        assert_eq!(
            cas_store::quality_events_for_task(&root, "cas-fixture")
                .unwrap()
                .len(),
            3
        );
        for (id, expected) in [("train_full_gate", 10.0), ("merge_queue", 2.0)] {
            assert!(rows.iter().any(|row| matches!(&row.payload,
                cas_store::QualityPayload::CheckRun { check_id, minutes: Some(minutes), .. }
                    if check_id == id && *minutes == expected)));
        }
    }
}
