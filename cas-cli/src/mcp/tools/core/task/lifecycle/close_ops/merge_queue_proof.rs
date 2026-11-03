//! Read the existing CI tree receipt instead of repeating a proven suite.
//!
//! This is called only after close authenticates a supervisor override and
//! only for its mandatory scoped-proof obligation. A passing workflow alone
//! is insufficient: ci.yml publishes the tree artifact after Fast Validation
//! passes, including any authoritative full-gate reuse. Docs-only admission
//! never publishes it. Content, CI, QA and declared-risk admission remain in
//! the ordinary close path.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::bounded_process::{Deadline, run_command};
use crate::git_evidence::{BoundedFetch, fetch_parent_branch_result, git_commit_is_ancestor};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn api(repo: &Path, binary: &Path, endpoint: &str, deadline: Deadline) -> Result<Value, String> {
    let mut command = Command::new(binary);
    command
        .current_dir(repo)
        .env("GH_HOST", "github.com")
        .args(["api", endpoint]);
    let output = run_command(&mut command, deadline, PROBE_TIMEOUT)
        .map_err(|_| "merge-group evidence lookup unavailable or timed out")?;
    if !output.status.success() {
        // Provider stderr can contain credentials; never echo it into a task.
        return Err("merge-group evidence lookup failed".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "invalid merge-group evidence JSON".into())
}

/// The current override's reason must explicitly cite `merge_group_run=<id>`.
/// Old notes, free-form claims of green CI and workflow success without the
/// canonical tree receipt never authorize this substitution.
pub(super) fn verify(
    reason: &str,
    repo: &Path,
    delivery: Option<&str>,
) -> Result<Option<String>, String> {
    let citations: Vec<_> = reason
        .split_whitespace()
        .filter_map(|word| {
            word.strip_prefix("merge_group_run=")
                .map(|value| value.trim_end_matches([',', ';', '.']))
        })
        .collect();
    if citations.is_empty() {
        return Ok(None);
    }
    if citations.len() != 1 || !citations[0].bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("cite exactly one positive merge_group_run=<numeric run id>".into());
    }
    let run_id: u64 = citations[0]
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or("merge_group_run must be a positive numeric run id")?;
    let delivery = delivery
        .filter(|sha| full_sha(sha))
        .ok_or("delivery SHA is unresolved")?;
    let deadline = Deadline::after(Duration::from_secs(12));
    let binary = crate::github_repo::gh_binary();
    let github = crate::github_repo::resolve_origin(repo, &binary, PROBE_TIMEOUT)?;
    let slug = &github.canonical;
    let run = api(
        repo,
        &binary,
        &format!("repos/{slug}/actions/runs/{run_id}"),
        deadline,
    )?;
    if run["id"].as_u64() != Some(run_id)
        || run["event"] != "merge_group"
        || run["status"] != "completed"
        || run["conclusion"] != "success"
        || run["path"]
            .as_str()
            .map(|path| path.split('@').next().unwrap_or_default())
            != Some(".github/workflows/ci.yml")
        || run.pointer("/repository/full_name").and_then(Value::as_str) != Some(slug.as_str())
        || run
            .pointer("/head_repository/full_name")
            .and_then(Value::as_str)
            != Some(slug.as_str())
    {
        return Err(
            "cited run is not successful completed merge-group CI for this repository".into(),
        );
    }
    let default = super::resolve_close_gate_default_branch(repo)?;
    let prefix = format!("gh-readonly-queue/{default}/");
    if !run["head_branch"]
        .as_str()
        .is_some_and(|branch| branch.starts_with(&prefix))
    {
        return Err("cited run did not validate the default branch's merge queue".into());
    }
    let head = run["head_sha"]
        .as_str()
        .filter(|sha| full_sha(sha))
        .ok_or("merge-group head SHA is missing or invalid")?;
    // GitHub binds the synthetic queue commit to its exact tested tree. Its
    // commit identity need not equal the later commit that lands on main.
    let commit = api(
        repo,
        &binary,
        &format!("repos/{slug}/git/commits/{head}"),
        deadline,
    )?;
    let tree = commit
        .pointer("/tree/sha")
        .and_then(Value::as_str)
        .filter(|sha| full_sha(sha))
        .ok_or("merge-group tree SHA is missing or invalid")?;
    if commit["sha"] != head {
        return Err("merge-group commit lookup returned a different head".into());
    }
    let artifacts = api(
        repo,
        &binary,
        &format!("repos/{slug}/actions/runs/{run_id}/artifacts?per_page=100"),
        deadline,
    )?;
    let marker = format!("merge-queue-validated-tree-{tree}");
    if !artifacts["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts
            .iter()
            .any(|artifact| artifact["name"] == marker && artifact["expired"] == false)
    }) {
        return Err("cited run has no live Fast Validation receipt for its exact tree".into());
    }
    if fetch_parent_branch_result(repo, &default) != BoundedFetch::Succeeded {
        return Err(
            "cannot refresh the default branch to establish landed-tree containment".into(),
        );
    }
    let target = format!("origin/{default}");
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .args(["log", "--first-parent", "--format=%H:%T", &target]);
    let output = run_command(&mut command, deadline, PROBE_TIMEOUT)
        .map_err(|_| "landed-tree lookup unavailable or timed out")?;
    if !output.status.success() {
        return Err("cannot inspect the default branch's landed trees".into());
    }
    let history = String::from_utf8_lossy(&output.stdout);
    let landed = history
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(sha, candidate_tree)| {
            (candidate_tree == tree && full_sha(sha) && git_commit_is_ancestor(repo, delivery, sha))
                .then_some(sha)
        })
        .ok_or("the validated tree has no default-branch landing containing this delivery")?;
    Ok(Some(format!(
        "MERGE_QUEUE_PROOF: repo={slug} run={run_id} head={head} tree={tree} landed={landed} target={target} delivery={delivery}"
    )))
}
