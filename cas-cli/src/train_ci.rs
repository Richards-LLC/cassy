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
            scan_result(root, &epic, branch, previous.as_ref(), &mut state)
        })();
        if let Err(error) = result {
            tracing::warn!(train=%epic.id, %error, "train CI scan deferred");
        }
    }
    Ok(())
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
