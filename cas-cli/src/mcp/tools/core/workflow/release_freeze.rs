//! Release freeze (cas-2f4d): the operator's release request as train state.
//!
//! The freeze is one label on the release-train epic, so it reaches a peer
//! machine through ordinary task sync and that machine's merge gate refuses
//! too. While a train is frozen, Cassy refuses worktree_merge into it, new
//! staffing on it and CI retries armed against it. A `release-repair` task is
//! the one exception, and only while the train is red: a recorded
//! train-suite red, or a failed GitHub train run at the frozen tip. The freeze
//! ends when a published release covers its tip, or when the operator
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

/// The freeze recorded on an open release train, published or not. `freeze`
/// writes exactly one label; task updates only append labels, so the first
/// one is authoritative and a later appended label cannot move its tip.
pub(crate) fn freeze_of(train: &Task) -> Option<Freeze> {
    if !is_release_train(train) || train.is_terminal() {
        return None;
    }
    recorded(train)
}

/// The freeze label on any train, closed ones included: the cut history
/// auto-cut's cadence reads (cas-e060).
pub(crate) fn recorded(train: &Task) -> Option<Freeze> {
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

/// A freeze that still holds: no published release covers its tip. The
/// publication evidence is the friction-ledger `Release` event that
/// `release-published-receipt.sh` records after it verifies the published
/// assets: its version's tag must contain the frozen tip. Landing on the
/// integration branch does not end a freeze; the cut still has to publish.
pub(crate) fn active(root: &Path, train: &Task) -> Option<Freeze> {
    let freeze = freeze_of(train)?;
    let published = repo_of(root, train).is_some_and(|repo| {
        cas_store::quality_release_events(root).unwrap_or_default().iter().any(|event| {
            // The ledger stores the version as its `vX.Y.Z` tag name.
            event.release_version.as_deref().is_some_and(|tag| is_ancestor(&repo, &freeze.tip, &format!("refs/tags/{tag}")))
        })
    });
    (!published).then_some(freeze)
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
        "TRAIN FROZEN: train frozen for release, parked for the next train. {}; {subject}. \
         No merge was attempted. Only a `{REPAIR_LABEL}` task merges while the train is red. \
         The freeze ends when the release is published or the operator lifts it in Commander.",
        frozen_since(train, freeze),
    )
}

/// The train whose standing freeze holds merges into `target`, an effective
/// epic ref, among `tasks`.
pub(crate) fn frozen_target(root: &Path, tasks: &[Task], target: &str) -> Option<(Task, Freeze)> {
    tasks.iter().filter(|train| freeze_of(train).is_some()).find_map(|train| {
        let lane = train.effective_epic_branch()?;
        if crate::mcp::tools::core::task::repo_context::effective_epic_ref(root, lane) != target {
            return None;
        }
        let freeze = active(root, train)?;
        Some((train.clone(), freeze))
    })
}

/// "cas-tr has been frozen since 2:58 AM ET at tip 0123456789ab".
pub(crate) fn frozen_since(train: &Task, freeze: &Freeze) -> String {
    format!("{} has been frozen since {} at tip {}", train.id, eastern(freeze.at), short(&freeze.tip))
}

/// The merge gate: worktree_merge (before and under the target lock) and CI
/// retry arming. A repair admitted under a red is noted on the train once.
pub(crate) fn check_merge(root: &Path, target: &str, task_id: Option<&str>) -> Result<(), String> {
    let store = crate::store::open_task_store(root).map_err(|error| error.to_string())?;
    let tasks = store.list(None).map_err(|error| error.to_string())?;
    let Some((train, freeze)) = frozen_target(root, &tasks, target) else { return Ok(()) };
    let task = task_id.and_then(|id| store.get(id).ok());
    let Some(task) = task.filter(is_repair) else {
        let subject = task_id.map_or("this merge names no task".to_string(), |id| format!("{id} stays parked"));
        return Err(crate::gates::tag("G-100", refusal(&train, &freeze, &subject)));
    };
    let Some(red) = red_evidence(root, &train, &freeze) else {
        return Err(crate::gates::tag("G-100", refusal(
            &train,
            &freeze,
            &format!("{} is marked {REPAIR_LABEL}, but no red is recorded at or after the frozen tip", task.id),
        )));
    };
    let line = format!("FREEZE REPAIR: {} admitted into frozen {} under {red}", task.id, train.id);
    if !train.notes.contains(&line) {
        store.append_note(&train.id, &line).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The staffing gate: spawning, assigning or starting new work on a frozen
/// train. Work already under way may resume; repairs may always start.
pub(crate) fn check_staffing(root: &Path, task_id: &str) -> Result<()> {
    let store = crate::store::open_task_store(root)?;
    // An unknown id is the caller's own error to report, not a freeze refusal.
    let Ok(task) = store.get(task_id) else { return Ok(()) };
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
        Some(freeze) => anyhow::bail!(crate::gates::tag("G-06", refusal(&train, &freeze, &subject).replace(" No merge was attempted.", ""))),
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
    let median = |values: Vec<f64>| crate::cli::ledger::median(values.into_iter().rev().take(10).collect());
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
         The freeze ends when the release is published or the operator lifts it in Commander.",
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
    /// MCP release freeze: a live supervisor explicitly confirms the lift policy.
    pub(crate) fn train_freeze(&self, train_id: &str, reason: Option<&str>) -> Result<String> {
        let actor = self
            .resolve_live_supervisor_authority()
            .map_err(|_| anyhow::anyhow!("train_freeze requires a live registered supervisor"))?;
        let reason = reason
            .and_then(|reason| reason.trim().strip_prefix("release-freeze:"))
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .ok_or_else(|| anyhow::anyhow!(
                "TRAIN_FREEZE_CONFIRMATION_REQUIRED: train_freeze is a release freeze, not a temporary CI hold. Only the operator can lift it in Commander; a published release covering its tip also ends it. A supervisor cannot lift it. Confirm with reason=\"release-freeze: <reason>\". For a temporary CI wait, coordinate peers to defer merges; see CONTRIBUTING's Temporary CI merge hold follow-up. No freeze was applied by this call."
            ))?;
        freeze(&self.cas_root, train_id, &format!("supervisor {}", actor.name), Some(reason))
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

    #[test]
    fn mcp_release_freeze_requires_explicit_confirmation() {
        let _env = TestEnvGuard::temp_home();
        let (_temp, root) = fixture();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        let supervisor = cas_types::Agent::new_with_role(
            "freeze-supervisor".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        core.open_agent_store().unwrap().register(&supervisor).unwrap();
        core.set_agent_id_for_testing(supervisor.id);
        let store = crate::store::open_task_store(&root).unwrap();
        let before = serde_json::to_value(store.get("cas-tr").unwrap()).unwrap();

        for reason in [Some("Lane CI freeze until run 42582303864 finishes"), None, Some("release-freeze: ")] {
            let error = core.train_freeze("cas-tr", reason).expect_err("unconfirmed release freeze");
            eprintln!("{error}");
            assert_eq!(error.to_string().split(':').next(), Some("TRAIN_FREEZE_CONFIRMATION_REQUIRED"));
            assert_eq!(serde_json::to_value(store.get("cas-tr").unwrap()).unwrap(), before);
        }

        core.train_freeze("cas-tr", Some("release-freeze: cut the reviewed release")).unwrap();
        assert!(freeze_of(&store.get("cas-tr").unwrap()).is_some());
        lift(&root, "cas-tr", "operator").unwrap();
        assert!(freeze_of(&store.get("cas-tr").unwrap()).is_none());
    }

    /// The 4.2.1 incident: merges kept landing after the release ask. Frozen,
    /// a feature is refused and a repair is refused until the train is red,
    /// by receipt or by GitHub. A peer reads only the synced task row, and
    /// the freeze ends when a published release covers its tip.
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
        assert!(notes.contains("cas-fix"), "{notes}");

        // Landing on main does not end the freeze; only publication does.
        git(repo, &["merge", "-q", "--no-ff", lane, "-m", "release"]);
        assert!(check_merge(&root, lane, Some("cas-feat")).is_err(), "landed, not published");
        assert!(check_staffing(&root, "cas-feat").is_err(), "landed, not published");
        // The published-release receipt records a Release event whose tag holds the tip.
        git(repo, &["tag", "v9.9.9"]);
        let mut published = cas_store::NewQualityEvent::new(
            "release|9.9.9",
            cas_store::QualityPayload::Release { commit_to_main_minutes: None, main_to_published_minutes: None, ejections: 0, rescues: 0 },
        );
        published.release_version = Some("9.9.9".into());
        published.occurred_at = Utc::now();
        cas_store::record_quality_event(&root, &published).unwrap();
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
