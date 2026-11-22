//! Fresh provider observations. No cached target SHA, heartbeat or caller JSON
//! can stand in for PR queue/auto-merge and active workflow observations.
use std::{path::{Path, PathBuf}, process::Command, time::Duration};
use serde_json::{Value, json};
use crate::worktree::git::train_authority::{Snapshot, TransferChecks};

const TIMEOUT: Duration = Duration::from_secs(30);
const ACTIVE: [&str; 5] = ["requested", "queued", "in_progress", "waiting", "pending"];

fn pending(detail: impl std::fmt::Display) -> String {
    format!("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: {detail}")
}

pub(crate) trait Provider {
    fn prs(&self, branch: &str, cursor: Option<&str>) -> Result<Value, String>;
    fn runs(&self, status: &str) -> Result<Value, String>;
}

pub(crate) struct Gh {
    repo: PathBuf,
    slug: String,
}

impl Gh {
    fn request(&self, args: &[&str]) -> Result<Value, String> {
        let mut command = Command::new(crate::github_repo::gh_binary());
        command.current_dir(&self.repo).env("GH_HOST", "github.com").args(args);
        let output = crate::bounded_process::run_command(&mut command,
            crate::bounded_process::Deadline::after(TIMEOUT), TIMEOUT)
            .map_err(|e| pending(format!("provider read unavailable: {e:?}")))?;
        if !output.status.success() { return Err(pending("provider read failed")); }
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| pending("provider returned invalid JSON"))?;
        if value.get("errors").is_some_and(|errors| errors.as_array().is_none_or(|errors| !errors.is_empty())) {
            return Err(pending("provider returned GraphQL errors"));
        }
        Ok(value)
    }
}

impl Provider for Gh {
    fn prs(&self, branch: &str, cursor: Option<&str>) -> Result<Value, String> {
        let (owner, name) = self.slug.split_once('/').ok_or_else(|| pending("invalid repository"))?;
        let owner = format!("owner={owner}");
        let name = format!("name={name}");
        let branch = format!("branch={branch}");
        let mut args = vec!["api", "graphql", "-f",
            "query=query($owner:String!,$name:String!,$branch:String!,$cursor:String){repository(owner:$owner,name:$name){nameWithOwner pullRequests(first:100,headRefName:$branch,after:$cursor){nodes{number state headRefName autoMergeRequest{enabledAt} mergeQueueEntry{id}} pageInfo{hasNextPage endCursor}}}}",
            "-f", &owner, "-f", &name, "-f", &branch];
        let cursor = cursor.map(|cursor| format!("cursor={cursor}"));
        if let Some(cursor) = &cursor { args.extend(["-f", cursor]); }
        self.request(&args)
    }

    fn runs(&self, status: &str) -> Result<Value, String> {
        self.request(&["api", "--method", "GET", &format!("repos/{}/actions/runs?per_page=1&status={status}", self.slug)])
    }
}

pub(crate) struct GitHubChecks<P = Gh> {
    provider: P,
    slug: String,
}

impl GitHubChecks<Gh> {
    pub(crate) fn new(repo: &Path) -> Result<Self, String> {
        let slug = crate::github_repo::origin_slug(repo, TIMEOUT).map_err(pending)?
            .ok_or_else(|| pending("canonical GitHub origin is unavailable"))?;
        Ok(Self { provider: Gh { repo: repo.into(), slug: slug.clone() }, slug })
    }
}

impl<P: Provider> TransferChecks for GitHubChecks<P> {
    fn external_clear(&self, snapshot: &Snapshot) -> Result<Value, String> {
        let mut cursor = None::<String>;
        let mut seen = std::collections::BTreeSet::new();
        let mut observed = Vec::new();
        loop {
            let page = self.provider.prs(&snapshot.record.target, cursor.as_deref()).map_err(pending)?;
            let repo = page.pointer("/data/repository").ok_or_else(|| pending("PR repository missing"))?;
            if repo["nameWithOwner"].as_str().is_none_or(|repo| !repo.eq_ignore_ascii_case(&self.slug)) {
                return Err(pending("PR repository scope disagrees"));
            }
            let connection = &repo["pullRequests"];
            let prs = connection["nodes"].as_array().ok_or_else(|| pending("PR observation incomplete"))?;
            for pr in prs {
                let state = pr["state"].as_str().ok_or_else(|| pending("PR state missing"))?;
                if pr["headRefName"] != snapshot.record.target || pr["number"].as_u64().is_none()
                    || !pr.get("autoMergeRequest").is_some() || !pr.get("mergeQueueEntry").is_some()
                    || !matches!(state, "OPEN" | "CLOSED" | "MERGED")
                { return Err(pending("PR state or association unknown")); }
                if state == "OPEN" && (!pr["autoMergeRequest"].is_null() || !pr["mergeQueueEntry"].is_null()) {
                    return Err(pending(format!("PR {} remains armed or queued", pr["number"])));
                }
                observed.push(pr.clone());
            }
            match connection.pointer("/pageInfo/hasNextPage").and_then(Value::as_bool) {
                Some(false) => break,
                Some(true) => {
                    let next = connection.pointer("/pageInfo/endCursor").and_then(Value::as_str)
                        .filter(|s| !s.is_empty()).ok_or_else(|| pending("PR pagination incomplete"))?;
                    if !seen.insert(next.to_string()) { return Err(pending("PR pagination repeated")); }
                    cursor = Some(next.into());
                }
                None => return Err(pending("PR pagination unknown")),
            }
        }
        // Until every release/downstream reservation is provider-bound, take
        // the conservative repository-wide view. Any active run holds; an
        // unknown association can never be treated as an unrelated safe run.
        // No pagination is needed to prove zero; a nonzero count already holds.
        for status in ACTIVE {
            let runs = self.provider.runs(status).map_err(pending)?;
            if runs["total_count"].as_u64() != Some(0)
                || runs["workflow_runs"].as_array().is_none_or(|runs| !runs.is_empty())
            { return Err(pending(format!("{status} publication runs remain active or unknown"))); }
        }
        Ok(json!({"repository": self.slug, "prs": observed,
            "active_runs": 0, "run_scope": "repository", "observed_at": chrono::Utc::now()}))
    }

    fn publishers_stopped(&self, _snapshot: &Snapshot) -> Result<Value, String> {
        // An authenticated transfer command is not positive coverage of old
        // daemons/credentials. Enrollment and quiescence stay held until the
        // runtime participation inventory can establish that boundary.
        Err("TRAIN_TRANSFER_QUIESCENCE_REQUIRED: publication runtime coverage is not established".into())
    }
}

#[cfg(test)]
pub(crate) fn fixture_checks<P: Provider>(provider: P) -> GitHubChecks<P> {
    GitHubChecks { provider, slug: "test/fixture".into() }
}
