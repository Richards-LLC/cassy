//! Successful evidence closes measure immutable unmerged history rather than
//! caller-supplied file lists or the mutable worker checkout.
use super::{Task, resolve_close_delivery_branch};
use crate::git_evidence::{git_text, resolve_ref_commit_sha};
use std::{collections::BTreeSet, path::Path};

pub(super) struct Measurement {
    pub base: String,
    pub tip: String,
    pub paths: Vec<String>,
}

fn evidence_path(path: &str) -> bool {
    let mut components = Path::new(path).components();
    matches!(components.next(), Some(std::path::Component::Normal(root))
        if root == "docs" || root == "artifacts")
        && components.clone().next().is_some()
        && components.all(|part| matches!(part, std::path::Component::Normal(_)))
        && Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "md" | "txt"
                        | "html"
                        | "pdf"
                        | "svg"
                        | "png"
                        | "jpg"
                        | "jpeg"
                        | "webp"
                        | "gif"
                        | "json"
                        | "csv"
                        | "tsv"
                        | "log"
                        | "yaml"
                        | "yml"
                )
            })
}

pub(super) fn measure(repo: &Path, task: &Task, target: &str) -> Result<Measurement, String> {
    let target = super::super::qa_dispatch::freshest_target_ref(repo, target);
    let target = resolve_ref_commit_sha(repo, &target)
        .ok_or("cannot resolve the integration target for evidence measurement")?;
    // GH #1151: a no-code task with no commit of its own keeps its evidence
    // in the artifact. Record the target it was judged against and no paths;
    // the worker lane is never measured as its delivery.
    // GH #1167: evidence_only is a supervisor review of the artifact, so only
    // commits that name this task count as its own.
    // cas-b38a: an operations task that never declared a methodology and
    // has no commit of its own (cas-3507: its work was a pull request in
    // another repository) is judged the same way; a code methodology keeps
    // the measurement below.
    if super::no_code_task_without_own_commits_judged(repo, task, &target, None, true)
        || (task.execution_note.is_none()
            && super::task_without_own_commits_judged(repo, task, &target, None, true))
    {
        return Ok(Measurement {
            base: target.clone(),
            tip: target,
            paths: Vec::new(),
        });
    }
    let delivery = if let Some(anchor) = task.deliverables.factory_branch_anchor.as_ref() {
        anchor.clone()
    } else {
        let assignee = task
            .assignee
            .as_deref()
            .ok_or("evidence delivery needs an assigned worker or recorded delivery anchor")?;
        resolve_close_delivery_branch(repo, task, assignee)?
    };
    let tip = resolve_ref_commit_sha(repo, &delivery)
        .ok_or("cannot resolve the task's immutable evidence delivery")?;
    let base = git_text(repo, &["merge-base", &target, &tip])
        .ok_or("cannot find the evidence delivery base")?;
    let commits = git_text(
        repo,
        &["rev-list", "--max-count=257", &tip, &format!("^{target}")],
    )
    .ok_or("cannot enumerate the unmerged evidence delivery")?;
    let commits: Vec<_> = commits.lines().collect();
    if commits.is_empty() || commits.len() > 256 {
        return Err("evidence-only requires 1–256 unmerged commits; use ordinary close for integrated delivery".into());
    }
    let mut paths = BTreeSet::new();
    for commit in commits {
        let parent = git_text(repo, &["rev-parse", &format!("{commit}^1")])
            .ok_or("cannot resolve an evidence commit parent")?;
        // Check every commit, including reverted changes and merge resolutions.
        // Disabling rename detection measures both original and destination paths.
        let diff = git_text(
            repo,
            &["diff", "--raw", "-z", "--no-renames", &parent, commit, "--"],
        )
        .ok_or("cannot measure evidence commit paths and modes")?;
        let mut fields = diff.split('\0').filter(|field| !field.is_empty());
        while let Some(header) = fields.next() {
            let path = fields.next().ok_or("malformed evidence path measurement")?;
            let modes: Vec<_> = header.split_whitespace().collect();
            if modes.len() != 5 || !modes[0].starts_with(':') {
                return Err("malformed evidence mode measurement".into());
            }
            if path.contains(char::REPLACEMENT_CHARACTER)
                || !evidence_path(path)
                || ![modes[0].trim_start_matches(':'), modes[1]]
                    .iter()
                    .all(|mode| matches!(*mode, "000000" | "100644"))
            {
                return Err(format!(
                    "delivery touches non-evidence path or executable/symlink/submodule: {path}. Only regular docs/artifacts evidence formats are permitted."
                ));
            }
            paths.insert(path.to_string());
        }
    }
    if paths.is_empty() {
        return Err("evidence delivery has no measured docs/artifacts changes".into());
    }
    Ok(Measurement {
        base,
        tip,
        paths: paths.into_iter().collect(),
    })
}
