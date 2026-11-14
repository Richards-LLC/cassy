//! Keep parked deliveries mergeable as their train advances (cas-c05a).
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use cas_types::{Task, TaskStatus};

use super::{close_ops, supervisor_push};
use crate::mcp::tools::core::task::repo_context;
use crate::worktree::GitOperations;

// Polling unchanged delivery/target pairs must not repeat the Git check.
type Checked = HashMap<(PathBuf, String), (String, String)>;
static CHECKED: OnceLock<Mutex<Checked>> = OnceLock::new();

pub(crate) fn return_to_worker(
    root: &Path,
    expected: &Task,
    target: &str,
    target_tip: &str,
    paths: &[String],
    actor: &str,
) -> Result<String> {
    let store = crate::store::open_task_store_cached(root)?;
    let current = store.get(&expected.id)?;
    if current.status != TaskStatus::AwaitingMerge
        || current.deliverables.factory_branch_anchor != expected.deliverables.factory_branch_anchor
        || current.deliverables.parked_branch != expected.deliverables.parked_branch
        || current.deliverables.work_target != expected.deliverables.work_target
        || current.assignee != expected.assignee
    {
        return Ok("delivery changed during conflict preflight; no task transition attempted".into());
    }
    let source = expected.deliverables.parked_branch.as_deref().context("parked branch missing")?;
    let tip = expected.deliverables.factory_branch_anchor.as_deref().context("parked anchor missing")?;
    let paths = paths.iter().map(|path| GitOperations::visible_git_label(path)).collect::<Vec<_>>().join(", ");
    let reason = format!(
        "merge_conflicted: {} @ {tip} conflicts with {} @ {target_tip}; conflicting paths: {paths}. Merge the target into the delivery branch, resolve, and re-deliver.",
        GitOperations::visible_git_label(source), GitOperations::visible_git_label(target),
    );
    cas_store::request_changes_for_parked_delivery(root, &expected.id, actor, &reason)?;
    let _ = crate::store::publish_direct_task_write(root, &expected.id);
    let returned = store.get(&expected.id)?;
    let notice = supervisor_push::queue_merge_conflict_reopen_notice(root, &returned, tip, target_tip, &reason)
        .map_err(anyhow::Error::msg)?;
    Ok(format!("{reason}\nTask {} is Open again with its assignee kept; worker notice: {notice:?}.", expected.id))
}

/// Called after publication with its exact landed tip, and by the existing
/// daemon scan before it resumes any queued merge/CI admission.
pub(crate) fn recheck_target(root: &Path, repo: &Path, target: &str, target_tip: &str, actor: &str) -> Result<()> {
    if !target.starts_with("epic/") { return Ok(()); }
    let store = crate::store::open_task_store_cached(root)?;
    let parked = store.list(Some(TaskStatus::AwaitingMerge))?;
    let cache = CHECKED.get_or_init(Default::default);
    cache.lock().unwrap_or_else(|e| e.into_inner()).retain(|(owner, id), _| {
        owner != root || parked.iter().any(|task| &task.id == id)
    });
    let git = GitOperations::new(repo.to_path_buf());
    for task in parked {
        let parent = store.get_parent_epic(&task.id)?;
        let Some(work_target) = close_ops::effective_close_work_target(&task, parent.as_ref()) else { continue; };
        if work_target.target_branch != target || !repo_context::repo_answers_to(repo, &work_target.repo_selector) {
            continue;
        }
        let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref() else { continue; };
        let key = (root.to_path_buf(), task.id.clone());
        let proof = (target_tip.to_string(), anchor.to_string());
        if cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) == Some(&proof) { continue; }
        // A peer's parked source may not yet have reached our object database.
        if git.resolve_commit(anchor).is_none()
            && let Some(branch) = task.deliverables.parked_branch.as_deref()
        {
            anyhow::ensure!(
                crate::git_evidence::fetch_parent_branch_result(repo, branch) == crate::git_evidence::BoundedFetch::Succeeded,
                "could not fetch parked source {branch}",
            );
        }
        let paths = git.preflight_merge_conflicts(target_tip, anchor)?;
        if paths.is_empty() {
            cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, proof);
        } else {
            return_to_worker(root, &task, target, target_tip, &paths, actor)?;
        }
    }
    Ok(())
}

pub(crate) fn scan(root: &Path) -> Result<()> {
    let store = crate::store::open_task_store_cached(root)?;
    let mut targets = BTreeSet::new();
    for task in store.list(Some(TaskStatus::AwaitingMerge))? {
        let parent = store.get_parent_epic(&task.id)?;
        let Some(target) = close_ops::effective_close_work_target(&task, parent.as_ref()) else { continue; };
        if !target.target_branch.starts_with("epic/") { continue; }
        // Resolve the same repository authority as the actual merge.
        let context = repo_context::resolve_repo_context_from_local_root(root, &target)
            .or_else(|_| repo_context::resolve_repo_context(root, &target));
        match context {
            Ok(context) => { targets.insert((context.repo_root, context.target_branch)); }
            Err(error) => tracing::debug!(task_id = %task.id, %error, "parked conflict scan has no local repository"),
        }
    }
    for (repo, target) in targets {
        let git = GitOperations::new(repo.clone());
        if git.has_origin_remote() {
            anyhow::ensure!(
                crate::git_evidence::fetch_parent_branch_result(&repo, &target) == crate::git_evidence::BoundedFetch::Succeeded,
                "could not refresh train {target}",
            );
        }
        let reference = close_ops::freshest_target_ref(&repo, &target);
        let tip = git.resolve_commit(&reference).context("train tip unavailable")?;
        recheck_target(root, &repo, &target, &tip, "cassy-conflict-preflight")?;
    }
    Ok(())
}
