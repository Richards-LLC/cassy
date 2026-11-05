//! One integration per release: fixes merge to the train, the train goes
//! through the gate once.
//!
//! A task created with no epic and no explicit target joins the repository's
//! open release-train epic (cut from the integration branch) instead of
//! targeting that branch directly, so its delivery merges locally into the
//! train lane. A train whose content has landed on the integration branch is
//! closed here, and the next one opens lazily on the next create.

use std::borrow::Cow;
use std::path::Path;

use rmcp::ErrorData as McpError;

use crate::mcp::server::CasCore;
use crate::mcp::tools::core::task::lifecycle::close_ops::{
    BranchContentDirection, branch_content_direction,
};
use crate::mcp::tools::types::TaskCreateRequest;
use crate::types::{Task, TaskStatus, TaskType};

pub(crate) const RELEASE_TRAIN_LABEL: &str = "release-train";
const RELEASE_TRAIN_TITLE: &str = "Release train vNEXT";

pub(crate) fn is_release_train(task: &Task) -> bool {
    task.task_type == TaskType::Epic && task.labels.iter().any(|l| l == RELEASE_TRAIN_LABEL)
}

/// Open trains bound for `integration`, oldest first. A train cut for another
/// integration branch (the setting changed) is left for its owner to land.
pub(crate) fn open_trains(store: &dyn cas_store::TaskStore, integration: &str) -> Vec<Task> {
    let mut trains: Vec<Task> = store
        .list(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|task| {
            is_release_train(task)
                && !task.is_terminal()
                && task
                    .deliverables
                    .work_target
                    .as_ref()
                    .is_some_and(|target| target.target_branch == integration)
        })
        .collect();
    trains.sort_by_key(|task| task.created_at);
    trains
}

fn ref_exists(repo: &Path, refname: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{refname}^{{commit}}")])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()
        .is_ok_and(|status| status.success())
}

/// A train has landed when it carried at least one delivered child, none is
/// still in flight, and every copy of its lane is on the integration branch
/// (by ancestry, or byte-identical content after a squash).
fn landed(store: &dyn cas_store::TaskStore, repo: &Path, train: &Task, integration: &str) -> bool {
    let children = store.get_subtasks(&train.id).unwrap_or_default();
    if children.iter().any(|child| !child.is_terminal())
        || !children.iter().any(|child| child.status == TaskStatus::Closed)
    {
        return false;
    }
    let Some(lane) = train.effective_epic_branch() else {
        return false;
    };
    let lane_refs: Vec<String> = [lane.to_string(), format!("origin/{lane}")]
        .into_iter()
        .filter(|candidate| ref_exists(repo, candidate))
        .collect();
    let targets: Vec<String> = [integration.to_string(), format!("origin/{integration}")]
        .into_iter()
        .filter(|candidate| ref_exists(repo, candidate))
        .collect();
    !lane_refs.is_empty()
        && lane_refs.iter().all(|lane_ref| {
            targets.iter().any(|target| is_ancestor(repo, lane_ref, target))
                || matches!(
                    branch_content_direction(repo, lane_ref, integration),
                    BranchContentDirection::ContentPresent { .. }
                )
        })
}

impl CasCore {
    /// The release train a new task without an epic or explicit target joins,
    /// with a one-line receipt. `None` keeps the per-task integration target:
    /// the setting is off, the store is not in Git, or no integration branch
    /// resolves. A train that cannot be opened degrades to that target too,
    /// with the reason in the receipt.
    pub(crate) async fn release_train_for_new_task(
        &self,
        store: &dyn cas_store::TaskStore,
    ) -> (Option<String>, String) {
        let Some((repo, integration)) =
            super::repo_context::integration_branch(&self.cas_root)
        else {
            return (None, String::new());
        };
        if !crate::config::Config::load(&repo.join(".cas"))
            .map(|config| config.factory().release_train)
            .unwrap_or(true)
        {
            return (None, String::new());
        }
        let mut receipt = String::new();
        for train in open_trains(store, &integration) {
            if !landed(store, &repo, &train, &integration) {
                return (
                    Some(train.id.clone()),
                    format!(
                        "{receipt}\n\nRELEASE TRAIN: joined {} ({}); its delivery merges into the train, which reaches `{integration}` once per release. Pass target_branch=`{integration}` only for a hotfix.",
                        train.id,
                        train.effective_epic_branch().unwrap_or(integration.as_str()),
                    ),
                );
            }
            let mut closed = train.clone();
            closed.status = TaskStatus::Closed;
            closed.terminal_outcome = Some(cas_types::TaskTerminalOutcome::Delivered);
            closed.closed_at = Some(chrono::Utc::now());
            closed.updated_at = chrono::Utc::now();
            closed.close_reason = Some(format!(
                "Release landed: train lane is on `{integration}`; the next train opens on the next create."
            ));
            if store.update(&closed).is_ok() {
                receipt.push_str(&format!(
                    "\n\nRELEASE TRAIN: {} landed on `{integration}` and is closed.",
                    train.id
                ));
            }
        }
        let request: TaskCreateRequest = match serde_json::from_value(serde_json::json!({
            "title": RELEASE_TRAIN_TITLE,
            "task_type": "epic",
            "priority": 1,
            "labels": RELEASE_TRAIN_LABEL,
            "description": format!(
                "Default target for tasks created without an epic. Children merge into this lane; the release goes to `{integration}` once."
            ),
        })) {
            Ok(request) => request,
            Err(error) => return (None, format!("{receipt}\n\nRELEASE TRAIN UNAVAILABLE: {error}")),
        };
        let repo_arg = repo.to_string_lossy().to_string();
        let created = Box::pin(self.cas_task_create_with_delivery_mode(
            request,
            Some(&repo_arg),
            Some(&integration),
            true,
            None,
        ))
        .await;
        if let Err(McpError { message, .. }) = created {
            let message: Cow<'_, str> = message;
            return (
                None,
                format!(
                    "{receipt}\n\nRELEASE TRAIN UNAVAILABLE: {message}. This task targets `{integration}` directly."
                ),
            );
        }
        match open_trains(store, &integration).into_iter().next() {
            Some(train) => (
                Some(train.id.clone()),
                format!(
                    "{receipt}\n\nRELEASE TRAIN: opened {} ({}) from `{integration}`; this task joins it.",
                    train.id,
                    train.effective_epic_branch().unwrap_or(integration.as_str()),
                ),
            ),
            None => (None, receipt),
        }
    }
}
