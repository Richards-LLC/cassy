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
use cas_types::{Task, TaskStatus, TaskType};
use chrono::{DateTime, Utc};

use crate::mcp::CasCore;
use crate::supervisor_notices::{self, Recipient};

/// The release-train epic's label (cas-1896 owns train creation).
pub(crate) const RELEASE_TRAIN_LABEL: &str = "release-train";
const DROP_NOTE: &str = "Train drop:";
const REENQUEUED: &str = "re-enqueued PR #";

/// The train branch of a release-train epic.
pub(crate) fn train_lane(epic: &Task) -> Option<&str> {
    (epic.task_type == TaskType::Epic && epic.labels.iter().any(|l| l == RELEASE_TRAIN_LABEL))
        .then(|| epic.effective_epic_branch())
        .flatten()
}

/// The train's pull request in the merge queue.
pub(crate) trait TrainQueue {
    fn open_pr(&self, branch: &str) -> Result<Option<u64>>;
    fn enqueue(&self, pr: u64) -> Result<()>;
}

pub(crate) struct GhTrainQueue(pub PathBuf);

impl GhTrainQueue {
    fn gh(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("gh")
            .args(args)
            .current_dir(&self.0)
            .output()
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

#[derive(Debug)]
pub(crate) struct DropReceipt {
    pub task_id: String,
    pub train: String,
    pub merge: String,
    pub revert: String,
    pub pr: Option<u64>,
    pub worker: Option<String>,
}

impl DropReceipt {
    pub(crate) fn render(&self) -> String {
        let queue = match self.pr {
            Some(pr) => format!("Train PR #{pr} re-enqueued."),
            None => "No train PR is open; nothing to re-enqueue.".into(),
        };
        let worker = match &self.worker {
            Some(worker) => format!("Returned to {worker}."),
            None => "Reopened with no live assignee; assign it.".into(),
        };
        format!(
            "Dropped {} off {}: merge {} reverted as {}.\n{worker}\n{queue}",
            self.task_id, self.train, self.merge, self.revert
        )
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

/// Drop one child off its release train: revert, return, re-enqueue.
pub(crate) async fn drop_child(
    core: &CasCore,
    task_id: &str,
    reason: &str,
    queue: &dyn TrainQueue,
) -> Result<DropReceipt> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("train drop requires a nonempty --reason naming the failure");
    }
    let actor = core
        .resolve_live_supervisor_authority()
        .map_err(|_| anyhow::anyhow!("train drop requires a live registered supervisor"))?;
    let tasks = core.open_task_store().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let task = tasks.get(task_id)?;
    let epic = tasks
        .get_parent_epic(task_id)?
        .context("the task is not on a release train")?;
    let train = train_lane(&epic)
        .with_context(|| format!("{} is not a release-train epic", epic.id))?
        .to_string();

    // Rule-176(5): the drop that re-enqueued the train was its one repair cycle.
    if epic.notes.lines().any(|line| line.contains(DROP_NOTE) && line.contains(REENQUEUED)) {
        let detail = format!(
            "P0: release train {train} ({}) failed again after its one repair cycle (rule-176(5)). \
             A second drop was requested for {task_id}: {reason}. Nothing was reverted; you decide.",
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

    let root = match &epic.deliverables.work_target {
        Some(target) => {
            crate::mcp::tools::core::task::repo_context::resolve_repo_context(&core.cas_root, target)
                .map_err(anyhow::Error::msg)?
                .repo_root
        }
        None => core.cas_root.parent().context("project repository missing")?.to_path_buf(),
    };
    let record = cas_store::observed_delivery_merges_into(&core.cas_root, &train)?
        .into_iter()
        .rfind(|merge| merge.task_id == task_id)
        .with_context(|| format!("no recorded merge of {task_id} into {train}"))?;
    let merge = introducing_merge(&root, &record.merge_commit_sha, &record.source_commit_sha)?;
    if !is_ancestor(&root, &merge, &train) {
        bail!("merge {merge} is no longer on {train}");
    }
    let message = format!(
        "Revert {task_id} off the release train\n\n{reason}\n\nThis reverts merge {merge}."
    );
    let revert = crate::worktree::GitOperations::new(root.clone())
        .revert_merge_via_temp_worktree(&train, &merge, &message)?
        .context("the revert produced no commit")?;
    if git(&root, &["remote", "get-url", "origin"]).is_ok() {
        git(&root, &["push", "-q", "origin", &format!("{revert}:refs/heads/{train}")])
            .with_context(|| format!("revert {revert} is on local {train} but was not pushed"))?;
    }

    use rmcp::handler::server::wrapper::Parameters;
    core.cas_task_reopen(Parameters(crate::mcp::tools::TaskReopenRequest {
        id: task_id.to_string(),
        reason: Some(format!("dropped off the release train: {reason}")),
    }))
    .await
    .map_err(|error| anyhow::anyhow!("revert {revert} landed but reopen failed: {}", error.message))?;
    let now = Utc::now().format("%Y-%m-%d %H:%M");
    let instructions = format!(
        "{task_id} was dropped off the release train {train}: {reason}. Revert {revert} undid merge \
         {merge}. Start from the {train} tip, run `git revert {revert}` to restore your change, fix \
         it, then park again with merge_request=true."
    );
    tasks.append_trusted_note(task_id, &format!("[{now}] {instructions}"))?;
    let agents = crate::store::open_agent_store(&core.cas_root)?.list(None)?;
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
        &format!("[{now}] {DROP_NOTE} {task_id} merge {merge} reverted as {revert}; {outcome}"),
    )?;
    let pr = enqueued.with_context(|| format!("revert {revert} landed; re-enqueue the train PR by hand"))?;
    Ok(DropReceipt {
        task_id: task_id.to_string(),
        train,
        merge,
        revert,
        pr,
        worker: worker.map(|worker| worker.name.clone()),
    })
}

/// Send each due "train ready to cut" notice once. `queued` holds branches
/// whose PR is in the merge queue now; a train in the queue is being cut.
pub(crate) fn notify_train_cuts(
    cas_dir: &Path,
    session: &str,
    queued: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> Result<usize> {
    let max_age_hours = crate::config::Config::load(cas_dir)?.factory().release_train_max_age_hours;
    let tasks = crate::store::open_task_store(cas_dir)?;
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let mut sent = 0;
    for epic in tasks.list(None)?.into_iter().filter(|task| !task.is_terminal()) {
        let Some(train) = train_lane(&epic).map(str::to_string) else { continue };
        if queued.contains(&train) {
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
        let age = now - first.created_at;
        let cut = format!("`git switch {train} && scripts/release.sh` (cas-cut-release)");
        let (kind, occurrence, detail) = if in_flight == 0 {
            (
                "train_ready",
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
                "train_max_age",
                first.created_at.to_rfc3339(),
                format!(
                    "Release train {train} ({}) reached its {max_age_hours} h max age: {} reviewed \
                     fixes merged, first {} min ago; {in_flight} still in flight ride the next train. \
                     Cut it: {cut}. Cassy never cuts on its own.",
                    epic.id,
                    merged.len(),
                    age.num_minutes()
                ),
            )
        } else {
            continue;
        };
        let recipient =
            supervisor_notices::resolve(&agents, epic.epic_verification_owner.as_deref(), Some(session));
        supervisor_notices::enqueue(cas_dir, &recipient, kind, &detail, &format!("{}:{occurrence}", epic.id))?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
#[path = "train_guardrails_tests.rs"]
mod tests;
