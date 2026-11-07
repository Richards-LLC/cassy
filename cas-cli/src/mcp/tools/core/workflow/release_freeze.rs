//! Release freeze (cas-2f4d): the operator's release request as train state.
//!
//! The freeze is one label on the release-train epic, so it reaches a peer
//! machine through ordinary task sync and that machine's merge gate refuses
//! too. While a train is frozen, Cassy refuses worktree_merge into it, new
//! staffing on it and CI retries armed against it. A `release-repair` task is
//! the one exception, and only while the train is red: a recorded
//! train-suite red, or a failed GitHub train run at the frozen tip. The freeze
//! ends when its tip lands on the integration branch, or when the operator
//! lifts it in Commander. A supervisor cannot lift it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::mcp::tools::core::task::release_train::{is_cut_train, is_release_train};
use crate::types::Task;

const FROZEN_LABEL: &str = "release-train-frozen:";
/// Marks a task that repairs a red train; it may merge into a frozen train.
pub(crate) const REPAIR_LABEL: &str = "release-repair";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Freeze {
    pub at: DateTime<Utc>,
    pub tip: String,
}

/// The freeze recorded on an open release train, landed or not. `freeze`
/// writes exactly one label; task updates only append labels, so the first
/// one is authoritative and a later appended label cannot move its tip.
pub(crate) fn freeze_of(train: &Task) -> Option<Freeze> {
    if !is_release_train(train) || train.is_terminal() {
        return None;
    }
    train.labels.iter().find_map(|label| {
        let (secs, tip) = label.strip_prefix(FROZEN_LABEL)?.split_once(':')?;
        Some(Freeze { at: DateTime::from_timestamp(secs.parse().ok()?, 0)?, tip: tip.to_string() })
    })
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn repo_of(root: &Path, train: &Task) -> Option<PathBuf> {
    crate::train_guardrails::train_repo_root(root, train).ok()
}

/// A freeze that still holds: its tip has not reached the integration branch.
pub(crate) fn active(root: &Path, train: &Task) -> Option<Freeze> {
    let freeze = freeze_of(train)?;
    let integration = train.deliverables.work_target.as_ref().map(|target| target.target_branch.as_str());
    let landed = repo_of(root, train).zip(integration).is_some_and(|(repo, integration)| {
        [integration.to_string(), format!("origin/{integration}")]
            .iter()
            .any(|target| is_ancestor(&repo, &freeze.tip, target))
    });
    (!landed).then_some(freeze)
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn eastern(at: DateTime<Utc>) -> String {
    crate::cli::release_report::eastern_time(at)
}

fn is_repair(task: &Task) -> bool {
    task.labels.iter().any(|label| label == REPAIR_LABEL)
}

/// Evidence that the frozen train is red: the train-suite receipt names a red
/// at or after the frozen tip, or GitHub's latest train run at that tip
/// failed. Either one admits a repair; a missing receipt never locks it out.
fn red_evidence(root: &Path, train: &Task, freeze: &Freeze) -> Option<String> {
    let lane = train.effective_epic_branch()?;
    let repo = repo_of(root, train)?;
    let receipt = crate::train_ci::directory(root, &train.id)
        .ok()
        .and_then(|dir| std::fs::read(dir.join("state.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(state) = receipt
        && state["branch"].as_str() == Some(lane)
        && state["red_since"].is_string()
        && let Some(head) = state["red_head"].as_str()
        && is_ancestor(&repo, &freeze.tip, head)
    {
        return Some(format!(
            "train_ci_red run {} at {}",
            state["run_id"].as_str().unwrap_or("unknown"),
            short(head)
        ));
    }
    let mut command = Command::new(crate::github_repo::gh_binary());
    command
        .args(["run", "list", "--workflow", "full-validation.yml", "--branch", lane])
        .args(["--commit", &freeze.tip, "--event", "push", "--limit", "1"])
        .args(["--json", "databaseId,conclusion"])
        .current_dir(&repo);
    let timeout = Duration::from_secs(15);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(timeout),
        timeout,
    )
    .ok()
    .filter(|output| output.status.success())?;
    let runs: Value = serde_json::from_slice(&output.stdout).ok()?;
    let run = runs.as_array()?.first()?;
    (run["conclusion"] == "failure").then(|| {
        format!("GitHub run {} failed at frozen tip {}", run["databaseId"], short(&freeze.tip))
    })
}

fn refusal(train: &Task, freeze: &Freeze, subject: &str) -> String {
    format!(
        "TRAIN FROZEN: train frozen for release, parked for the next train. {} has been frozen since {} at tip {}; {subject}. \
         No merge was attempted. Only a `{REPAIR_LABEL}` task merges while the train is red. \
         The freeze ends when the release lands or the operator lifts it in Commander.",
        train.id,
        eastern(freeze.at),
        short(&freeze.tip),
    )
}

/// The merge gate: worktree_merge (before and under the target lock) and CI
/// retry arming. A repair admitted under a red is noted on the train once.
pub(crate) fn check_merge(root: &Path, target: &str, task_id: Option<&str>) -> Result<(), String> {
    let store = crate::store::open_task_store(root).map_err(|error| error.to_string())?;
    for train in store.list(None).map_err(|error| error.to_string())? {
        if freeze_of(&train).is_none() {
            continue;
        }
        let Some(lane) = train.effective_epic_branch() else { continue };
        if crate::mcp::tools::core::task::repo_context::effective_epic_ref(root, lane) != target {
            continue;
        }
        let Some(freeze) = active(root, &train) else { continue };
        let task = task_id.and_then(|id| store.get(id).ok());
        let Some(task) = task.filter(is_repair) else {
            let subject = task_id.map_or("this merge names no task".to_string(), |id| format!("{id} stays parked"));
            return Err(refusal(&train, &freeze, &subject));
        };
        let Some(red) = red_evidence(root, &train, &freeze) else {
            return Err(refusal(
                &train,
                &freeze,
                &format!("{} is marked {REPAIR_LABEL}, but no red is recorded at or after the frozen tip", task.id),
            ));
        };
        let line = format!("FREEZE REPAIR: {} merged into frozen {} under {red}", task.id, train.id);
        if !train.notes.contains(&line) {
            store.append_note(&train.id, &line).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    Ok(())
}

/// The staffing gate: spawning, assigning or starting new work on a frozen
/// train. Work already under way may resume; repairs may always start.
pub(crate) fn check_staffing(root: &Path, task_id: &str) -> Result<()> {
    let store = crate::store::open_task_store(root)?;
    let task = store.get(task_id)?;
    let (train, subject) = if is_release_train(&task) {
        (task, "no new workers join it".to_string())
    } else if task.status == crate::types::TaskStatus::Open && !is_repair(&task) {
        match store.get_parent_epic(task_id)? {
            Some(train) => (train, format!("{task_id} starts on the next train, not this one")),
            None => return Ok(()),
        }
    } else {
        return Ok(());
    };
    match active(root, &train) {
        Some(freeze) => anyhow::bail!(refusal(&train, &freeze, &subject).replace(" No merge was attempted.", "")),
        None => Ok(()),
    }
}

/// Median minutes of each release stage still ahead, from the friction
/// ledger: the train suite (unless the tip is already green), the merge
/// queue and publication. `None` names the stage that has no measurement.
pub(crate) fn remaining_minutes(root: &Path, train: &Task, now: DateTime<Utc>) -> Result<f64, &'static str> {
    use cas_store::QualityPayload;
    let events = cas_store::quality_events_between(root, Some(now - chrono::Duration::days(60)), Some(now))
        .unwrap_or_default();
    let median = |values: Vec<f64>| -> Option<f64> {
        let mut values: Vec<f64> = values.into_iter().rev().take(10).collect();
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        match values.len() {
            0 => None,
            len if len % 2 == 1 => Some(values[middle]),
            _ => Some((values[middle - 1] + values[middle]) / 2.0),
        }
    };
    let check = |id: &str| {
        median(
            events
                .iter()
                .filter(|event| event.project_id.is_empty())
                .filter_map(|event| match &event.payload {
                    QualityPayload::CheckRun { check_id, minutes, outcome, .. }
                        if check_id == id && *outcome == cas_store::CheckOutcome::Pass =>
                    {
                        *minutes
                    }
                    _ => None,
                })
                .collect(),
        )
    };
    let green = crate::train_ci::directory(root, &train.id)
        .ok()
        .and_then(|dir| std::fs::read(dir.join("state.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|state| {
            state["outcome"] == "PASS"
                && state["branch"].as_str() == train.effective_epic_branch()
                && repo_of(root, train).zip(state["head"].as_str()).is_some_and(|(repo, head)| {
                    train.effective_epic_branch().is_some_and(|lane| {
                        crate::worktree::GitOperations::new(repo).resolve_commit(lane).as_deref() == Some(head)
                    })
                })
        });
    let suite = if green { 0.0 } else { check("train_full_gate").ok_or("train suite")? };
    let queue = check("merge_queue").ok_or("merge queue")?;
    let publish = median(
        events
            .iter()
            .filter(|event| event.project_id.is_empty())
            .filter_map(|event| match &event.payload {
                QualityPayload::Release { main_to_published_minutes, .. } => *main_to_published_minutes,
                _ => None,
            })
            .collect(),
    )
    .ok_or("publish")?;
    Ok(suite + queue + publish)
}

/// "Frozen 2:05 PM ET · ETA 2:50 PM ET", or the stage that is unmeasured.
pub(crate) fn summary(root: &Path, train: &Task, freeze: &Freeze, now: DateTime<Utc>) -> String {
    let eta = match remaining_minutes(root, train, now) {
        Ok(minutes) => format!("ETA {}", eastern(now + chrono::Duration::seconds((minutes * 60.0) as i64))),
        Err(stage) => format!("ETA unmeasured: no {stage} timing"),
    };
    format!("Frozen {} · {eta}", eastern(freeze.at))
}

/// The Commander train card's freeze line, for an open train.
pub(crate) fn card(root: &Path, train: &Task) -> Option<Value> {
    let freeze = active(root, train)?;
    let now = Utc::now();
    let eta = remaining_minutes(root, train, now).ok().map(|minutes| (now + chrono::Duration::seconds((minutes * 60.0) as i64)).to_rfc3339());
    Some(serde_json::json!({
        "frozen_at": freeze.at.to_rfc3339(),
        "tip": freeze.tip,
        "eta": eta,
        "summary": summary(root, train, &freeze, now),
    }))
}

fn train(store: &dyn cas_store::TaskStore, train_id: &str) -> Result<Task> {
    let train = store.get(train_id).with_context(|| format!("{train_id} not found"))?;
    ensure!(is_release_train(&train) && !train.is_terminal(), "{train_id} is not an open release train");
    Ok(train)
}

/// Freeze a train at its current tip. Repeating it reports the standing freeze.
pub(crate) fn freeze(root: &Path, train_id: &str, actor: &str, reason: Option<&str>) -> Result<String> {
    let store = crate::store::open_task_store(root)?;
    let mut task = train(store.as_ref(), train_id)?;
    ensure!(!is_cut_train(&task), "{train_id} is already cut; it accepts no merges");
    if let Some(freeze) = active(root, &task) {
        return Ok(format!(
            "TRAIN FROZEN: {train_id} at {}. {}",
            short(&freeze.tip),
            summary(root, &task, &freeze, Utc::now())
        ));
    }
    let lane = task.effective_epic_branch().context("train has no lane")?.to_string();
    let git = crate::worktree::GitOperations::new(repo_of(root, &task).context("train repository missing")?);
    let tip = git
        .fetch_merge_target(&lane)
        .ok()
        .flatten()
        .or_else(|| git.resolve_commit(&lane))
        .with_context(|| format!("cannot resolve the tip of {lane}"))?;
    let now = Utc::now();
    task.labels.retain(|label| !label.starts_with(FROZEN_LABEL));
    task.labels.push(format!("{FROZEN_LABEL}{}:{tip}", now.timestamp()));
    task.updated_at = now;
    store.update(&task)?;
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty()).unwrap_or("release requested");
    store.append_note(
        train_id,
        &format!("RELEASE FREEZE: {actor} froze {train_id} at {} on tip {tip}: {reason}", eastern(now)),
    )?;
    let freeze = Freeze { at: DateTime::from_timestamp(now.timestamp(), 0).unwrap_or(now), tip };
    Ok(format!(
        "TRAIN FROZEN: {train_id} at {}. {}. Merges, new staffing and CI retries into {lane} are refused on every machine \
         except `{REPAIR_LABEL}` tasks while the train is red; new tasks join the next train. \
         The freeze ends when the release lands or the operator lifts it in Commander.",
        short(&freeze.tip),
        summary(root, &task, &freeze, now)
    ))
}

/// The operator's lift from Commander.
pub(crate) fn lift(root: &Path, train_id: &str, actor: &str) -> Result<String> {
    let store = crate::store::open_task_store(root)?;
    let mut task = train(store.as_ref(), train_id)?;
    ensure!(freeze_of(&task).is_some(), "{train_id} is not frozen");
    task.labels.retain(|label| !label.starts_with(FROZEN_LABEL));
    task.updated_at = Utc::now();
    store.update(&task)?;
    store.append_note(train_id, &format!("RELEASE FREEZE LIFTED: {actor} lifted the freeze on {train_id}"))?;
    Ok(format!("Freeze lifted on {train_id}; merges into the train resume."))
}

impl crate::mcp::CasCore {
    /// MCP `factory action=train_freeze id=<train>`: a live supervisor freezes.
    pub(crate) fn train_freeze(&self, train_id: &str, reason: Option<&str>) -> Result<String> {
        let actor = self
            .resolve_live_supervisor_authority()
            .map_err(|_| anyhow::anyhow!("train_freeze requires a live registered supervisor"))?;
        freeze(&self.cas_root, train_id, &format!("supervisor {}", actor.name), reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_types::{Dependency, DependencyType, TaskType, WorkTarget};
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.email=t@cas.test", "-c", "user.name=T"])
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// A repository with `main` and a train lane one commit ahead, its train
    /// epic and two parked children: a feature and a repair.
    fn fixture() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
        git(repo, &["add", "seed.txt"]);
        git(repo, &["commit", "-qm", "seed"]);
        git(repo, &["checkout", "-qb", "epic/release-train-vnext-cas-tr"]);
        std::fs::write(repo.join("work.txt"), "work\n").unwrap();
        git(repo, &["add", "work.txt"]);
        git(repo, &["commit", "-qm", "work"]);
        git(repo, &["checkout", "-q", "main"]);
        let root = repo.join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), "[project]\ncanonical_id = \"freeze-test\"\n").unwrap();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        let mut train = Task::new("cas-tr".into(), "Release train vNEXT".into());
        train.task_type = TaskType::Epic;
        train.labels = vec!["release-train".into()];
        train.branch = Some("epic/release-train-vnext-cas-tr".into());
        train.deliverables.work_target = Some(WorkTarget { repo_selector: "project:freeze-test".into(), target_branch: "main".into() });
        store.add(&train).unwrap();
        for (id, labels) in [("cas-feat", vec![]), ("cas-fix", vec![REPAIR_LABEL.to_string()])] {
            let mut task = Task::new(id.into(), id.into());
            task.labels = labels;
            store.add(&task).unwrap();
            store.add_dependency(&Dependency::new(id.into(), "cas-tr".into(), DependencyType::ParentChild)).unwrap();
        }
        (temp, root)
    }

    /// The 4.2.1 incident: merges kept landing after the release ask. Frozen,
    /// a feature is refused and a repair is refused until the train is red,
    /// by receipt or by GitHub. A peer reads only the synced task row, and
    /// the freeze ends when its tip lands on main.
    #[cfg(unix)]
    #[test]
    fn frozen_train_refuses_features_and_admits_repairs_of_a_red_tip() {
        let mut env = TestEnvGuard::temp_home();
        let (temp, root) = fixture();
        let repo = temp.path();
        let lane = "epic/release-train-vnext-cas-tr";
        let gh = repo.join("gh-fixture");
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nprintf '[]\\n'\n");
        env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
        assert!(check_merge(&root, lane, Some("cas-feat")).is_ok(), "unfrozen");

        let receipt = freeze(&root, "cas-tr", "supervisor calm-koala", Some("4.2.1")).unwrap();
        assert!(receipt.starts_with("TRAIN FROZEN"), "{receipt}");
        // A second machine opening the same synced row sees the same gate.
        let peer = crate::store::open_task_store(&root).unwrap().get("cas-tr").unwrap();
        assert!(freeze_of(&peer).is_some());
        let refused = check_merge(&root, lane, Some("cas-feat")).unwrap_err();
        assert!(refused.contains("train frozen for release, parked for the next train"), "{refused}");
        assert!(check_merge(&root, lane, Some("cas-fix")).is_err(), "repair without a red");

        let tip = git(repo, &["rev-parse", lane]);
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nprintf '[{\"databaseId\":77,\"conclusion\":\"failure\"}]\\n'\n");
        check_merge(&root, lane, Some("cas-fix")).unwrap();
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nexit 1\n");
        let dir = crate::train_ci::directory(&root, "cas-tr").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.json"), serde_json::json!({
            "branch": lane, "red_since": "2026-11-07T19:00:00Z", "red_head": tip, "run_id": "88",
        }).to_string()).unwrap();
        check_merge(&root, lane, Some("cas-fix")).unwrap();
        assert!(check_merge(&root, lane, Some("cas-feat")).is_err(), "red admits repairs only");
        assert!(check_staffing(&root, "cas-feat").is_err());
        let notes = crate::store::open_task_store(&root).unwrap().get("cas-tr").unwrap().notes;
        assert!(notes.contains("FREEZE REPAIR: cas-fix"), "{notes}");

        git(repo, &["merge", "-q", "--no-ff", lane, "-m", "release"]);
        check_merge(&root, lane, Some("cas-feat")).unwrap();
        check_staffing(&root, "cas-feat").unwrap();
    }

    /// The operator lift clears the gate; a supervisor has no lift action.
    #[test]
    fn operator_lift_reopens_the_train() {
        let _env = TestEnvGuard::temp_home();
        let (_temp, root) = fixture();
        let lane = "epic/release-train-vnext-cas-tr";
        freeze(&root, "cas-tr", "operator", None).unwrap();
        assert!(check_merge(&root, lane, Some("cas-feat")).is_err());
        lift(&root, "cas-tr", "operator").unwrap();
        check_merge(&root, lane, Some("cas-feat")).unwrap();
    }

    /// ETA = now + median train suite + queue + publish minutes.
    #[test]
    fn eta_adds_measured_stage_medians() {
        let _env = TestEnvGuard::temp_home();
        let (_temp, root) = fixture();
        let store = crate::store::open_task_store(&root).unwrap();
        let train = store.get("cas-tr").unwrap();
        let now = Utc::now();
        assert_eq!(remaining_minutes(&root, &train, now), Err("train suite"));
        for (key, payload) in [
            ("a", cas_store::QualityPayload::CheckRun { check_id: "train_full_gate".into(), stage: cas_store::CheckStage::Lane, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(20.0) }),
            ("b", cas_store::QualityPayload::CheckRun { check_id: "train_full_gate".into(), stage: cas_store::CheckStage::Lane, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(30.0) }),
            ("c", cas_store::QualityPayload::CheckRun { check_id: "train_full_gate".into(), stage: cas_store::CheckStage::Lane, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(90.0) }),
            ("d", cas_store::QualityPayload::CheckRun { check_id: "merge_queue".into(), stage: cas_store::CheckStage::Queue, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(10.0) }),
            ("e", cas_store::QualityPayload::Release { commit_to_main_minutes: None, main_to_published_minutes: Some(5.0), ejections: 0, rescues: 0 }),
        ] {
            let mut event = cas_store::NewQualityEvent::new(key, payload);
            event.occurred_at = now - chrono::Duration::hours(1);
            cas_store::record_quality_event(&root, &event).unwrap();
        }
        assert_eq!(remaining_minutes(&root, &train, now), Ok(45.0));
    }
}
