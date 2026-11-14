//! Route a confirmed default-branch merge queue through an exact-source PR.
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cas_types::Task;
use serde_json::Value;

use crate::bounded_process::{Deadline, run_command};

const PROBE: Duration = Duration::from_secs(10);
const WRITE: Duration = Duration::from_secs(30);
const FIELDS: &str = "number,url,state,headRefOid,headRefName,baseRefName";

pub(super) struct PullRequest {
    pub number: u64,
    pub url: String,
    pub repo: String,
    pub head: String,
}

#[track_caller]
fn gh(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(crate::github_repo::gh_binary());
    command
        .current_dir(repo)
        .env("GH_HOST", "github.com")
        .args(args);
    let output = run_command(&mut command, Deadline::after(WRITE), WRITE)
        .map_err(|_| "PR route command unavailable or timed out")?;
    if !output.status.success() {
        // Provider diagnostics may include credentials. Keep them out of tool replies.
        return Err(format!(
            "PR route {} {} failed; retry worktree_merge",
            args[0], args[1]
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .map_err(|_| "PR route returned invalid text".into())
}

fn checked_pr(
    value: Value,
    slug: &str,
    source: &str,
    target: &str,
    head: &str,
) -> Result<PullRequest, String> {
    let number = value["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or("PR number missing")?;
    let expected_url = format!("https://github.com/{slug}/pull/{number}");
    if value["state"] != "OPEN"
        || value["headRefName"] != source
        || value["baseRefName"] != target
        || value["headRefOid"] != head
        || value["url"] != expected_url
    {
        return Err("PR identity or head changed; no auto-merge was enabled".into());
    }
    Ok(PullRequest {
        number,
        url: expected_url,
        repo: slug.into(),
        head: head.into(),
    })
}

fn find_pr(
    repo: &Path,
    slug: &str,
    source: &str,
    target: &str,
    head: &str,
) -> Result<Option<PullRequest>, String> {
    let output = gh(
        repo,
        &[
            "pr", "list", "--repo", slug, "--base", target, "--head", source, "--state", "open",
            "--json", FIELDS,
        ],
    )?;
    let value: Value = serde_json::from_str(&output).map_err(|_| "invalid PR list response")?;
    let rows = value.as_array().ok_or("invalid PR list response")?;
    match rows.as_slice() {
        [] => Ok(None),
        [value] => checked_pr(value.clone(), slug, source, target, head).map(Some),
        _ => Err("multiple open delivery PRs; no auto-merge was enabled".into()),
    }
}

/// A confirmed absent queue leaves the existing merge/publication path in
/// force. The caller has already enforced review, CI, target and source admission.
pub(super) fn prepare(
    repo: &Path,
    slug: &str,
    source: &str,
    target: &str,
    head: &str,
    task: Option<&Task>,
) -> Result<Option<PullRequest>, String> {
    let binary = crate::github_repo::gh_binary();
    match crate::github_repo::merge_queue_enabled(slug, target, repo, &binary, PROBE) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        // cas-662e: a failed probe is not "no queue". Treating it as one
        // merged locally into a queued main, or refused over the operator's
        // edits in its checkout. Refuse before any target mutation.
        Err(error) => {
            tracing::warn!(%slug, %target, %error, "merge queue probe failed; merge refused before any local merge");
            return Err(format!(
                "merge queue state unknown for {target} ({error}); no merge was attempted, retry worktree_merge"
            ));
        }
    }
    let body = crate::review_body::delivery_body(
        repo,
        task,
        &format!("origin/{target}"),
        head,
        None,
        None,
        None,
    )
    .map_err(|_| "cannot prepare the delivery PR review body")?;
    let operations = crate::worktree::GitOperations::new(repo.to_path_buf());
    if !operations
        .publish_pinned_source_to_origin(source, head, WRITE)
        .is_published()
    {
        return Err("source publication failed; default branch was not merged".into());
    }
    if let Some(pr) = find_pr(repo, slug, source, target, head)? {
        return Ok(Some(pr));
    }
    let mut file = tempfile::NamedTempFile::new().map_err(|_| "cannot create PR body file")?;
    file.write_all(body.as_bytes())
        .map_err(|_| "cannot write PR body file")?;
    let path = file.path().to_str().ok_or("invalid PR body path")?;
    let title = task
        .map(|task| task.title.clone())
        .unwrap_or_else(|| format!("Review {source}"));
    let created = gh(
        repo,
        &[
            "pr",
            "create",
            "--repo",
            slug,
            "--base",
            target,
            "--head",
            source,
            "--title",
            &title,
            "--body-file",
            path,
        ],
    );
    let url = match created {
        Ok(url) => url,
        // A concurrent retry may have created it. Reuse only the exact identity.
        Err(error) => {
            return find_pr(repo, slug, source, target, head)?
                .map(Some)
                .ok_or(error);
        }
    };
    let prefix = format!("https://github.com/{slug}/pull/");
    if !url
        .strip_prefix(&prefix)
        .is_some_and(|number| number.parse::<u64>().is_ok_and(|n| n > 0))
    {
        return Err("PR creation returned an invalid repository URL".into());
    }
    let output = gh(
        repo,
        &["pr", "view", &url, "--repo", slug, "--json", FIELDS],
    )?;
    let value = serde_json::from_str(&output).map_err(|_| "invalid created PR response")?;
    checked_pr(value, slug, source, target, head).map(Some)
}

/// A server-side expected-head check prevents arming a PR that moved after review.
pub(super) fn arm(core: &crate::mcp::CasCore, repo: &Path, pr: &PullRequest, task_id: Option<&str>, accepted_override_codes: &mut Vec<&'static str>) -> Result<(), String> {
    // PR preparation can discover its number after worktree_merge's first
    // check. Recheck the same ask gate before enabling server-side publication.
    if let Some(refusal) = core.peer_ask_merge_refusal(task_id, Some(pr.number), accepted_override_codes) {
        return Err(refusal);
    }
    gh(
        repo,
        &[
            "pr",
            "merge",
            &pr.url,
            "--repo",
            &pr.repo,
            "--auto",
            "--merge",
            "--match-head-commit",
            &pr.head,
        ],
    )
    .map(|_| ())
}
