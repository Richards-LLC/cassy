//! Best-effort GitHub issue intake summary for supervisor SessionStart.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::bounded_process::{Deadline, run_command};
use crate::config::Config;

const CACHE_VERSION: u8 = 1;
const CACHE_FILE: &str = "github-issue-triage-cache.json";
const CACHE_TTL_SECS: u64 = 5 * 60;
const GH_TIMEOUT: Duration = Duration::from_secs(1);
const RECENT_ISSUE_LIMIT: usize = 3;
const MAX_TITLE_BYTES: usize = 120;

const ISSUE_QUERY: &str = r#"
query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    issues(states: OPEN, first: 3, orderBy: {field: CREATED_AT, direction: DESC}) {
      totalCount
      nodes { number title }
    }
  }
}
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IssueSummary {
    number: u64,
    title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IssueCache {
    version: u8,
    repo: String,
    fetched_at_unix_secs: u64,
    total_count: u64,
    issues: Vec<IssueSummary>,
}

#[derive(Deserialize)]
struct GraphQlResponse {
    data: Option<GraphQlData>,
}

#[derive(Deserialize)]
struct GraphQlData {
    repository: Option<GraphQlRepository>,
}

#[derive(Deserialize)]
struct GraphQlRepository {
    issues: GraphQlIssues,
}

#[derive(Deserialize)]
struct GraphQlIssues {
    #[serde(rename = "totalCount")]
    total_count: u64,
    nodes: Vec<IssueSummary>,
}

/// Build the supervisor issue-triage banner, or return `None` for every
/// disabled/failure case. A displayed cache is at most five minutes old;
/// once stale, a failed refresh is not replaced with older data.
pub(crate) fn build_session_start_banner(cas_root: &Path, config: &Config) -> Option<String> {
    let repo = configured_repo(config)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let cache_path = cas_root.join(CACHE_FILE);

    let cache = read_fresh_cache(&cache_path, repo, now)
        .or_else(|| fetch_issues(repo, now).inspect(|cache| write_cache(&cache_path, cache)))?;

    Some(render_banner(&cache, now))
}

fn configured_repo(config: &Config) -> Option<&str> {
    let repo = config.issues.as_ref()?.repo.as_deref()?.trim();
    let (owner, name) = repo.split_once('/')?;
    if name.contains('/') || !valid_repo_part(owner) || !valid_repo_part(name) {
        return None;
    }
    Some(repo)
}

fn valid_repo_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn read_fresh_cache(path: &Path, repo: &str, now: u64) -> Option<IssueCache> {
    let cache: IssueCache = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let age = now.checked_sub(cache.fetched_at_unix_secs)?;
    (cache.version == CACHE_VERSION && cache.repo == repo && age <= CACHE_TTL_SECS).then_some(cache)
}

fn fetch_issues(repo: &str, now: u64) -> Option<IssueCache> {
    let (owner, name) = repo.split_once('/')?;
    let output = run_command(
        Command::new("gh").args([
            "api",
            "graphql",
            "-f",
            &format!("query={ISSUE_QUERY}"),
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("name={name}"),
        ]),
        Deadline::after(GH_TIMEOUT),
        GH_TIMEOUT,
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }

    let response: GraphQlResponse = serde_json::from_slice(&output.stdout).ok()?;
    let issues = response.data?.repository?.issues;
    Some(IssueCache {
        version: CACHE_VERSION,
        repo: repo.to_string(),
        fetched_at_unix_secs: now,
        total_count: issues.total_count,
        issues: issues
            .nodes
            .into_iter()
            .take(RECENT_ISSUE_LIMIT)
            .map(|issue| IssueSummary {
                number: issue.number,
                title: sanitize_title(&issue.title),
            })
            .collect(),
    })
}

fn write_cache(path: &Path, cache: &IssueCache) {
    let Ok(body) = serde_json::to_vec(cache) else {
        return;
    };
    let temp_path = cache_temp_path(path);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let wrote_temp = options
        .open(&temp_path)
        .and_then(|mut file| file.write_all(&body))
        .is_ok();
    if wrote_temp && fs::rename(&temp_path, path).is_err() {
        // Windows does not replace an existing destination. The cache is
        // disposable, so a brief remove/rename window is preferable to a
        // permanently stale cache that forces a network call every session.
        let _ = fs::remove_file(path);
        let _ = fs::rename(&temp_path, path);
    }
    if temp_path.exists() {
        let _ = fs::remove_file(temp_path);
    }
}

fn cache_temp_path(path: &Path) -> PathBuf {
    let file_name = format!(".{CACHE_FILE}.{}.tmp", std::process::id());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

fn sanitize_title(title: &str) -> String {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    super::truncate_display(&normalized, MAX_TITLE_BYTES)
}

fn render_banner(cache: &IssueCache, now: u64) -> String {
    let age_secs = now.saturating_sub(cache.fetched_at_unix_secs);
    let age = if age_secs < 60 {
        "just now".to_string()
    } else {
        format!("{}m ago", age_secs / 60)
    };
    let mut banner = format!(
        "## GitHub issue triage — {}\n{} open (checked {age}; cache max age 5m)",
        cache.repo, cache.total_count
    );
    for issue in &cache.issues {
        banner.push_str(&format!("\n- #{} {}", issue.number, issue.title));
    }
    banner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IssuesConfig;

    fn config(repo: &str) -> Config {
        Config {
            issues: Some(IssuesConfig {
                repo: Some(repo.to_string()),
            }),
            ..Config::default()
        }
    }

    #[test]
    fn repo_target_is_exactly_the_shared_issues_repo_key() {
        assert_eq!(configured_repo(&config("owner/repo")), Some("owner/repo"));
        assert_eq!(configured_repo(&Config::default()), None);
        assert_eq!(configured_repo(&config("owner/repo/extra")), None);
        assert_eq!(configured_repo(&config("owner/repo\nspoof")), None);
    }

    #[test]
    fn cache_is_bounded_by_repo_version_and_five_minute_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CACHE_FILE);
        let cache = IssueCache {
            version: CACHE_VERSION,
            repo: "owner/repo".to_string(),
            fetched_at_unix_secs: 1_000,
            total_count: 1,
            issues: vec![],
        };
        fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();

        assert!(read_fresh_cache(&path, "owner/repo", 1_300).is_some());
        assert!(read_fresh_cache(&path, "owner/repo", 1_301).is_none());
        assert!(read_fresh_cache(&path, "other/repo", 1_100).is_none());
        assert!(read_fresh_cache(&path, "owner/repo", 999).is_none());
    }

    #[test]
    fn titles_cannot_inject_extra_session_start_lines() {
        assert_eq!(sanitize_title("one\n\n- fake\trow"), "one - fake row");
    }
}
