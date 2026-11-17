//! Release freeze (cas-2f4d): the operator's release request as train state.
//!
//! The freeze is one label on the release-train epic, so it reaches a peer
//! machine through ordinary task sync and that machine's merge gate refuses
//! too. While a train is frozen, Cassy refuses worktree_merge into it, new
//! staffing on it and CI retries armed against it. A `release-repair` task is
//! the one exception, and only while the train is red: a recorded
//! train-suite red, a cut refusal recorded by `release.sh` (GH #2027), or a
//! failed GitHub train run at the frozen tip. The freeze ends when a
//! published release covers its tip, when the operator lifts it in Commander,
//! or when the train's live supervisor lifts it with an audited reason
//! (cas-444b). A freeze the operator applied in Commander stays the
//! operator's unless `release.supervisor_lifts_operator_freeze` grants it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::mcp::tools::core::task::release_train::{is_cut_train, is_release_train};
use crate::types::Task;

const FROZEN_LABEL: &str = "release-train-frozen:";
/// Marks a freeze Cassy or a supervisor applied (cas-444b). It syncs with the
/// train row. A freeze without it, from the operator or from before this
/// label existed, is the operator's.
const SUPERVISOR_LIFTABLE_LABEL: &str = "release-train-freeze-supervisor-liftable";
/// The structured lift decision on the train: the audit record and the tip
/// auto-cut must not re-freeze (cas-b523).
const LIFT_NOTE: &str = "RELEASE FREEZE LIFTED: ";
/// Marks a task that repairs a red train; it may merge into a frozen train.
pub(crate) const REPAIR_LABEL: &str = "release-repair";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Freeze {
    pub at: DateTime<Utc>,
    pub tip: String,
}

/// The freeze recorded on an open release train, published or not. `freeze`
/// writes exactly one label. The first valid label is authoritative if legacy
/// stored data contains multiple freeze labels.
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
/// at or after the frozen tip, a Cassy gate refused the cut of such a tip, or
/// GitHub's latest train run at that tip failed. Any one admits a repair; a
/// missing receipt never locks it out.
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
    if let Some(refusal) = crate::train_ci::release_red(root, &train.id)
        && refusal["branch"].as_str() == Some(lane)
        && let Some(gate) = refusal["gate"].as_str().filter(|gate| crate::train_ci::RELEASE_RED_GATES.contains(gate))
        && let Some(head) = refusal["head"].as_str()
        && is_ancestor(&repo, &freeze.tip, head)
    {
        return Some(format!("release cut refused {gate} at {}", short(head)));
    }
    let mut command = Command::new(crate::github_repo::gh_binary());
    command
        .args(["run", "list", "--workflow", "full-validation.yml", "--branch", lane])
        .args(["--commit", &freeze.tip, "--event", "workflow_dispatch", "--limit", "1"])
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
         The freeze ends on publication, an operator lift in Commander, or an audited supervisor lift.",
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
        // A repeat freeze by the operator takes ownership of a standing freeze.
        if actor.starts_with("operator") && !operator_frozen(&task) {
            task.labels.retain(|label| label != SUPERVISOR_LIFTABLE_LABEL);
            task.updated_at = Utc::now();
            store.update(&task)?;
            store.append_note(train_id, &format!("RELEASE FREEZE: {actor} took ownership of the standing freeze on {train_id}"))?;
        }
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
    task.labels.retain(|label| !label.starts_with(FROZEN_LABEL) && label != SUPERVISOR_LIFTABLE_LABEL);
    task.labels.push(format!("{FROZEN_LABEL}{}:{tip}", now.timestamp()));
    if !actor.starts_with("operator") {
        task.labels.push(SUPERVISOR_LIFTABLE_LABEL.to_string());
    }
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
         The freeze ends on publication, an operator lift in Commander, or an audited supervisor lift.",
        short(&freeze.tip),
        summary(root, &task, &freeze, now)
    ))
}

/// Whether the standing freeze is the operator's: theirs, or unmarked legacy.
fn operator_frozen(train: &Task) -> bool {
    !train.labels.iter().any(|label| label == SUPERVISOR_LIFTABLE_LABEL)
}

/// The most recent structured lift decision on the train.
pub(crate) fn last_lift(train: &Task) -> Option<Value> {
    train.notes.lines().rev().find_map(|line| serde_json::from_str(line.split_once(LIFT_NOTE)?.1).ok())
}

/// The lift that already reopened the train at `tip`, if any: auto-cut must
/// not re-freeze a lifted tip; a new merge moves the tip and re-arms it.
pub(crate) fn lifted_at(train: &Task, tip: &str) -> Option<Value> {
    last_lift(train).filter(|lift| lift["tip"] == tip || lift["frozen_tip"] == tip)
}

/// Both lifts clear the same freeze and write the same audit decision.
fn lift_with_reason(root: &Path, train_id: &str, actor: &str, reason: &str) -> Result<(String, Value)> {
    let store = crate::store::open_task_store(root)?;
    let mut task = train(store.as_ref(), train_id)?;
    let freeze = freeze_of(&task).with_context(|| format!("{train_id} is not frozen"))?;
    // The local lane tip is the one auto-cut reads.
    let tip = task
        .effective_epic_branch()
        .zip(repo_of(root, &task))
        .and_then(|(lane, repo)| crate::worktree::GitOperations::new(repo).resolve_commit(lane))
        .unwrap_or_else(|| freeze.tip.clone());
    let now = Utc::now();
    let decision = serde_json::json!({
        "actor": actor, "at": now.to_rfc3339(), "reason": reason, "train_id": train_id,
        "tip": tip, "frozen_tip": freeze.tip, "operator_freeze": operator_frozen(&task),
    });
    task.labels.retain(|label| !label.starts_with(FROZEN_LABEL) && label != SUPERVISOR_LIFTABLE_LABEL);
    task.updated_at = now;
    store.update(&task)?;
    store.append_note(train_id, &format!("[{}] ✅ DECISION {LIFT_NOTE}{decision}", now.format("%Y-%m-%d %H:%M")))?;
    let detail = format!(
        "Freeze lifted on {train_id} by {actor} at {} at tip {}: {reason}. Merges into the train resume; \
         auto-cut will not re-freeze this tip.",
        eastern(now),
        short(&tip)
    );
    Ok((detail, decision))
}

/// The operator's lift from Commander.
pub(crate) fn lift(root: &Path, train_id: &str, actor: &str) -> Result<String> {
    lift_with_reason(root, train_id, actor, "operator lift in Commander").map(|(detail, _)| detail)
}

impl crate::mcp::CasCore {
    /// MCP release freeze: a live supervisor explicitly confirms the release request.
    pub(crate) fn train_freeze(&self, train_id: &str, reason: Option<&str>) -> Result<String> {
        let actor = self
            .resolve_live_supervisor_authority()
            .map_err(|_| anyhow::anyhow!("train_freeze requires a live registered supervisor"))?;
        let reason = reason
            .and_then(|reason| reason.trim().strip_prefix("release-freeze:"))
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .ok_or_else(|| anyhow::anyhow!(
                "TRAIN_FREEZE_CONFIRMATION_REQUIRED: train_freeze is a release freeze, not a temporary CI hold. It ends on publication of its tip, an operator lift in Commander, or the train supervisor's audited decision=lift with a reason. Confirm with reason=\"release-freeze: <reason>\". For a temporary CI wait, coordinate peers to defer merges; see CONTRIBUTING's Temporary CI merge hold follow-up. No freeze was applied by this call."
            ))?;
        freeze(&self.cas_root, train_id, &format!("supervisor {}", actor.name), Some(reason))
    }

    /// MCP lift (cas-444b): the train's live supervisor reopens a frozen train
    /// with a required reason, audited on the train and sent to the operator.
    // gate[G-157 release-freeze-lift]: Train freeze lift authority and reason
    pub(crate) fn train_lift(&self, train_id: &str, reason: Option<&str>) -> Result<String> {
        let actor = self.resolve_live_supervisor_authority().map_err(|_| {
            anyhow::anyhow!(crate::gates::tag("G-157", "TRAIN_LIFT_SUPERVISOR_REQUIRED: train_freeze decision=lift requires a live registered supervisor"))
        })?;
        let reason = reason.map(str::trim).filter(|reason| !reason.is_empty()).ok_or_else(|| {
            anyhow::anyhow!(crate::gates::tag("G-157", "TRAIN_LIFT_REASON_REQUIRED: train_freeze decision=lift requires a nonempty reason. No freeze was lifted."))
        })?;
        let task = train(crate::store::open_task_store(&self.cas_root)?.as_ref(), train_id)?;
        // Another factory session's live supervisor owns this train's release.
        if let Some(owner) = task.epic_verification_owner.as_deref()
            && owner != actor.id
            && owner != actor.name
            && let Some(other) = self.open_agent_store()?.list(None)?.into_iter().find(|agent| {
                (agent.id == owner || agent.name == owner)
                    && agent.role == cas_types::AgentRole::Supervisor
                    && agent.is_alive()
                    && agent.factory_session != actor.factory_session
            })
        {
            anyhow::bail!(crate::gates::tag("G-157", format!(
                "TRAIN_LIFT_NOT_TRAIN_SUPERVISOR: {train_id} belongs to supervisor {} in another factory session; ask it to lift. No freeze was lifted.",
                other.name
            )));
        }
        let granted = crate::config::Config::load(&self.cas_root)?.release.unwrap_or_default().supervisor_lifts_operator_freeze;
        ensure!(
            !operator_frozen(&task) || granted,
            crate::gates::tag("G-157", format!(
                "TRAIN_LIFT_OPERATOR_FREEZE: the operator froze {train_id} in Commander, or the freeze predates supervisor lifts; only the operator lifts it there, \
                 unless they grant supervisor lifts with release.supervisor_lifts_operator_freeze. No freeze was lifted."
            ))
        );
        let (detail, decision) = lift_with_reason(&self.cas_root, train_id, &format!("supervisor {}", actor.name), reason)?;
        crate::supervisor_notices::enqueue_with_context(
            &self.cas_root,
            &crate::supervisor_notices::Recipient::operator(actor.factory_session.as_deref().unwrap_or(&actor.id)),
            "release_freeze_lifted",
            &detail,
            &format!("{train_id}:{}", decision["at"]),
            Some(decision),
        )?;
        Ok(detail)
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
    /// by receipt, by a recorded cut refusal or by GitHub. A peer reads only the synced task row, and
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

        freeze(&root, "cas-tr", "supervisor calm-koala", Some("4.2.1")).unwrap();
        // A second machine opening the same synced row sees the same gate.
        let peer = crate::store::open_task_store(&root).unwrap().get("cas-tr").unwrap();
        assert!(freeze_of(&peer).is_some());
        assert!(check_merge(&root, lane, Some("cas-feat")).is_err(), "feature while frozen");
        assert!(check_merge(&root, lane, Some("cas-fix")).is_err(), "repair without a red");

        let tip = git(repo, &["rev-parse", lane]);
        // GH #2027: release.sh refused the cut of the green frozen tip.
        let refusal = crate::train_ci::record_release_red(&root, "cas-tr", lane, &tip, "ZERO_WOULD_CLOSE").unwrap();
        check_merge(&root, lane, Some("cas-fix")).unwrap();
        assert!(check_merge(&root, lane, Some("cas-feat")).is_err(), "a cut refusal admits repairs only");
        std::fs::remove_file(refusal).unwrap();
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

    /// GH #1919: G-100 says to retarget parked work to the next train. That
    /// one proof_scope_fix update now moves the parent too, so the re-park
    /// start is not refused G-06 by the frozen train.
    #[tokio::test]
    async fn retarget_to_next_train_moves_parent_so_start_is_not_refused_gh_1919() {
        let _env = TestEnvGuard::temp_home();
        let (temp, root) = fixture();
        let next = "epic/release-train-vnext-cas-tr2";
        git(temp.path(), &["branch", next]);
        let store = crate::store::open_task_store(&root).unwrap();
        let mut train = store.get("cas-tr").unwrap();
        train.id = "cas-tr2".into();
        train.branch = Some(next.into());
        train.labels = vec!["release-train".into()];
        store.add(&train).unwrap();
        let mut parked = store.get("cas-feat").unwrap();
        parked.status = crate::types::TaskStatus::AwaitingMerge;
        parked.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:freeze-test".into(),
            target_branch: "epic/release-train-vnext-cas-tr".into(),
        });
        store.update(&parked).unwrap();
        freeze(&root, "cas-tr", "operator", None).unwrap();

        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        let supervisor = cas_types::Agent::new_with_role(
            "retarget-supervisor".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        core.open_agent_store().unwrap().register(&supervisor).unwrap();
        core.set_agent_id_for_testing(supervisor.id);
        let req = serde_json::from_value(serde_json::json!({"id": "cas-feat"})).unwrap();
        core.cas_task_update_with_delivery_mode(
            req, None, Some(next), true, Some("cas-tr froze; next train"), None, None, None, false,
        )
        .await
        .unwrap_or_else(|error| panic!("{}", error.message));

        assert_eq!(store.get_parent_epic("cas-feat").unwrap().map(|epic| epic.id).as_deref(), Some("cas-tr2"));
        check_staffing(&root, "cas-feat").unwrap();
    }

    /// The train's live supervisor, registered and calling the real factory tool.
    fn supervisor_service(root: &Path, id: &str, session: &str) -> crate::mcp::tools::service::CasService {
        let core = crate::mcp::CasCore::with_daemon(root.to_path_buf(), None, None);
        let mut supervisor = cas_types::Agent::new_with_role(id.into(), format!("{id}-name"), cas_types::AgentRole::Supervisor);
        supervisor.factory_session = Some(session.into());
        core.open_agent_store().unwrap().register(&supervisor).unwrap();
        core.set_agent_id_for_testing(supervisor.id);
        crate::mcp::tools::service::CasService::new(core, None)
    }

    /// The newest structured lift decision recorded on the train.
    fn lift_decision(train: &Task) -> Value {
        train.notes.lines().rev()
            .find_map(|line| serde_json::from_str(line.split_once("RELEASE FREEZE LIFTED: ")?.1).ok())
            .expect("audited lift decision on the train")
    }

    async fn lift_via_tool(
        service: &crate::mcp::tools::service::CasService,
        reason: Option<&str>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let request = serde_json::from_value(serde_json::json!({
            "action": "train_freeze", "id": "cas-tr", "decision": "lift", "reason": reason,
        }))
        .unwrap();
        service.factory(rmcp::handler::server::wrapper::Parameters(request)).await
    }

    /// cas-444b, the Nov 16 4:31 PM incident: auto-cut froze the train and
    /// held its deliveries until the operator lifted it. The train's supervisor
    /// now lifts it through the factory tool with a required reason, audited
    /// on the train and sent to the operator, and auto-cut does not re-freeze
    /// the lifted tip a minute later (cas-b523); a new merge re-arms it.
    #[cfg(unix)]
    #[tokio::test]
    async fn supervisor_lifts_an_auto_cut_freeze_with_an_audited_reason() {
        use super::super::release_auto_cut::{Outcome, attempt};
        let _env = TestEnvGuard::temp_home();
        let (temp, root) = fixture();
        let repo = temp.path();
        let lane = "epic/release-train-vnext-cas-tr";
        git(repo, &["checkout", "-q", lane]);
        for path in ["scripts/release.sh", "changelog.d/cas-x.md"] {
            std::fs::create_dir_all(repo.join(path).parent().unwrap()).unwrap();
            std::fs::write(repo.join(path), "x\n").unwrap();
        }
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "release script and fragment"]);
        let service = supervisor_service(&root, "sup-id", "release-session");
        let store = crate::store::open_task_store(&root).unwrap();
        let mut train = store.get("cas-tr").unwrap();
        train.epic_verification_owner = Some("sup-id".into());
        store.update(&train).unwrap();
        let tip = git(repo, &["rev-parse", lane]);
        let poll = |tip: &str| attempt(&root, &store.get("cas-tr").unwrap(), lane, tip, false, false, Utc::now()).unwrap();

        assert!(matches!(poll(&tip), Outcome::Handled));
        assert!(check_merge(&root, lane, Some("cas-feat")).is_err(), "auto-cut froze the train");
        let frozen = serde_json::to_value(store.get("cas-tr").unwrap()).unwrap();
        for reason in [None, Some("  ")] {
            let error = lift_via_tool(&service, reason).await.unwrap_err();
            assert!(error.message.starts_with("TRAIN_LIFT_REASON_REQUIRED"), "{}", error.message);
            assert_eq!(crate::gates::codes_in(&error.message), vec!["G-157"]);
            assert_eq!(serde_json::to_value(store.get("cas-tr").unwrap()).unwrap(), frozen);
        }

        let reason = "release.sh refused the cut (GH #2026); repair must merge";
        assert_ne!(lift_via_tool(&service, Some(reason)).await.unwrap().is_error, Some(true));
        check_merge(&root, lane, Some("cas-feat")).unwrap();
        let train = store.get("cas-tr").unwrap();
        let decision = lift_decision(&train);
        assert_eq!(decision["actor"], "supervisor sup-id-name");
        assert_eq!(decision["reason"], reason);
        assert_eq!(decision["tip"], tip.as_str());
        let notices = crate::store::open_supervisor_queue_store(&root).unwrap().peek("operator", 10).unwrap();
        assert_eq!(notices.len(), 1);
        let notice: Value = serde_json::from_str(&notices[0].payload).unwrap();
        assert_eq!(notice["kind"], "release_freeze_lifted");
        assert_eq!(notice["context"], decision);

        assert!(matches!(poll(&tip), Outcome::Declined(_)));
        assert!(freeze_of(&store.get("cas-tr").unwrap()).is_none(), "auto-cut must not re-freeze the lifted tip");
        git(repo, &["commit", "-q", "--allow-empty", "-m", "next delivery"]);
        assert!(matches!(poll(&git(repo, &["rev-parse", lane])), Outcome::Handled));
        assert!(freeze_of(&store.get("cas-tr").unwrap()).is_some(), "a new tip re-arms auto-cut");
    }

    /// cas-444b: a freeze that is the operator's stays theirs. That is a
    /// Commander freeze, a legacy freeze with no ownership record, and a
    /// Cassy freeze the operator froze again. The train's supervisor is
    /// refused, as is a live supervisor of another factory session, until the
    /// operator grants supervisor lifts.
    #[tokio::test]
    async fn supervisor_cannot_lift_an_operator_freeze_without_the_operator_grant() {
        let _env = TestEnvGuard::temp_home();
        let (_temp, root) = fixture();
        let store = crate::store::open_task_store(&root).unwrap();
        let mut train = store.get("cas-tr").unwrap();
        train.epic_verification_owner = Some("sup-id".into());
        store.update(&train).unwrap();
        let foreign = supervisor_service(&root, "other-sup", "other-session");
        let service = supervisor_service(&root, "sup-id", "release-session");
        let legacy = || {
            freeze(&root, "cas-tr", "Cassy auto-cut", None).unwrap();
            let mut train = store.get("cas-tr").unwrap();
            let label = train.labels.iter().find(|label| label.starts_with(FROZEN_LABEL)).unwrap().clone();
            train.labels = vec!["release-train".into(), label];
            store.update(&train).unwrap();
        };
        let commander = || { freeze(&root, "cas-tr", "operator via Commander", None).unwrap(); };
        let refrozen = || {
            freeze(&root, "cas-tr", "Cassy auto-cut", None).unwrap();
            freeze(&root, "cas-tr", "operator via Commander", None).unwrap();
        };
        let setups: [&dyn Fn(); 3] = [&commander, &legacy, &refrozen];

        for setup in setups {
            setup();
            let frozen = serde_json::to_value(store.get("cas-tr").unwrap()).unwrap();
            for (caller, code) in [(&foreign, "TRAIN_LIFT_NOT_TRAIN_SUPERVISOR"), (&service, "TRAIN_LIFT_OPERATOR_FREEZE")] {
                let error = lift_via_tool(caller, Some("cut refused")).await.unwrap_err();
                assert!(error.message.starts_with(code), "{}", error.message);
                assert_eq!(crate::gates::codes_in(&error.message), vec!["G-157"]);
                assert_eq!(serde_json::to_value(store.get("cas-tr").unwrap()).unwrap(), frozen);
            }
            lift(&root, "cas-tr", "operator via Commander").unwrap();
        }

        legacy();
        let mut config = crate::config::Config::load(&root).unwrap();
        config.set("release.supervisor_lifts_operator_freeze", "true").unwrap();
        config.save(&root).unwrap();
        assert_ne!(lift_via_tool(&service, Some("cut refused")).await.unwrap().is_error, Some(true));
        assert!(freeze_of(&store.get("cas-tr").unwrap()).is_none());
        assert_eq!(lift_decision(&store.get("cas-tr").unwrap())["operator_freeze"], true);
    }

    /// The operator's Commander lift clears the same gate.
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
            ("a", cas_store::QualityPayload::CheckRun { review: None, check_id: "train_full_gate".into(), stage: cas_store::CheckStage::Lane, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(20.0) }),
            ("b", cas_store::QualityPayload::CheckRun { review: None, check_id: "train_full_gate".into(), stage: cas_store::CheckStage::Lane, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(30.0) }),
            ("c", cas_store::QualityPayload::CheckRun { review: None, check_id: "train_full_gate".into(), stage: cas_store::CheckStage::Lane, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(90.0) }),
            ("d", cas_store::QualityPayload::CheckRun { review: None, check_id: "merge_queue".into(), stage: cas_store::CheckStage::Queue, outcome: cas_store::CheckOutcome::Pass, sha: None, minutes: Some(10.0) }),
            ("e", cas_store::QualityPayload::Release { commit_to_main_minutes: None, main_to_published_minutes: Some(5.0), ejections: 0, rescues: 0 }),
        ] {
            let mut event = cas_store::NewQualityEvent::new(key, payload);
            event.occurred_at = now - chrono::Duration::hours(1);
            cas_store::record_quality_event(&root, &event).unwrap();
        }
        assert_eq!(remaining_minutes(&root, &train, now), Ok(45.0));
    }
}
