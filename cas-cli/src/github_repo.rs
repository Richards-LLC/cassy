//! Explicit GitHub repository binding for CI and diagnostics.
//! A second remote or `gh repo set-default` must never select the CI repository.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedRepo {
    pub origin: String,
    pub canonical: String,
}

pub(crate) fn gh_binary() -> PathBuf {
    std::env::var_os(crate::github_issue_attach::GH_BIN_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"))
}

fn valid_slug(slug: &str) -> bool {
    let parts = slug.split('/').collect::<Vec<_>>();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
}

pub(crate) fn origin_slug(cwd: &Path, timeout: Duration) -> Result<Option<String>, String> {
    let mut git = Command::new("git");
    // Read the configured identity before Git's url.*.insteadOf transport
    // rewriting (SSH aliases and local fixture servers are not repository IDs).
    git.current_dir(cwd)
        .args(["config", "--get", "remote.origin.url"]);
    let output = run_command(&mut git, Deadline::after(timeout), timeout).map_err(|error| {
        match error {
            BoundedCommandError::TimedOut => "GitHub origin lookup timed out",
            BoundedCommandError::Io => {
                "GitHub origin lookup unavailable (cannot start or capture git)"
            }
        }
        .to_string()
    })?;
    // `git config --get` exits 1 for an unset key. Other failures, such as
    // an unreadable/malformed config, must not masquerade as absent origin.
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(format!("GitHub origin lookup failed ({})", output.status));
    }
    Ok(
        crate::cli::integrate::github::parse_origin_url(&String::from_utf8_lossy(&output.stdout))
            .map(|repo| repo.full_name())
            .filter(|slug| valid_slug(slug)),
    )
}

/// One explicit lookup follows GitHub's rename/transfer redirect. Callers cache
/// this result for all requests in the operation rather than resolving per SHA.
pub(crate) fn resolve_slug(
    origin: &str,
    cwd: &Path,
    binary: &Path,
    timeout: Duration,
) -> Result<ResolvedRepo, String> {
    if !valid_slug(origin) {
        return Err("GitHub origin is not an owner/repo slug".to_string());
    }
    let mut command = Command::new(binary);
    command.current_dir(cwd).env("GH_HOST", "github.com").args([
        "repo",
        "view",
        origin,
        "--json",
        "nameWithOwner",
    ]);
    let output = run_command(&mut command, Deadline::after(timeout), timeout)
        .map_err(|error| match error {
            BoundedCommandError::TimedOut => {
                format!("canonical GitHub repository lookup timed out for origin {origin}")
            }
            BoundedCommandError::Io => format!(
                "canonical GitHub repository lookup unavailable for origin {origin} (cannot start or capture gh)"
            ),
        })?;
    if !output.status.success() {
        // No stderr here: gh can include credentials in an error. The CI
        // receipt names the failed operation, not arbitrary provider output.
        return Err(format!(
            "canonical GitHub repository lookup failed for origin {origin} ({})",
            output.status
        ));
    }
    let canonical = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| {
            value
                .get("nameWithOwner")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .filter(|slug| valid_slug(slug))
        .ok_or_else(|| {
            "canonical GitHub repository response omitted a valid nameWithOwner".to_string()
        })?;
    Ok(ResolvedRepo {
        origin: origin.to_string(),
        canonical,
    })
}

pub(crate) fn resolve_origin(
    cwd: &Path,
    binary: &Path,
    timeout: Duration,
) -> Result<ResolvedRepo, String> {
    let origin = origin_slug(cwd, timeout)?.ok_or_else(|| {
        "GitHub CI repository not configured: origin is missing or is not a GitHub URL".to_string()
    })?;
    resolve_slug(&origin, cwd, binary, timeout)
}

/// Advisory queue settings, never authorization to write a ruleset or merge.
pub(crate) struct QueueReport {
    pub warning: bool,
    pub message: String,
}

/// Reuse one response for origin and queue diagnostics, including redirects.
pub(crate) fn repository_health(
    origin: &str,
    cwd: &Path,
    binary: &Path,
    timeout: Duration,
) -> Result<(ResolvedRepo, QueueReport), String> {
    if !valid_slug(origin) {
        return Err("GitHub merge queue unavailable: invalid origin".into());
    }
    let (owner, name) = origin.split_once('/').unwrap();
    let mut command = Command::new(binary);
    command.current_dir(cwd).env("GH_HOST", "github.com").args([
        "api",
        "graphql",
        "-f",
        &format!("owner={owner}"),
        "-f",
        &format!("name={name}"),
        "-f",
        concat!(
            "query=",
            include_str!("../../scripts/github-queue-health.graphql")
        ),
    ]);
    let output = run_command(&mut command, Deadline::after(timeout), timeout)
        .map_err(|_| "GitHub merge queue unavailable: cannot run gh or request timed out")?;
    // Provider stderr can carry credentials. Keep failure honest and bounded.
    if !output.status.success() {
        return Err(
            "GitHub merge queue unavailable: authentication, rate limit or provider failure".into(),
        );
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "GitHub merge queue unavailable: invalid JSON")?;
    if value
        .get("errors")
        .is_some_and(|e| e.as_array().is_none_or(|e| !e.is_empty()))
    {
        return Err("GitHub merge queue unavailable: GraphQL error (possibly rate limited)".into());
    }
    let repo = value
        .pointer("/data/repository")
        .ok_or("GitHub merge queue unavailable: missing repository")?;
    let canonical = repo
        .get("nameWithOwner")
        .and_then(serde_json::Value::as_str)
        .filter(|s| valid_slug(s))
        .ok_or("GitHub merge queue unavailable: invalid repository identity")?;
    let report = queue_report(repo).unwrap_or_else(|error| QueueReport {
        warning: true,
        message: format!("merge queue unavailable: {error}"),
    });
    Ok((
        ResolvedRepo {
            origin: origin.into(),
            canonical: canonical.into(),
        },
        report,
    ))
}

fn queue_report(repo: &serde_json::Value) -> Result<QueueReport, &'static str> {
    let branch = repo
        .pointer("/defaultBranchRef/name")
        .and_then(serde_json::Value::as_str)
        .ok_or("default branch missing")?;
    let rules = repo
        .pointer("/defaultBranchRef/rules")
        .ok_or("active rules missing")?;
    if rules
        .pointer("/pageInfo/hasNextPage")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
    {
        return Err("active rules incomplete; no additional request made");
    }
    let nodes = rules
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or("active rules missing")?;
    let mut reports = Vec::new();
    for rule in nodes.iter().filter(|r| r["type"] == "MERGE_QUEUE") {
        let build = rule
            .pointer("/parameters/maxEntriesToBuild")
            .and_then(serde_json::Value::as_u64)
            .ok_or("build limit missing")?;
        let merge = rule
            .pointer("/parameters/maxEntriesToMerge")
            .and_then(serde_json::Value::as_u64)
            .ok_or("merge limit missing")?;
        let grouping = rule
            .pointer("/parameters/groupingStrategy")
            .and_then(serde_json::Value::as_str)
            .ok_or("grouping strategy missing")?;
        if build >= 5 && merge >= 5 && grouping == "ALLGREEN" {
            continue;
        }
        let ruleset = &rule["repositoryRuleset"];
        let id = ruleset["databaseId"].as_u64().ok_or("ruleset id missing")?;
        let source = &ruleset["source"];
        let endpoint = match source["__typename"].as_str() {
            Some("Repository") => {
                let slug = source["nameWithOwner"]
                    .as_str()
                    .filter(|s| valid_slug(s))
                    .ok_or("ruleset source invalid")?;
                format!("repos/{slug}/rulesets/{id}")
            }
            _ => return Err("ruleset source unsupported"),
        };
        // Print only. Read the complete current ruleset before an operator's
        // PUT so unrelated protection, bypasses and higher limits survive.
        let fix = format!(
            r#"gh api {endpoint} | jq '{{name,target,enforcement,conditions,bypass_actors,rules}} | (.rules[] | select(.type == "merge_queue").parameters) |= (.max_entries_to_build = ([.max_entries_to_build,5]|max) | .max_entries_to_merge = ([.max_entries_to_merge,5]|max) | .grouping_strategy = "ALLGREEN")' | gh api -X PUT {endpoint} --input -"#
        );
        reports.push(format!("ruleset {id}: build={build}, merge={merge}, grouping={grouping}; expected build>=5, merge>=5, ALLGREEN. Run:\n{fix}"));
    }
    if !reports.is_empty() {
        return Ok(QueueReport {
            warning: true,
            message: format!("merge queue drift on {branch}: {}", reports.join("; ")),
        });
    }
    if !nodes.iter().any(|r| r["type"] == "MERGE_QUEUE") {
        return Ok(QueueReport {
            warning: false,
            message: format!("merge queue not configured on {branch}; advisory not applicable"),
        });
    }
    Ok(QueueReport {
        warning: false,
        message: format!("merge queue on {branch}: build>=5, merge>=5, ALLGREEN"),
    })
}

/// GitHub's nullable Repository.mergeQueue(branch) names the queue for this
/// branch, including ruleset-backed queues. Do not infer one from workflows,
/// plan names or branch protection alone. Provider errors are not absence.
/// Schema: https://docs.github.com/en/graphql/reference/repos#repository
pub(crate) fn merge_queue_enabled(
    repo: &str,
    branch: &str,
    cwd: &Path,
    binary: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    let (owner, name) = repo.split_once('/').ok_or("invalid GitHub repository")?;
    let mut command = Command::new(binary);
    command.current_dir(cwd).env("GH_HOST", "github.com").args([
        "api", "graphql", "-f",
        "query=query($owner:String!,$name:String!,$branch:String!){repository(owner:$owner,name:$name){mergeQueue(branch:$branch){id}}}",
        "-f", &format!("owner={owner}"),
        "-f", &format!("name={name}"),
        "-f", &format!("branch={branch}"),
    ]);
    let output = run_command(&mut command, Deadline::after(timeout), timeout)
        .map_err(|_| "merge queue lookup unavailable or timed out".to_string())?;
    if !output.status.success() {
        return Err("merge queue lookup failed".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "merge queue lookup returned invalid JSON")?;
    if value
        .get("errors")
        .is_some_and(|errors| errors.as_array().is_none_or(|errors| !errors.is_empty()))
    {
        return Err("merge queue lookup returned GraphQL errors".into());
    }
    match value.pointer("/data/repository/mergeQueue") {
        Some(serde_json::Value::Null) => Ok(false),
        Some(queue)
            if queue
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| !id.is_empty()) =>
        {
            Ok(true)
        }
        _ => Err("merge queue lookup omitted repository.mergeQueue".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_slug_rejects_malformed_provider_identity() {
        for slug in ["acme/repo", "Richards-LLC/cassy"] {
            assert!(valid_slug(slug));
        }
        for slug in [
            "",
            "acme",
            "acme/repo/extra",
            "acme/..",
            "acme/repo?x",
            "acme/repo\n",
        ] {
            assert!(!valid_slug(slug), "{slug}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn github_repo_redirect_uses_explicit_slug_and_validates_response() {
        let temp = tempfile::TempDir::new().unwrap();
        let gh = temp.path().join("gh");
        crate::test_paths::warm_stub(
            &gh,
            "#!/bin/sh\n[ \"$1 $2 $3\" = 'repo view pippenz/cas' ] || exit 97\nprintf '%s' '{\"nameWithOwner\":\"Richards-LLC/cassy\"}'\n",
        );
        let resolved =
            resolve_slug("pippenz/cas", temp.path(), &gh, Duration::from_secs(2)).unwrap();
        assert_eq!(resolved.canonical, "Richards-LLC/cassy");
        assert_eq!(resolved.origin, "pippenz/cas");
        crate::test_paths::warm_stub(
            &gh,
            "#!/bin/sh\nprintf '%s' '{\"nameWithOwner\":\"bad/repo/extra\"}'\n",
        );
        assert!(resolve_slug("pippenz/cas", temp.path(), &gh, Duration::from_secs(2)).is_err());
        crate::test_paths::warm_stub(
            &gh,
            "#!/bin/sh\necho 'GH_TOKEN=fixture-secret' >&2\nexit 7\n",
        );
        let failed =
            resolve_slug("pippenz/cas", temp.path(), &gh, Duration::from_secs(2)).unwrap_err();
        assert!(failed.contains("exit status: 7"), "{failed}");
        assert!(!failed.contains("fixture-secret"), "{failed}");
        let timed_out = resolve_slug("pippenz/cas", temp.path(), &gh, Duration::ZERO).unwrap_err();
        assert!(timed_out.contains("timed out"), "{timed_out}");
        assert!(!timed_out.contains("unavailable"), "{timed_out}");
        let unavailable = resolve_slug(
            "pippenz/cas",
            temp.path(),
            &temp.path().join("missing-gh"),
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(unavailable.contains("unavailable"), "{unavailable}");
        assert!(!unavailable.contains("timed out"), "{unavailable}");
    }

    #[test]
    fn github_origin_inspection_distinguishes_git_failure_from_absent_origin_cas_e1ca() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .current_dir(temp.path())
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            origin_slug(temp.path(), Duration::from_secs(2)).unwrap(),
            None
        );
        std::fs::write(temp.path().join(".git/config"), "[broken\n").unwrap();
        let failed = origin_slug(temp.path(), Duration::from_secs(2)).unwrap_err();
        assert!(failed.contains("GitHub origin lookup failed"), "{failed}");
        assert!(failed.contains("128"), "{failed}");
        let timed_out = origin_slug(temp.path(), Duration::ZERO).unwrap_err();
        assert!(timed_out.contains("timed out"), "{timed_out}");
    }
}
