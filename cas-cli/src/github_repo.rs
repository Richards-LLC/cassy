//! Explicit GitHub repository binding for CI and diagnostics.
//! A second remote or `gh repo set-default` must never select the CI repository.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{Deadline, run_command};

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
    git.current_dir(cwd).args(["remote", "get-url", "origin"]);
    let output = run_command(&mut git, Deadline::after(timeout), timeout)
        .map_err(|_| "cannot inspect GitHub origin (bounded Git lookup unavailable)".to_string())?;
    if !output.status.success() {
        return Ok(None);
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
        .map_err(|_| "canonical GitHub repository lookup unavailable or timed out".to_string())?;
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
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nexit 1\n");
        assert!(resolve_slug("pippenz/cas", temp.path(), &gh, Duration::from_secs(2)).is_err());
    }
}
