//! Positive content evidence for replacing a rewritten parked delivery: the
//! one delivery patch-id (`qa_pass::change_patch_id_excluding`, design rev 3.1
//! §1.3), behind the fences that let a rewritten history keep its identity.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{Deadline, run_command};

/// Patch-id of the task's own linear history over `target`. A merge commit or
/// a commit message claiming another task is not this delivery's rewrite.
fn delivery_patch_id(repo: &Path, tip: &str, target: &str, task_id: &str) -> Option<String> {
    let range = format!("{target}..{tip}");
    let log = run_command(
        Command::new("git").current_dir(repo).args(["log", "--format=%P%x1f%B%x1e", &range, "--"]),
        Deadline::after(Duration::from_secs(5)),
        Duration::from_secs(2),
    )
    .ok()
    .filter(|output| output.status.success())?;
    let log = String::from_utf8(log.stdout).ok()?;
    let commits: Vec<_> = log.split('\u{1e}').map(str::trim_start).filter(|c| !c.is_empty()).collect();
    let foreign = |commit: &&str| commit.split_once('\u{1f}').is_none_or(|(parents, message)| {
        parents.split_whitespace().count() != 1 || super::message_claims_other_task(message, Some(task_id))
    });
    if commits.is_empty() || commits.iter().any(foreign) {
        return None;
    }
    crate::qa_pass::change_patch_id_excluding(repo, tip, target, &[])
}

/// Whether `tip` rewrites `recorded` with the same delivered change, each
/// measured over its own merge-base with `target`.
pub(crate) fn same_patch_set(repo: &Path, recorded: &str, tip: &str, target: &str, task_id: &str) -> bool {
    if recorded == tip || [recorded, tip, target].iter().any(|r| !super::is_safe_git_refname(r)) {
        return false;
    }
    match (delivery_patch_id(repo, recorded, target, task_id), delivery_patch_id(repo, tip, target, task_id)) {
        (Some(recorded), Some(candidate)) => recorded == candidate,
        _ => false,
    }
}

/// Find the rewritten boundary before corrections added after a rebase.
/// Equality covers the complete old delivery; callers still validate every
/// commit after that boundary as an attributable continuation of this task.
pub(super) fn equivalent_prefix(repo: &Path, recorded: &str, tip: &str, target: &str, task_id: &str) -> Option<String> {
    if [recorded, tip, target].iter().any(|r| !super::is_safe_git_refname(r))
        || super::git_commit_is_ancestor(repo, recorded, tip)
    {
        return None;
    }
    let original = delivery_patch_id(repo, recorded, target, task_id)?;
    let candidates = super::git_stdout_lines(repo, &["rev-list", "--first-parent", &format!("{target}..{tip}"), "--"]).ok()?;
    candidates.into_iter().skip(1).find(|candidate| {
        delivery_patch_id(repo, candidate, target, task_id).as_deref() == Some(original.as_str())
    })
}
