//! Best-effort GitHub issue intake summary for supervisor SessionStart.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{Config, IssueRepoRegistry};
use crate::gh_graphql;

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
///
/// Returns full + compact renderings for the SessionStart size budget
/// (cas-b114).
///
/// The compact form keeps the repo and open-issue count and drops the
/// per-issue list, which is the only part that grows.
pub(crate) fn build_session_start_banner_sized(
    cas_root: &Path,
    config: &Config,
) -> Option<crate::hooks::handlers::session_hygiene::SessionStartBanner> {
    let repo = configured_repo(config)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let cache_path = cas_root.join(CACHE_FILE);

    let cache = read_fresh_cache(&cache_path, repo, now)
        .or_else(|| fetch_issues(repo, now).inspect(|cache| write_cache(&cache_path, cache)))?;

    let registry = config.issue_repo_registry();
    let full = format!(
        "{}\n\n{}",
        render_banner(&cache, now),
        render_issue_repo_registry(&registry)
    );
    let compact = format!(
        "## GitHub issue triage — {}\n{} open — run `gh issue list --repo {}` for the list.",
        cache.repo, cache.total_count, cache.repo
    ) + "\n\n"
        + &render_issue_repo_registry(&registry);
    Some(crate::hooks::handlers::session_hygiene::SessionStartBanner { full, compact })
}

fn configured_repo(config: &Config) -> Option<&str> {
    let repo = config.issues.as_ref()?.repo.as_deref()?.trim();
    // Validation lives in `gh_graphql` so the banner and the history indexer
    // cannot drift on what counts as a legal `issues.repo` (spec §1.6: one
    // source of the owner/name).
    gh_graphql::split_repo(repo).ok()?;
    Some(repo)
}

fn read_fresh_cache(path: &Path, repo: &str, now: u64) -> Option<IssueCache> {
    let cache: IssueCache = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let age = now.checked_sub(cache.fetched_at_unix_secs)?;
    (cache.version == CACHE_VERSION && cache.repo == repo && age <= CACHE_TTL_SECS).then_some(cache)
}

/// Fetch the banner's three-issue summary over the shared `gh api graphql`
/// path (`gh_graphql`), keeping this handler's own one-second budget: the
/// banner sits in a SessionStart critical path and must never make a session
/// wait on the network.
fn fetch_issues(repo: &str, now: u64) -> Option<IssueCache> {
    let (owner, name) = gh_graphql::split_repo(repo).ok()?;
    let data = gh_graphql::run_graphql(
        ISSUE_QUERY,
        &[
            ("owner", owner.to_string()),
            ("name", name.to_string()),
        ],
        GH_TIMEOUT,
    )
    .ok()?;

    let repository: Option<GraphQlRepository> =
        serde_json::from_value(data.get("repository")?.clone()).ok()?;
    let issues = repository?.issues;
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

fn render_issue_repo_registry(registry: &IssueRepoRegistry) -> String {
    format!(
        "## Where to file bugs\n\
- project: {} — the current project's own issue tracker\n\
- cassy: {} — Cassy runtime, hooks, MCP, factory, and skills\n\
- mecha_cassy: {} — MechaCassy Slack hub and message delivery\n\
- cloud: {} — Cassy Cloud sync, hub relay, and pairing\n\
If you hit a bug during operation, file a ticket in the matching repo before moving on.",
        registry.project.as_deref().unwrap_or("<unset>"),
        registry.cassy,
        registry.mecha_cassy,
        registry.cloud,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IssuesConfig;

    fn config(repo: &str) -> Config {
        Config {
            issues: Some(IssuesConfig {
                repo: Some(repo.to_string()),
                ..IssuesConfig::default()
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
    fn triage_banner_includes_all_issue_destinations_and_operational_directive() {
        let registry = config("owner/repo").issue_repo_registry();
        let rendered = render_issue_repo_registry(&registry);
        for repo in [
            "owner/repo",
            "Richards-LLC/cassy",
            "Richards-LLC/mecha-cassy",
            "Richards-LLC/petra-stella-cloud",
        ] {
            assert!(rendered.contains(repo), "missing {repo}: {rendered}");
        }
        assert!(rendered.contains(
            "If you hit a bug during operation, file a ticket in the matching repo before moving on."
        ));
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
