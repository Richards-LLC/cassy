//! cas-f823: an open release train takes main's new commits on its own.
//!
//! After a release lands, main holds the old train plus the release commit,
//! so the next train is a small merge behind. The daemon's CI poll merges
//! origin's integration branch into every open, unfrozen train lane and
//! publishes it, then tells the workers whose deliveries ride that lane. A
//! conflict changes nothing: one sync task naming the files is filed under the
//! train and the supervisor is told, instead of a spawn discovering it later
//! as EPIC BASE REFRESH REFUSED.

use std::path::Path;

use anyhow::{Context, Result};
use cas_types::{Dependency, DependencyType, Priority, Task, TaskRisk, TaskStatus, TaskType};

use crate::mcp::tools::core::task::release_train::{is_cut_train, is_release_train};
use crate::mcp::tools::core::workflow::release_freeze;
use crate::supervisor_notices::{self, Recipient};
use crate::worktree::{GitError, GitOperations};

/// Marks the one open sync task for a train: `main-sync:<train id>`.
const LABEL: &str = "main-sync:";
/// The notice kind for both outcomes.
const NOTICE: &str = "train_main_sync";

/// Merge the integration branch into each open train that lacks it; returns
/// how many trains moved.
pub(crate) fn sync_open_trains(cas_dir: &Path) -> Result<usize> {
    let tasks = crate::store::open_task_store(cas_dir)?;
    let all = tasks.list(None)?;
    let mut synced = 0;
    for train in all.iter().filter(|task| {
        is_release_train(task) && !task.is_terminal() && !is_cut_train(task)
    }) {
        let Some(lane) = train.effective_epic_branch() else { continue };
        let has_sync_task = all.iter().any(|task| {
            !task.is_terminal() && task.labels.iter().any(|label| *label == format!("{LABEL}{}", train.id))
        });
        // A frozen tip carries a receipt a merge would void; its freeze ends
        // at the cut, and the next train takes main then.
        if has_sync_task || release_freeze::freeze_of(train).is_some() {
            continue;
        }
        match sync_one(cas_dir, &*tasks, &all, train, lane) {
            Ok(moved) => synced += usize::from(moved),
            Err(error) => tracing::warn!(train = %train.id, %error, "train main sync deferred"),
        }
    }
    Ok(synced)
}

fn sync_one(
    cas_dir: &Path,
    tasks: &dyn cas_store::TaskStore,
    all: &[Task],
    train: &Task,
    lane: &str,
) -> Result<bool> {
    let repo = crate::train_guardrails::train_repo_root(cas_dir, train)?;
    let integration = train
        .deliverables
        .work_target
        .as_ref()
        .map(|target| target.target_branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
        .or_else(|| {
            crate::mcp::tools::core::task::repo_context::integration_branch(cas_dir).map(|(_, branch)| branch)
        })
        .context("no integration branch")?;
    let git = GitOperations::new(repo.clone());
    // No origin, or origin without the integration branch: nothing landed.
    if git.fetch_merge_target(&integration)?.is_none() {
        return Ok(false);
    }
    let main = format!("origin/{integration}");
    if git.resolve_commit(lane).is_none() {
        return Ok(false);
    }
    git.fetch_merge_target(lane)?;
    // Only a lane that matches origin (or can fast-forward to it) is merged,
    // so the push below is always a fast-forward.
    if git.resolve_commit(&format!("origin/{lane}")).is_some() {
        use crate::worktree::git::TargetReconcile::{AlreadyCurrent, FastForwarded};
        if !matches!(git.reconcile_target_with_fetched_origin(lane), AlreadyCurrent { .. } | FastForwarded { .. }) {
            return Ok(false);
        }
    }
    let Some(main_sha) = git.resolve_commit(&main) else { return Ok(false) };
    if git
        .commits_behind(lane, &main)
        .is_ok_and(|behind| behind == 0)
    {
        return Ok(false);
    }
    let remote = git.resolve_commit(&format!("origin/{lane}")).is_some();
    let mut push = |new_tip: &str, _old: &str| -> crate::worktree::git::Result<String> {
        if remote {
            let output = std::process::Command::new("git")
                .arg("-C").arg(&repo)
                .args(["push", "-q", "origin", &format!("{new_tip}:refs/heads/{lane}")])
                .output()?;
            if !output.status.success() {
                return Err(GitError::CommandFailed(String::from_utf8_lossy(&output.stderr).trim().to_string()));
            }
        }
        Ok(new_tip.to_string())
    };
    match git.merge_branch_with_publication(lane, &main, &mut push) {
        Ok(Some(tip)) => {
            notify_deliveries(cas_dir, tasks, train, lane, &integration, &main_sha, &tip);
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(GitError::MergeConflictPaths(files)) => {
            file_sync_task(cas_dir, tasks, all, train, lane, &integration, &main_sha, &files)?;
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

/// Tell each worker whose unfinished delivery rides the lane that it moved.
fn notify_deliveries(
    cas_dir: &Path,
    tasks: &dyn cas_store::TaskStore,
    train: &Task,
    lane: &str,
    integration: &str,
    main_sha: &str,
    tip: &str,
) {
    let Ok(children) = tasks.get_subtasks(&train.id) else { return };
    let Ok(agents) = crate::store::open_agent_store(cas_dir).map_err(anyhow::Error::from).and_then(|s| Ok(s.list(None)?)) else {
        return;
    };
    let detail = format!(
        "{lane} took {integration} ({}) in a merge now at {}. Before your next push, run `git fetch origin && git merge origin/{lane}`.",
        &main_sha[..main_sha.len().min(12)],
        &tip[..tip.len().min(12)],
    );
    for child in children.iter().filter(|child| {
        matches!(child.status, TaskStatus::Open | TaskStatus::InProgress | TaskStatus::AwaitingMerge)
    }) {
        let worker = child.assignee.as_deref().and_then(|assignee| {
            agents.iter().filter(|a| a.id == assignee || a.name == assignee).max_by_key(|a| a.registered_at)
        });
        if let Some(worker) = worker
            && let Err(error) = supervisor_notices::enqueue(cas_dir, &Recipient::agent(worker), NOTICE, &detail, tip)
        {
            tracing::warn!(task = %child.id, %error, "train main sync notice not queued");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn file_sync_task(
    cas_dir: &Path,
    tasks: &dyn cas_store::TaskStore,
    _all: &[Task],
    train: &Task,
    lane: &str,
    integration: &str,
    main_sha: &str,
    files: &[String],
) -> Result<()> {
    let id = tasks.generate_id()?;
    let mut task = Task::new(id.clone(), format!("Sync {lane} with {integration}: merge conflicts in {} file(s)", files.len()));
    task.task_type = TaskType::Chore;
    task.priority = Priority::HIGH;
    task.risk = vec![TaskRisk::None];
    task.labels = vec![format!("{LABEL}{}", train.id)];
    task.description = format!(
        "Merging {integration} ({main_sha}) into {lane} conflicts in:\n{}\n\nNothing changed on {lane}. \
         On a checkout of {lane}: `git fetch origin && git merge origin/{integration}`, resolve those files, \
         run the touched tests, and push {lane}.",
        files.iter().map(|file| format!("- {file}")).collect::<Vec<_>>().join("\n"),
    );
    task.acceptance_criteria = format!("{lane} contains origin/{integration}; the merge is pushed.");
    tasks.add(&task)?;
    tasks.add_dependency(&Dependency::new(id.clone(), train.id.clone(), DependencyType::ParentChild))?;
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let recipient = supervisor_notices::resolve(&agents, train.epic_verification_owner.as_deref(), None);
    supervisor_notices::enqueue(
        cas_dir,
        &recipient,
        NOTICE,
        &format!("{lane} cannot take {integration} ({main_sha}): conflicts in {}. Sync task {id} is filed under {}; staff it before the next cut.", files.join(", "), train.id),
        &format!("{}:{main_sha}", train.id),
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "train_main_sync_tests.rs"]
mod tests;
