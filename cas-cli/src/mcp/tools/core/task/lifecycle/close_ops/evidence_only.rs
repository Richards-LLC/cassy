//! Successful evidence closes measure the immutable delivery's net diff rather than
//! caller-supplied file lists or the mutable worker checkout.
use super::{Task, resolve_close_delivery_branch};
use crate::git_evidence::{git_text, resolve_ref_commit_sha};
use std::{collections::BTreeSet, path::Path};

pub(super) struct Measurement {
    pub base: String,
    pub tip: String,
    pub paths: Vec<String>,
    pub audit: Option<String>,
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

pub(super) fn measure(repo: &Path, task: &Task, target: &str, reference: &str) -> Result<Measurement, String> {
    let target = super::super::qa_dispatch::freshest_target_ref(repo, target);
    let target = resolve_ref_commit_sha(repo, &target)
        .ok_or("cannot resolve the integration target for evidence measurement")?;
    if let Some(branch) = reference.strip_prefix("never-merge:branch:") {
        return measure_workflow_proof(repo, task, &target, branch);
    }
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
            audit: None,
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
    let commits = unmerged_commits(repo, std::slice::from_ref(&tip), &target)?;
    if commits.is_empty() {
        return Err("evidence-only requires 1–256 unmerged commits; use ordinary close for integrated delivery".into());
    }
    let paths = measure_paths(repo, &[(base.clone(), tip.clone())], false)?;
    Ok(Measurement { base, tip, paths, audit: None })
}

/// One bounded union for all relevant refs; the same 256-commit limit applies
/// to ordinary evidence and the explicitly non-delivery proof disposition.
fn unmerged_commits(repo: &Path, tips: &[String], target: &str) -> Result<Vec<String>, String> {
    let excluded = format!("^{target}");
    let mut args = vec!["rev-list", "--max-count=257"];
    args.extend(tips.iter().map(String::as_str));
    args.push(&excluded);
    args.push("--");
    let commits = git_text(repo, &args).ok_or("cannot enumerate the unmerged evidence delivery")?;
    let commits: Vec<String> = commits.lines().map(str::to_string).collect();
    if commits.len() > 256 {
        return Err("evidence-only exceeds 256 unmerged commits".into());
    }
    Ok(commits)
}

/// The caller has already checked live supervisor authority, the task-root
/// artifact, reference grammar and rationale. This is a non-integration
/// declaration, never a waiver of an unmeasured product delivery.
fn measure_workflow_proof(repo: &Path, task: &Task, target: &str, branch: &str) -> Result<Measurement, String> {
    let delivery = &task.deliverables;
    if task.execution_note.as_deref().is_some_and(|note| note != "no-code")
        || delivery.integration_batch.is_some()
        || !delivery.files_changed.is_empty()
        || delivery.commit_hash.is_some()
        || delivery.merge_commit.is_some()
        || delivery.delivery_pr_merge_commit.is_some()
    {
        return Err("never-merge proof cannot retire a code methodology or recorded product delivery".into());
    }
    // Resolve branch refs explicitly, not an arbitrary revision or tag. Both
    // local and origin snapshots must be workflow-only if both are present.
    let mut required_tips = Vec::new();
    for reference in [format!("refs/remotes/origin/{branch}"), format!("refs/heads/{branch}")] {
        if super::git_ref_exists(repo, &reference) {
            required_tips.push(resolve_ref_commit_sha(repo, &reference)
                .ok_or("cannot resolve the never-merge proof branch")?);
        }
    }
    let proof_tip = required_tips.first().cloned().ok_or("never-merge proof branch is missing")?;
    if required_tips.iter().any(|tip| super::git_commit_is_ancestor(repo, tip, target)) {
        return Err("never-merge proof branch must be unmerged into the fresh target".into());
    }
    for anchor in delivery.factory_branch_anchor.iter().chain(&delivery.historical_factory_branch_anchors) {
        required_tips.push(resolve_ref_commit_sha(repo, anchor)
            .ok_or("cannot resolve a retained delivery anchor for never-merge proof")?);
    }
    let required: BTreeSet<String> = unmerged_commits(repo, &required_tips, target)?.into_iter().collect();
    let mut tips = required_tips;
    for lane in super::task_delivery_branch_candidates(task) {
        let lane = lane.strip_prefix("origin/").unwrap_or(&lane);
        for reference in [format!("refs/heads/{lane}"), format!("refs/remotes/origin/{lane}")] {
            if super::git_ref_exists(repo, &reference) {
                tips.push(resolve_ref_commit_sha(repo, &reference)
                    .ok_or("cannot resolve a task lane for never-merge proof")?);
            }
        }
    }
    let identity = super::TaskCommitIdentity { start_base: None, task_id: Some(task.id.clone()), known_commits: Vec::new() };
    let commits: Vec<String> = unmerged_commits(repo, &tips, target)?.into_iter()
        .filter(|commit| required.contains(commit)
            || !super::task_attribution::commit_claims_another_task(repo, commit, &identity))
        .collect();
    // Workflow proofs are intentionally never delivered: every intermediate
    // commit must stay workflow-only, even when later commits revert it.
    let mut comparisons = Vec::with_capacity(commits.len());
    for commit in &commits {
        if super::git_commit_parent_count(repo, commit) != 1 {
            return Err("never-merge workflow proof requires non-merge commits with one parent".into());
        }
        let parent = git_text(repo, &["rev-parse", &format!("{commit}^1")])
            .ok_or("cannot resolve an evidence commit parent")?;
        comparisons.push((parent, commit.clone()));
    }
    let paths = measure_paths(repo, &comparisons, true)?;
    Ok(Measurement {
        base: target.to_string(), tip: target.to_string(), paths: Vec::new(),
        audit: Some(format!(
            "never-merge workflow proof {branch}@{proof_tip} declared non-delivery against {target}; measured commits: {}; workflow paths: {}. These proof branches must never be merged.",
            commits.join(", "), paths.join(", ")
        )),
    })
}

fn workflow_path(path: &str) -> bool {
    let parts: Vec<_> = Path::new(path).components().collect();
    matches!(parts.as_slice(), [std::path::Component::Normal(root), std::path::Component::Normal(dir), std::path::Component::Normal(_)]
        if *root == ".github" && *dir == "workflows")
        && matches!(Path::new(path).extension().and_then(|ext| ext.to_str()), Some("yml" | "yaml"))
}

fn measure_paths(repo: &Path, comparisons: &[(String, String)], workflow_only: bool) -> Result<Vec<String>, String> {
    let mut paths = BTreeSet::new();
    for (base, tip) in comparisons {
        // Disabling rename detection measures both original and destination
        // paths; raw modes also expose executable/symlink/submodule changes.
        let diff = git_text(
            repo,
            &["diff", "--raw", "-z", "--no-renames", base, tip, "--"],
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
                || !(if workflow_only { workflow_path(path) } else { evidence_path(path) })
                || ![modes[0].trim_start_matches(':'), modes[1]]
                    .iter()
                    .all(|mode| matches!(*mode, "000000" | "100644"))
            {
                return Err(format!(
                    "delivery touches non-evidence path or executable/symlink/submodule: {path}. Only regular {} are permitted.",
                    if workflow_only { ".github/workflows/*.yml or *.yaml proof files" } else { "docs/artifacts evidence formats" }
                ));
            }
            paths.insert(path.to_string());
        }
    }
    if paths.is_empty() {
        return Err("evidence delivery has no measured docs/artifacts changes".into());
    }
    Ok(paths.into_iter().collect())
}
