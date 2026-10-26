//! Positive content evidence for replacing a rewritten parked delivery.

use std::io::{Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::bounded_process::{Deadline, run_command, run_command_with_stdin};

fn git(repo: &Path, args: &[&str], deadline: Deadline) -> Option<Vec<u8>> {
    run_command(
        Command::new("git").current_dir(repo).args(args),
        deadline,
        Duration::from_secs(2),
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| output.stdout)
}

fn patches(
    repo: &Path,
    tip: &str,
    target: &str,
    task_id: &str,
    deadline: Deadline,
) -> Option<Vec<String>> {
    let base = String::from_utf8(git(repo, &["merge-base", target, tip], deadline)?).ok()?;
    let range = format!("{}..{tip}", base.trim());
    let parents =
        String::from_utf8(git(repo, &["log", "--format=%P", &range, "--"], deadline)?).ok()?;
    let count = parents.lines().count();
    // A merge's resolution is not represented by its side-commit patch set.
    // Empty/unknown ranges and commits without a patch are not positive proof.
    if count == 0
        || parents
            .lines()
            .any(|line| line.split_whitespace().count() != 1)
    {
        return None;
    }
    let messages = String::from_utf8(git(
        repo,
        &["log", "--format=%B%x1e", &range, "--"],
        deadline,
    )?)
    .ok()?;
    if messages
        .split('\u{1e}')
        .any(|message| super::message_claims_other_task(message, Some(task_id)))
    {
        return None;
    }
    let diff = git(
        repo,
        &[
            "log",
            "--pretty=format:commit %H",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "-p",
            &range,
            "--",
        ],
        deadline,
    )?;
    let mut input = tempfile::tempfile().ok()?;
    input.write_all(&diff).ok()?;
    input.rewind().ok()?;
    let output = run_command_with_stdin(
        Command::new("git")
            .current_dir(repo)
            .args(["patch-id", "--verbatim"]),
        deadline,
        Duration::from_secs(2),
        Stdio::from(input),
    )
    .ok()
    .filter(|output| output.status.success())?;
    let output = String::from_utf8(output.stdout).ok()?;
    let valid_hash = |value: &str| {
        matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    let mut patches = Vec::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (patch, commit) = (fields.next()?, fields.next()?);
        if fields.next().is_some() || !valid_hash(patch) || !valid_hash(commit) {
            return None;
        }
        patches.push(patch.to_string());
    }
    if patches.len() != count {
        return None;
    }
    // Keep multiplicity: an extra copy of a patch is also new delivery work.
    patches.sort();
    Some(patches)
}

/// Compare every task patch relative to each tip's own target merge-base.
/// Whole trees would include unrelated target updates; last-commit comparison
/// could lose earlier work. Verbatim patch IDs retain meaningful whitespace.
pub(crate) fn same_patch_set(
    repo: &Path,
    recorded: &str,
    tip: &str,
    target: &str,
    task_id: &str,
) -> bool {
    if recorded == tip
        || [recorded, tip, target]
            .iter()
            .any(|reference| !super::is_safe_git_refname(reference))
    {
        return false;
    }
    let deadline = Deadline::after(Duration::from_secs(5));
    match (
        patches(repo, recorded, target, task_id, deadline),
        patches(repo, tip, target, task_id, deadline),
    ) {
        (Some(recorded), Some(candidate)) => recorded == candidate,
        _ => false,
    }
}

/// Find the rewritten boundary before corrections added after a rebase.
/// Equality covers the complete old delivery; callers still validate every
/// commit after that boundary as an attributable continuation of this task.
pub(super) fn equivalent_prefix(
    repo: &Path,
    recorded: &str,
    tip: &str,
    target: &str,
    task_id: &str,
) -> Option<String> {
    if [recorded, tip, target]
        .iter()
        .any(|reference| !super::is_safe_git_refname(reference))
        || super::git_commit_is_ancestor(repo, recorded, tip)
    {
        return None;
    }
    let deadline = Deadline::after(Duration::from_secs(5));
    let original = patches(repo, recorded, target, task_id, deadline)?;
    let range = format!("{target}..{tip}");
    let candidates = String::from_utf8(git(
        repo,
        &["rev-list", "--first-parent", &range, "--"],
        deadline,
    )?).ok()?;
    candidates.lines().skip(1).take_while(|_| !deadline.remaining().is_zero()).find_map(|candidate| {
        (patches(repo, candidate, target, task_id, deadline).as_ref() == Some(&original))
            .then(|| candidate.to_string())
    })
}
