//! The single `gh api graphql` acquisition path (EPIC cas-6212 / cas-9a38).
//!
//! Spec §1.6 and §8 are explicit that GitHub data has **one** way in: the
//! `gh api graphql` call the SessionStart issue-triage banner already made
//! (`hooks/handlers/issue_triage.rs`). M6 needs a much larger query than the
//! banner does, so rather than write a second GitHub client — the exact
//! duplication §1.9 forbids — the invocation is factored out here and both
//! callers share it. This is the same move M1 made with the NUL-safe git-log
//! parser (`git_log.rs`).
//!
//! # What is shared, and what is deliberately not
//!
//! Shared: the `gh` binary, the `issues.repo` owner/name split and its
//! validation, the bounded-process discipline, and the classification of
//! *why* a call failed.
//!
//! Not shared: the **timeout**, which is a per-caller policy rather than a
//! property of GitHub. The banner budgets one second because it sits in a
//! SessionStart critical path where a slow network must never delay a session;
//! the indexer runs on a 15-minute daemon tick where a one-second cap would
//! fail every page of a real fetch. Forcing one number on both would make one
//! of them wrong.
//!
//! Also not shared: the banner's five-minute JSON cache. It holds three issue
//! titles and a count — it is a *rendering* cache, not a corpus, and the
//! indexer reading it would index three issues and call the repository
//! covered. The indexer's durable equivalent is the `history_index_state`
//! cursor (§8 incrementality), which is what "one cache to extend" means once
//! the cache in question is a database row rather than a banner.

use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{Deadline, run_command};

/// Why a GitHub call did not produce data. Each variant is a *declared
/// boundary* (spec §10.2), and each is worded so the message stored in
/// `history_index_state.last_error` tells an operator what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhError {
    /// `issues.repo` is unset or malformed. Following the precedent set by the
    /// SessionStart detector (`session_hygiene.rs`), this reports and proposes
    /// nothing — guessing a repository from the git remote would index the
    /// wrong project's issues into this project's store.
    RepoNotConfigured,
    /// The `gh` binary is not installed or could not be executed.
    GhUnavailable,
    /// `gh` ran and refused: not authenticated, no access, rate limited, or a
    /// GraphQL error. The captured stderr/`errors` text is carried verbatim.
    CallFailed(String),
    /// `gh` timed out inside its budget.
    TimedOut,
    /// `gh` returned something that is not the JSON shape we asked for.
    MalformedResponse(String),
}

impl std::fmt::Display for GhError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RepoNotConfigured => write!(
                f,
                "issues.repo is not configured; set it with `cas config set issues.repo owner/name`"
            ),
            Self::GhUnavailable => write!(f, "the `gh` CLI is not available on PATH"),
            Self::CallFailed(detail) => write!(f, "gh api graphql failed: {detail}"),
            Self::TimedOut => write!(f, "gh api graphql timed out"),
            Self::MalformedResponse(detail) => {
                write!(f, "gh api graphql returned an unexpected shape: {detail}")
            }
        }
    }
}

impl std::error::Error for GhError {}

/// Validate and split `owner/name` from the shared `issues.repo` key.
///
/// The character allowlist is the banner's, unchanged: it is what stops a
/// crafted repo value from injecting extra argument-looking text.
pub fn split_repo(repo: &str) -> Result<(&str, &str), GhError> {
    let repo = repo.trim();
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(GhError::RepoNotConfigured);
    };
    if name.contains('/') || !valid_repo_part(owner) || !valid_repo_part(name) {
        return Err(GhError::RepoNotConfigured);
    }
    Ok((owner, name))
}

pub fn valid_repo_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// A GraphQL transport. The indexer takes this rather than calling `gh`
/// directly so its paging, incrementality and parsing can be tested against
/// recorded responses without a network or a `gh` binary.
pub trait GraphQlTransport {
    /// Run `query` with string variables, returning the parsed `data` object.
    fn run(&self, query: &str, variables: &[(&str, String)]) -> Result<serde_json::Value, GhError>;
}

/// The real transport: `gh api graphql`, bounded.
pub struct GhCliTransport {
    timeout: Duration,
}

impl GhCliTransport {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl GraphQlTransport for GhCliTransport {
    fn run(&self, query: &str, variables: &[(&str, String)]) -> Result<serde_json::Value, GhError> {
        run_graphql(query, variables, self.timeout)
    }
}

/// Invoke `gh api graphql` once and return its `data` object.
///
/// Variables are passed with `-F`, which is `gh`'s typed form: it sends numbers
/// and booleans as such and everything else as a string, and — the reason it is
/// used here rather than string-interpolating into the query — a variable value
/// can never be read as query syntax.
pub fn run_graphql(
    query: &str,
    variables: &[(&str, String)],
    timeout: Duration,
) -> Result<serde_json::Value, GhError> {
    let mut command = Command::new("gh");
    command.args(["api", "graphql", "-f", &format!("query={query}")]);
    for (key, value) in variables {
        command.args(["-F", &format!("{key}={value}")]);
    }

    let output = match run_command(&mut command, Deadline::after(timeout), timeout) {
        Ok(output) => output,
        Err(crate::bounded_process::BoundedCommandError::TimedOut) => return Err(GhError::TimedOut),
        Err(crate::bounded_process::BoundedCommandError::Io) => return Err(GhError::GhUnavailable),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = first_meaningful_line(&stderr)
            .or_else(|| first_meaningful_line(&stdout))
            .unwrap_or_else(|| format!("exit status {}", output.status));
        return Err(GhError::CallFailed(detail));
    }

    parse_data(&output.stdout)
}

/// Pull `data` out of a GraphQL envelope, turning a top-level `errors` array
/// into a failure rather than an empty success.
///
/// This matters because GitHub answers a partially-failed query with HTTP 200,
/// `data: null` and an `errors` array. Treating that as "no issues found" is
/// precisely the silent-partial-index outcome spec §8 forbids.
pub fn parse_data(stdout: &[u8]) -> Result<serde_json::Value, GhError> {
    let value: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|e| GhError::MalformedResponse(format!("not JSON: {e}")))?;

    if let Some(errors) = value.get("errors").and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let detail = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(GhError::CallFailed(if detail.is_empty() {
            serde_json::Value::Array(errors.clone()).to_string()
        } else {
            detail
        }));
    }

    match value.get("data") {
        Some(serde_json::Value::Null) | None => Err(GhError::MalformedResponse(
            "response carried no `data` object".to_string(),
        )),
        Some(data) => Ok(data.clone()),
    }
}

fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_validation_matches_the_banners_rules() {
        assert_eq!(split_repo("owner/repo"), Ok(("owner", "repo")));
        assert_eq!(split_repo("  owner/repo  "), Ok(("owner", "repo")));
        assert_eq!(split_repo("owner/repo/extra"), Err(GhError::RepoNotConfigured));
        assert_eq!(split_repo("owner"), Err(GhError::RepoNotConfigured));
        assert_eq!(split_repo("owner/repo\nspoof"), Err(GhError::RepoNotConfigured));
        assert_eq!(split_repo("/repo"), Err(GhError::RepoNotConfigured));
        assert_eq!(split_repo("own er/repo"), Err(GhError::RepoNotConfigured));
    }

    /// HTTP 200 with an `errors` array is a failure, not an empty result. This
    /// is the difference between "GitHub refused" and "the repo has no issues",
    /// and conflating them is a silent partial index.
    #[test]
    fn a_graphql_errors_array_is_a_failure_not_an_empty_result() {
        let body = br#"{"data": null, "errors": [{"message": "Could not resolve to a Repository"}]}"#;
        assert_eq!(
            parse_data(body),
            Err(GhError::CallFailed(
                "Could not resolve to a Repository".to_string()
            ))
        );
    }

    #[test]
    fn a_missing_data_object_is_malformed_not_empty() {
        assert!(matches!(
            parse_data(b"{}"),
            Err(GhError::MalformedResponse(_))
        ));
        assert!(matches!(
            parse_data(b"not json"),
            Err(GhError::MalformedResponse(_))
        ));
    }

    #[test]
    fn a_well_formed_response_yields_its_data_object() {
        let data = parse_data(br#"{"data": {"repository": {"issues": {"nodes": []}}}}"#).unwrap();
        assert!(data["repository"]["issues"]["nodes"].is_array());
    }

    /// Every boundary must render as an actionable sentence — these strings are
    /// what an operator reads out of `history_index_state.last_error`.
    #[test]
    fn every_boundary_renders_an_actionable_message() {
        for (error, needle) in [
            (GhError::RepoNotConfigured, "issues.repo"),
            (GhError::GhUnavailable, "gh"),
            (GhError::CallFailed("401".into()), "401"),
            (GhError::TimedOut, "timed out"),
            (GhError::MalformedResponse("x".into()), "unexpected shape"),
        ] {
            assert!(
                error.to_string().contains(needle),
                "{error:?} does not mention {needle}"
            );
        }
    }
}
