//! cas-dcc9: release-train guardrails.
//!
//! `cas factory train drop <task>` reverts one child's merge off the train
//! (a new revert commit, never a history rewrite), returns the task to its
//! worker and re-enqueues the train PR. Rule-176(5) bounds it: one repair
//! cycle per train; a second ejection goes to the operator as a P0.
//!
//! The daemon sends the supervisor one "train ready to cut" notice when the
//! train holds reviewed fixes and nothing is in flight, and one when the train
//! reaches its max age. Cassy never cuts a release on its own.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use cas_types::{Task, TaskStatus};
use chrono::{DateTime, Utc};

use crate::mcp::CasCore;
use crate::supervisor_notices::{self, Recipient};

const DROP_NOTE: &str = "Train drop:";
const REENQUEUED: &str = "re-enqueued PR #";

/// The train branch of a release-train epic.
pub(crate) fn train_lane(epic: &Task) -> Option<&str> {
    crate::mcp::tools::core::task::release_train::is_release_train(epic)
        .then(|| epic.effective_epic_branch())
        .flatten()
}

/// cas-5f67: a drop re-enqueued this open train and a dropped child has not
/// closed again, so its one repair cycle is still under way.
pub(crate) fn repair_cycle_open(train: &Task, tasks: &[Task]) -> bool {
    train_lane(train).is_some()
        && !train.is_terminal()
        && train.notes.lines().any(|line| {
            let Some(rest) = line.split_once(DROP_NOTE).map(|(_, rest)| rest) else { return false };
            line.contains(REENQUEUED)
                && rest.split(" reverted up to").next().unwrap_or_default().split(',').any(|id| {
                    tasks.iter().any(|task| task.id == id.trim() && !task.is_terminal())
                })
        })
}

/// The train's pull request in the merge queue.
pub(crate) trait TrainQueue {
    fn open_pr(&self, branch: &str) -> Result<Option<u64>>;
    fn enqueue(&self, pr: u64) -> Result<()>;
}

pub(crate) struct GhTrainQueue(pub PathBuf);

impl GhTrainQueue {
    fn gh(&self, args: &[&str]) -> Result<String> {
        let output = crate::gh_budget::output(
            Command::new("gh").args(args).current_dir(&self.0),
            std::time::Duration::from_secs(60),
        )
        .context("gh is unavailable")?;
        if !output.status.success() {
            bail!("gh {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

impl TrainQueue for GhTrainQueue {
    fn open_pr(&self, branch: &str) -> Result<Option<u64>> {
        let number = self.gh(&[
            "pr", "list", "--head", branch, "--state", "open", "--json", "number", "--jq",
            ".[0].number // empty",
        ])?;
        Ok(number.parse().ok())
    }
    fn enqueue(&self, pr: u64) -> Result<()> {
        self.gh(&["pr", "merge", &pr.to_string(), "--auto", "--merge"]).map(drop)
    }
}

/// One child reverted off the train.
#[derive(Debug)]
pub(crate) struct Dropped {
    pub task_id: String,
    pub merge: String,
    pub revert: String,
    pub worker: Option<String>,
}

#[derive(Debug)]
pub(crate) struct DropReceipt {
    pub train: String,
    pub dropped: Vec<Dropped>,
    pub pr: Option<u64>,
}

impl DropReceipt {
    pub(crate) fn render(&self) -> String {
        let mut lines: Vec<String> = self
            .dropped
            .iter()
            .map(|d| {
                let worker = match &d.worker {
                    Some(worker) => format!("returned to {worker}"),
                    None => "reopened with no live assignee; assign it".into(),
                };
                format!("Dropped {}: merge {} reverted as {}; {worker}.", d.task_id, d.merge, d.revert)
            })
            .collect();
        lines.push(match self.pr {
            Some(pr) => format!("Train {} PR #{pr} re-enqueued once.", self.train),
            None => format!("No PR is open for {}; nothing to re-enqueue.", self.train),
        });
        lines.join("\n")
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> bool {
    git(root, &["merge-base", "--is-ancestor", ancestor, descendant]).is_ok()
}

/// The first-parent merge on the train that brought `source` in. The recorded
/// tip can be a generated-output commit just after the merge itself.
fn introducing_merge(root: &Path, recorded: &str, source: &str) -> Result<String> {
    let mut commit = recorded.to_string();
    for _ in 0..8 {
        let line = git(root, &["rev-list", "--parents", "-n", "1", &commit])?;
        let parents: Vec<&str> = line.split_whitespace().collect();
        if parents.len() == 3
            && is_ancestor(root, source, parents[2])
            && !is_ancestor(root, source, parents[1])
        {
            return Ok(commit);
        }
        commit = parents.get(1).context("no merge commit brought the task in")?.to_string();
    }
    bail!("no merge commit near {recorded} brought in {source}")
}

/// The checkout holding a train's repository.
pub(crate) fn train_repo_root(cas_root: &Path, epic: &Task) -> Result<PathBuf> {
    Ok(match &epic.deliverables.work_target {
        Some(target) => {
            crate::mcp::tools::core::task::repo_context::resolve_repo_context(cas_root, target)
                .map_err(anyhow::Error::msg)?
                .repo_root
        }
        None => cas_root.parent().context("project repository missing")?.to_path_buf(),
    })
}

/// Drop children off their release train in one batch: revert every one,
/// return each to its worker, re-enqueue the train once (rule-176(5)).
pub(crate) async fn drop_children(
    core: &CasCore,
    task_ids: &[String],
    reason: &str,
    queue: &dyn TrainQueue,
) -> Result<DropReceipt> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("train drop requires a nonempty --reason naming the failure");
    }
    let mut seen = BTreeSet::new();
    let task_ids: Vec<&String> = task_ids.iter().filter(|id| seen.insert(id.as_str())).collect();
    if task_ids.is_empty() {
        bail!("train drop requires at least one task");
    }
    let actor = core
        .resolve_live_supervisor_authority()
        .map_err(|_| anyhow::anyhow!("train drop requires a live registered supervisor"))?;
    let tasks = core.open_task_store().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut children = Vec::new();
    let mut epic: Option<Task> = None;
    for task_id in &task_ids {
        let task = tasks.get(task_id)?;
        let parent = tasks
            .get_parent_epic(task_id)?
            .with_context(|| format!("{task_id} is not on a release train"))?;
        if epic.as_ref().is_some_and(|epic| epic.id != parent.id) {
            bail!("{task_id} rides a different train; drop each train's children separately");
        }
        children.push(task);
        epic = Some(parent);
    }
    let epic = epic.expect("at least one task");
    let train = train_lane(&epic)
        .with_context(|| format!("{} is not a release-train epic", epic.id))?
        .to_string();
    let listed = task_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>().join(", ");

    // Rule-176(5): the drop that re-enqueued the train was its one repair cycle.
    if epic.notes.lines().any(|line| line.contains(DROP_NOTE) && line.contains(REENQUEUED)) {
        let detail = format!(
            "P0: release train {train} ({}) failed again after its one repair cycle (rule-176(5)). \
             A second drop was requested for {listed}: {reason}. Nothing was reverted; you decide.",
            epic.id
        );
        let session = actor.factory_session.as_deref().unwrap_or("supervisor-recovery");
        supervisor_notices::enqueue(
            &core.cas_root,
            &Recipient::operator(session),
            "train_second_ejection",
            &detail,
            &epic.id,
        )?;
        bail!("{detail} Reported to the operator.");
    }

    let root = train_repo_root(&core.cas_root, &epic)?;
    let records = cas_store::observed_delivery_merges_into(&core.cas_root, &train)?;
    let mut merges = Vec::new();
    for task in &children {
        let (position, record) = records
            .iter()
            .enumerate()
            .rfind(|(_, merge)| merge.task_id == task.id)
            .with_context(|| format!("no recorded merge of {} into {train}", task.id))?;
        let merge = introducing_merge(&root, &record.merge_commit_sha, &record.source_commit_sha)?;
        if !is_ancestor(&root, &merge, &train) {
            bail!("merge {merge} of {} is no longer on {train}", task.id);
        }
        merges.push((position, task, merge));
    }
    // Newest first, so a later child's revert never conflicts with an earlier one's.
    merges.sort_by(|a, b| b.0.cmp(&a.0));
    let plan: Vec<(String, String)> = merges
        .iter()
        .map(|(_, task, merge)| {
            (
                merge.clone(),
                format!("Revert {} off the release train\n\n{reason}\n\nThis reverts merge {merge}.", task.id),
            )
        })
        .collect();
    let tip = crate::worktree::GitOperations::new(root.clone())
        .revert_merges_via_temp_worktree(&train, &plan)
        .map_err(|error| {
            let error = error.to_string();
            match merges.iter().find(|(_, _, merge)| error.contains(merge.as_str())) {
                Some((_, task, _)) => anyhow::anyhow!("dropping {}: {error}", task.id),
                None => anyhow::anyhow!(error),
            }
        })?
        .context("the revert produced no commit")?;
    if git(&root, &["remote", "get-url", "origin"]).is_ok() {
        git(&root, &["push", "-q", "origin", &format!("{tip}:refs/heads/{train}")])
            .with_context(|| format!("reverts up to {tip} are on local {train} but were not pushed"))?;
    }
    // One revert commit per child, in plan order.
    let mut reverts: Vec<String> = git(&root, &["rev-list", "--first-parent", "-n", &plan.len().to_string(), &tip])?
        .lines()
        .map(str::to_string)
        .collect();
    reverts.reverse();

    use rmcp::handler::server::wrapper::Parameters;
    let agents = crate::store::open_agent_store(&core.cas_root)?.list(None)?;
    let now = Utc::now().format("%Y-%m-%d %H:%M");
    let mut dropped = Vec::new();
    for ((_, task, merge), revert) in merges.iter().zip(reverts) {
        core.cas_task_reopen(Parameters(crate::mcp::tools::TaskReopenRequest {
            id: task.id.clone(),
            reason: Some(format!("dropped off the release train: {reason}")),
        }))
        .await
        .map_err(|error| anyhow::anyhow!("revert {revert} landed but reopening {} failed: {}", task.id, error.message))?;
        let instructions = format!(
            "{} was dropped off the release train {train}: {reason}. Revert {revert} undid merge \
             {merge}. Start from the {train} tip, run `git revert {revert}` to restore your change, fix \
             it, then park again with merge_request=true.",
            task.id
        );
        tasks.append_trusted_note(&task.id, &format!("[{now}] {instructions}"))?;
        let worker = task.assignee.as_deref().and_then(|assignee| {
            agents
                .iter()
                .filter(|agent| agent.id == assignee || agent.name == assignee)
                .max_by_key(|agent| agent.registered_at)
        });
        if let Some(worker) = worker {
            supervisor_notices::enqueue(
                &core.cas_root,
                &Recipient::agent(worker),
                "train_drop",
                &instructions,
                &revert,
            )?;
        }
        dropped.push(Dropped {
            task_id: task.id.clone(),
            merge: merge.clone(),
            revert,
            worker: worker.map(|worker| worker.name.clone()),
        });
    }

    let enqueued = queue.open_pr(&train).and_then(|pr| match pr {
        Some(pr) => queue.enqueue(pr).map(|()| Some(pr)),
        None => Ok(None),
    });
    let outcome = match &enqueued {
        Ok(Some(pr)) => format!("{REENQUEUED}{pr}"),
        Ok(None) => "no train PR open to re-enqueue".into(),
        Err(error) => format!("re-enqueue failed: {error}"),
    };
    tasks.append_trusted_note(
        &epic.id,
        &format!("[{now}] {DROP_NOTE} {listed} reverted up to {tip}; {outcome}"),
    )?;
    let pr = enqueued.with_context(|| format!("reverts landed at {tip}; re-enqueue the train PR by hand"))?;
    Ok(DropReceipt { train, dropped, pr })
}

/// How the notice tells the supervisor to cut: the project's configured
/// release or promotion command, else `scripts/release.sh` when it exists.
fn cut_command(configured: Option<&str>, repo_root: Option<&Path>, train: &str) -> String {
    let script = repo_root
        .is_some_and(|root| root.join("scripts/release.sh").is_file())
        .then_some("scripts/release.sh");
    match configured.or(script) {
        Some(command) => format!("on {train}, run `{command}`"),
        None => format!(
            "run this project's release or promotion from {train} (set factory.release_command to name it here)"
        ),
    }
}

/// The current tip, when it holds the verdict `scripts/release.sh` demands
/// before it cuts: the train-tip full-set receipt `train_ci::observe` records
/// must be green at it. Missing, red, stale or unreadable all mean not green.
fn green_tip(cas_dir: &Path, epic: &Task, train: &str) -> Option<String> {
    let state = crate::train_ci::directory(cas_dir, &epic.id)
        .ok()
        .and_then(|dir| std::fs::read(dir.join("state.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let tip = train_repo_root(cas_dir, epic).ok().and_then(|root| git(&root, &["rev-parse", train]).ok());
    let (state, tip) = (state?, tip?);
    (state["outcome"] == "PASS" && state["head"].as_str() == Some(tip.as_str())).then_some(tip)
}

/// Send each due "train ready to cut" notice once. `queued` holds branches
/// whose PR is in the merge queue now; a train in the queue is being cut.
pub(crate) fn notify_train_cuts(
    cas_dir: &Path,
    queued: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> Result<usize> {
    let factory = crate::config::Config::load(cas_dir)?.factory();
    let max_age_hours = factory.release_train_max_age_hours;
    let tasks = crate::store::open_task_store(cas_dir)?;
    let mut sent = 0;
    for epic in tasks.list(None)?.into_iter().filter(|task| !task.is_terminal()) {
        let Some(train) = train_lane(&epic).map(str::to_string) else { continue };
        if queued.contains(&train) || crate::mcp::tools::core::task::release_train::is_cut_train(&epic) {
            continue;
        }
        let merged: Vec<_> = cas_store::observed_delivery_merges_into(cas_dir, &train)?
            .into_iter()
            .filter(|merge| tasks.get(&merge.task_id).is_ok_and(|t| t.status == TaskStatus::Closed))
            .collect();
        let (Some(first), Some(last)) = (merged.first(), merged.last()) else { continue };
        let in_flight = tasks
            .get_subtasks(&epic.id)?
            .iter()
            .filter(|child| matches!(child.status, TaskStatus::InProgress | TaskStatus::AwaitingMerge))
            .count();
        let Some(tip) = green_tip(cas_dir, &epic, &train) else { continue };
        let age = now - first.created_at;
        let cut = cut_command(
            factory.release_command.as_deref(),
            train_repo_root(cas_dir, &epic).ok().as_deref(),
            &train,
        );
        let (occurrence, detail) = if in_flight == 0 {
            (
                last.merge_commit_sha.clone(),
                format!(
                    "Release train {train} ({}) is ready to cut: {} reviewed fixes merged, none in \
                     progress, no queue run in flight. Cut it: {cut}. Cassy never cuts on its own.",
                    epic.id,
                    merged.len()
                ),
            )
        } else if age >= chrono::Duration::hours(i64::from(max_age_hours)) {
            (
                format!("max-age:{tip}"),
                format!(
                    "Release train {train} ({}) reached its {max_age_hours} h max age: {} reviewed \
                     fixes merged, first {} min ago; {in_flight} still in flight remain assigned to this train. \
                     Cut it: {cut}; retarget unfinished deliveries to the next train before merging them. Cassy never cuts on its own.",
                    epic.id,
                    merged.len(),
                    age.num_minutes()
                ),
            )
        } else {
            continue;
        };
        supervisor_notices::release_action(
            cas_dir,
            &epic,
            supervisor_notices::ReleaseNoticePhase::CutReady,
            &occurrence,
            &detail,
        )?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
#[path = "train_guardrails_tests.rs"]
mod tests;
