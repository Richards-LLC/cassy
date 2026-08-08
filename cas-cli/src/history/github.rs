//! Incremental GitHub issue/PR/comment indexer (EPIC cas-6212 / cas-9a38,
//! spec §8).
//!
//! Extends the existing `gh api graphql` acquisition path (`crate::gh_graphql`,
//! factored out of the SessionStart triage banner) rather than adding a second
//! GitHub client. One binary, one `issues.repo` key, one failure taxonomy.
//!
//! # Incrementality, and why the cursor is a data timestamp
//!
//! `history_index_state('github').last_indexed_at` holds the newest `updatedAt`
//! this indexer has ever *stored*. Each pass asks GitHub for items updated at
//! or after that instant, newest first, and stops as soon as it sees one that
//! is not newer than the cursor. Two consequences worth stating:
//!
//! - The cursor comes from GitHub's own timestamps, never from this machine's
//!   clock. If the two disagree, the worst case is re-fetching the boundary
//!   item — idempotent, because the upsert is keyed on the doc id. Using local
//!   time would instead let a fast clock skip an item permanently.
//! - The comparison is `>=`, not `>`. Two items can share an `updatedAt` to the
//!   second; a strict `>` would drop the second one forever.
//!
//! # Issues and pull requests are fetched differently, on purpose
//!
//! GitHub's `issues` connection accepts `filterBy: {since:}`, so the server
//! does the filtering. `pullRequests` has no such argument — so PRs are fetched
//! newest-updated-first and this module stops paging at the cursor itself. The
//! asymmetry is GitHub's, not ours; hiding it behind a uniform-looking helper
//! would hide the fact that the PR half pays for one extra page.
//!
//! # PR ↔ commit edges (spec §8)
//!
//! A PR node carries `mergeCommit.oid` and its `commits` list. Both are full
//! 40-char SHAs straight from the API, and they land in
//! `history_docs.refs_json` as *structured* references — the "which PR shipped
//! this commit" edge with no heuristics and no prefix matching.
//!
//! # Boundaries, never silent partials (spec §8, §10.2)
//!
//! Absent `gh`, an unauthenticated `gh`, an unset `issues.repo`, a timeout, a
//! GraphQL error: each is returned as a named boundary, recorded in
//! `history_index_state('github').last_error`, and surfaced by
//! `cas history status`. None of them is an empty success, and none of them
//! stops the git half of the index from running.

use std::path::Path;
use std::time::Duration;

use cas_store::{
    DOC_KIND_COMMENT, DOC_KIND_ISSUE, DOC_KIND_PR, HistoryDoc, HistoryStore, SOURCE_GITHUB,
    SqliteHistoryStore,
};
use serde_json::Value;

use crate::gh_graphql::{GhCliTransport, GhError, GraphQlTransport};

use super::refs::extract_from_text;

/// Items per GraphQL page. GitHub's hard ceiling is 100; 50 keeps the response
/// small enough that a page with long issue bodies plus their comments does not
/// blow the point budget for one call.
const PAGE_SIZE: usize = 50;

/// Comments requested per issue/PR. See [`FetchOutcome::comments_truncated`]:
/// exceeding this is reported, not silently dropped.
const COMMENT_PAGE_SIZE: usize = 100;

/// Pages per pass, per connection. A stop that is only a cursor comparison can
/// in principle walk an entire repository; this bounds one tick's work. Hitting
/// it is reported (see [`FetchOutcome::page_limit_hit`]) and simply means the
/// next tick resumes from the advanced cursor.
const MAX_PAGES: usize = 40;

/// Per-`gh`-call budget. Far above the banner's one second, because this runs
/// on a 15-minute daemon tick where a slow page costs nothing but a slow-page
/// *timeout* costs a whole pass.
pub const GH_CALL_TIMEOUT: Duration = Duration::from_secs(30);

const ISSUE_QUERY: &str = r#"
query($owner: String!, $name: String!, $first: Int!, $comments: Int!, $after: String, $since: DateTime) {
  repository(owner: $owner, name: $name) {
    issues(first: $first, after: $after, states: [OPEN, CLOSED],
           orderBy: {field: UPDATED_AT, direction: DESC},
           filterBy: {since: $since}) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title body state url createdAt updatedAt closedAt
        author { login }
        comments(first: $comments) {
          totalCount
          nodes { id body url createdAt updatedAt author { login } }
        }
      }
    }
  }
}
"#;

const PR_QUERY: &str = r#"
query($owner: String!, $name: String!, $first: Int!, $comments: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequests(first: $first, after: $after, states: [OPEN, CLOSED, MERGED],
                 orderBy: {field: UPDATED_AT, direction: DESC}) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title body state url createdAt updatedAt closedAt mergedAt
        author { login }
        mergeCommit { oid }
        commits(first: 100) { totalCount nodes { commit { oid } } }
        comments(first: $comments) {
          totalCount
          nodes { id body url createdAt updatedAt author { login } }
        }
      }
    }
  }
}
"#;

/// What one GitHub pass actually did. Every count here is reported by
/// `cas history docs`, so a pass can never look more complete than it was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchOutcome {
    pub issues: usize,
    pub pull_requests: usize,
    pub comments: usize,
    /// Pages actually requested, across both connections.
    pub pages: usize,
    /// The cursor this pass will store, i.e. the newest `updatedAt` seen.
    /// `None` when nothing was new.
    pub cursor: Option<String>,
    /// The cursor the pass started from. `None` on a first (full) run.
    pub since: Option<String>,
    /// Items whose comment list exceeded [`COMMENT_PAGE_SIZE`] and was
    /// therefore truncated. Non-zero means the index is knowingly incomplete
    /// for those items — reported rather than absorbed.
    pub comments_truncated: usize,
    /// True when [`MAX_PAGES`] stopped the walk before the cursor did.
    pub page_limit_hit: bool,
}

impl FetchOutcome {
    pub fn docs_total(&self) -> usize {
        self.issues + self.pull_requests + self.comments
    }

    /// A first run with no prior cursor is a backfill.
    pub fn is_backfill(&self) -> bool {
        self.since.is_none()
    }
}

/// Fetch every issue/PR/comment updated at or after `since`.
///
/// Pure with respect to the network: the transport is injected, which is what
/// lets the incrementality contract be tested against recorded responses.
pub fn fetch(
    transport: &dyn GraphQlTransport,
    owner: &str,
    name: &str,
    repository: &str,
    since: Option<&str>,
) -> Result<(Vec<HistoryDoc>, FetchOutcome), GhError> {
    let mut docs = Vec::new();
    let mut outcome = FetchOutcome {
        since: since.map(str::to_string),
        ..FetchOutcome::default()
    };

    // --- issues: server-side `since` filter ---------------------------------
    let mut after: Option<String> = None;
    for page in 0..MAX_PAGES {
        let mut vars = base_vars(owner, name);
        if let Some(cursor) = &after {
            vars.push(("after", cursor.clone()));
        }
        if let Some(s) = since {
            vars.push(("since", s.to_string()));
        }
        let data = transport.run(ISSUE_QUERY, &vars)?;
        outcome.pages += 1;

        let connection = connection(&data, "issues")?;
        let stop = collect_page(
            connection,
            repository,
            DOC_KIND_ISSUE,
            since,
            &mut docs,
            &mut outcome,
        );

        match next_cursor(connection) {
            Some(cursor) if !stop => after = Some(cursor),
            _ => break,
        }
        if page + 1 == MAX_PAGES {
            outcome.page_limit_hit = true;
        }
    }

    // --- pull requests: no server-side filter; stop at the cursor -----------
    let mut after: Option<String> = None;
    for page in 0..MAX_PAGES {
        let mut vars = base_vars(owner, name);
        if let Some(cursor) = &after {
            vars.push(("after", cursor.clone()));
        }
        let data = transport.run(PR_QUERY, &vars)?;
        outcome.pages += 1;

        let connection = connection(&data, "pullRequests")?;
        let stop = collect_page(
            connection,
            repository,
            DOC_KIND_PR,
            since,
            &mut docs,
            &mut outcome,
        );

        match next_cursor(connection) {
            Some(cursor) if !stop => after = Some(cursor),
            _ => break,
        }
        if page + 1 == MAX_PAGES {
            outcome.page_limit_hit = true;
        }
    }

    Ok((docs, outcome))
}

fn base_vars<'a>(owner: &str, name: &str) -> Vec<(&'a str, String)> {
    vec![
        ("owner", owner.to_string()),
        ("name", name.to_string()),
        ("first", PAGE_SIZE.to_string()),
        ("comments", COMMENT_PAGE_SIZE.to_string()),
    ]
}

fn connection<'a>(data: &'a Value, field: &str) -> Result<&'a Value, GhError> {
    data.get("repository")
        .filter(|r| !r.is_null())
        .and_then(|r| r.get(field))
        .filter(|c| !c.is_null())
        .ok_or_else(|| {
            GhError::MalformedResponse(format!("response carried no repository.{field}"))
        })
}

fn next_cursor(connection: &Value) -> Option<String> {
    let info = connection.get("pageInfo")?;
    info.get("hasNextPage")?
        .as_bool()
        .unwrap_or(false)
        .then(|| info.get("endCursor").and_then(|c| c.as_str()))
        .flatten()
        .map(str::to_string)
}

/// Turn one page of nodes into docs.
///
/// Returns `true` when a node older than the cursor was reached, which is the
/// signal to stop paging.
fn collect_page(
    connection: &Value,
    repository: &str,
    kind: &str,
    since: Option<&str>,
    docs: &mut Vec<HistoryDoc>,
    outcome: &mut FetchOutcome,
) -> bool {
    let Some(nodes) = connection.get("nodes").and_then(|n| n.as_array()) else {
        return true;
    };

    for node in nodes {
        let updated = str_field(node, "updatedAt");
        // `>=`, not `>`: two items can share a second, and a strict comparison
        // would drop the second one permanently.
        if let (Some(cursor), Some(updated)) = (since, updated.as_deref())
            && updated < cursor
        {
            return true;
        }

        let Some(number) = node.get("number").and_then(Value::as_i64) else {
            continue;
        };
        advance_cursor(outcome, updated.as_deref());

        let mut refs = extract_from_text(&format!(
            "{}\n{}",
            str_field(node, "title").unwrap_or_default(),
            str_field(node, "body").unwrap_or_default()
        ));
        if kind == DOC_KIND_PR {
            refs.merge_commit = node
                .get("mergeCommit")
                .and_then(|m| m.get("oid"))
                .and_then(Value::as_str)
                .map(str::to_string);
            refs.pr_commits = node
                .get("commits")
                .and_then(|c| c.get("nodes"))
                .and_then(Value::as_array)
                .map(|nodes| {
                    nodes
                        .iter()
                        .filter_map(|n| n.get("commit")?.get("oid")?.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
        }

        let id_prefix = if kind == DOC_KIND_PR { "pr" } else { "issue" };
        docs.push(HistoryDoc {
            id: format!("gh:{id_prefix}:{number}"),
            doc_kind: kind.to_string(),
            number: Some(number),
            title: str_field(node, "title"),
            body: str_field(node, "body"),
            // A merged PR reports `state: MERGED`; keeping GitHub's own word
            // avoids inventing a vocabulary that has to be mapped back later.
            state: str_field(node, "state"),
            author: node
                .get("author")
                .and_then(|a| a.get("login"))
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at: str_field(node, "createdAt"),
            updated_at: updated,
            closed_at: str_field(node, "closedAt"),
            url: str_field(node, "url"),
            refs_json: refs.to_json(),
            repository: repository.to_string(),
            source: SOURCE_GITHUB.to_string(),
        });
        match kind {
            DOC_KIND_PR => outcome.pull_requests += 1,
            _ => outcome.issues += 1,
        }

        collect_comments(node, number, repository, docs, outcome);
    }
    false
}

fn collect_comments(
    node: &Value,
    number: i64,
    repository: &str,
    docs: &mut Vec<HistoryDoc>,
    outcome: &mut FetchOutcome,
) {
    let Some(comments) = node.get("comments") else {
        return;
    };
    let total = comments
        .get("totalCount")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let nodes = comments
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if total > nodes.len() as i64 {
        outcome.comments_truncated += 1;
    }

    for comment in nodes {
        let Some(id) = comment.get("id").and_then(Value::as_str) else {
            continue;
        };
        let updated = str_field(&comment, "updatedAt");
        advance_cursor(outcome, updated.as_deref());
        let refs = extract_from_text(str_field(&comment, "body").unwrap_or_default().as_str());
        docs.push(HistoryDoc {
            id: format!("gh:comment:{id}"),
            doc_kind: DOC_KIND_COMMENT.to_string(),
            // The parent issue/PR number: a comment is only meaningful next to
            // the thread it belongs to, and this is the join to it.
            number: Some(number),
            title: None,
            body: str_field(&comment, "body"),
            state: None,
            author: comment
                .get("author")
                .and_then(|a| a.get("login"))
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at: str_field(&comment, "createdAt"),
            updated_at: updated,
            closed_at: None,
            url: str_field(&comment, "url"),
            refs_json: refs.to_json(),
            repository: repository.to_string(),
            source: SOURCE_GITHUB.to_string(),
        });
        outcome.comments += 1;
    }
}

/// Keep the newest timestamp seen. RFC3339 UTC strings from GitHub are all
/// `Z`-suffixed and fixed-width, so lexicographic order is chronological order.
fn advance_cursor(outcome: &mut FetchOutcome, updated: Option<&str>) {
    let Some(updated) = updated else { return };
    if outcome.cursor.as_deref().is_none_or(|c| updated > c) {
        outcome.cursor = Some(updated.to_string());
    }
}

fn str_field(node: &Value, field: &str) -> Option<String> {
    node.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Run one GitHub indexing pass against the live `gh` CLI and store the result.
///
/// The `Err` arm is always a *declared boundary*: it is recorded on the
/// `github` state row before it propagates, so `cas history status` can report
/// it. Callers that must not fail on absent GitHub data (the daemon, and
/// `cas history docs` running offline) treat it as a boundary rather than an
/// error — see [`is_boundary`].
pub fn run_pass(
    cas_root: &Path,
    repo_root: &Path,
    repo: &str,
    force: bool,
) -> Result<FetchOutcome, GhError> {
    let store = SqliteHistoryStore::open(cas_root)
        .map_err(|e| GhError::MalformedResponse(format!("opening history store: {e}")))?;
    let repository = super::repository_id(repo_root);
    let transport = GhCliTransport::new(GH_CALL_TIMEOUT);

    match run_pass_with(&store, &transport, repo, &repository, force) {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            let _ = store.record_attempt(&repository, SOURCE_GITHUB, Some(&e.to_string()));
            Err(e)
        }
    }
}

/// Transport-injected core of [`run_pass`], so the incrementality contract can
/// be exercised against recorded responses.
pub fn run_pass_with(
    store: &SqliteHistoryStore,
    transport: &dyn GraphQlTransport,
    repo: &str,
    repository: &str,
    force: bool,
) -> Result<FetchOutcome, GhError> {
    let (owner, name) = crate::gh_graphql::split_repo(repo)?;

    let state = store
        .index_state(repository, SOURCE_GITHUB)
        .map_err(|e| GhError::MalformedResponse(format!("reading github watermark: {e}")))?;
    // `--force` discards the cursor for this pass only; the stored one is
    // overwritten by whatever this pass observes, never merely cleared, so a
    // failed forced run cannot leave the index with no cursor at all.
    let since = (!force)
        .then(|| state.as_ref().and_then(|s| s.last_indexed_at.clone()))
        .flatten();

    let (docs, outcome) = fetch(transport, owner, name, repository, since.as_deref())?;

    store
        .upsert_docs(
            repository,
            SOURCE_GITHUB,
            &docs,
            outcome.cursor.as_deref(),
            !outcome.page_limit_hit,
        )
        .map_err(|e| GhError::MalformedResponse(format!("writing history docs: {e}")))?;

    Ok(outcome)
}

/// Whether a failure is an "absent GitHub data" boundary that the rest of the
/// index must survive (spec §10.2), rather than a bug.
///
/// Everything `gh` can tell us falls in this bucket — including a GraphQL
/// refusal, which is what an unauthenticated or unauthorised token produces.
/// The distinction that matters is not severity but *scope*: none of these
/// says anything about the git half of the index, so none of them may stop it.
pub fn is_boundary(error: &GhError) -> bool {
    matches!(
        error,
        GhError::RepoNotConfigured
            | GhError::GhUnavailable
            | GhError::CallFailed(_)
            | GhError::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::refs::DocRefs;
    use std::cell::RefCell;

    /// A transport that replays recorded GraphQL responses and records the
    /// variables it was asked for — which is how the incremental contract is
    /// asserted without a network.
    struct Recorded {
        issue_pages: RefCell<Vec<Value>>,
        pr_pages: RefCell<Vec<Value>>,
        calls: RefCell<Vec<(String, Vec<(String, String)>)>>,
    }

    impl Recorded {
        fn new(issue_pages: Vec<Value>, pr_pages: Vec<Value>) -> Self {
            Self {
                issue_pages: RefCell::new(issue_pages),
                pr_pages: RefCell::new(pr_pages),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn since_arguments(&self) -> Vec<Option<String>> {
            self.calls
                .borrow()
                .iter()
                .filter(|(kind, _)| kind == "issues")
                .map(|(_, vars)| {
                    vars.iter()
                        .find(|(k, _)| k == "since")
                        .map(|(_, v)| v.clone())
                })
                .collect()
        }
    }

    impl GraphQlTransport for Recorded {
        fn run(&self, query: &str, variables: &[(&str, String)]) -> Result<Value, GhError> {
            let kind = if query.contains("pullRequests") {
                "pullRequests"
            } else {
                "issues"
            };
            self.calls.borrow_mut().push((
                kind.to_string(),
                variables
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), v.clone()))
                    .collect(),
            ));
            let mut pages = if kind == "issues" {
                self.issue_pages.borrow_mut()
            } else {
                self.pr_pages.borrow_mut()
            };
            if pages.is_empty() {
                return Ok(empty_page(kind));
            }
            Ok(pages.remove(0))
        }
    }

    fn empty_page(field: &str) -> Value {
        serde_json::json!({
            "repository": { field: {
                "pageInfo": {"hasNextPage": false, "endCursor": Value::Null},
                "nodes": []
            }}
        })
    }

    fn issue_page(nodes: Value, has_next: bool, cursor: &str) -> Value {
        serde_json::json!({
            "repository": { "issues": {
                "pageInfo": {"hasNextPage": has_next, "endCursor": cursor},
                "nodes": nodes
            }}
        })
    }

    fn pr_page(nodes: Value) -> Value {
        serde_json::json!({
            "repository": { "pullRequests": {
                "pageInfo": {"hasNextPage": false, "endCursor": Value::Null},
                "nodes": nodes
            }}
        })
    }

    fn issue_node(number: i64, updated: &str, comments: Value, total: i64) -> Value {
        serde_json::json!({
            "number": number,
            "title": format!("issue {number}"),
            "body": "fixes #1 in ab12cd3",
            "state": "OPEN",
            "url": format!("https://github.test/i/{number}"),
            "createdAt": "2026-08-01T00:00:00Z",
            "updatedAt": updated,
            "closedAt": Value::Null,
            "author": {"login": "ada"},
            "comments": {"totalCount": total, "nodes": comments},
        })
    }

    fn store() -> (tempfile::TempDir, SqliteHistoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteHistoryStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn a_first_pass_fetches_everything_and_stores_the_data_cursor() {
        let (_d, store) = store();
        let transport = Recorded::new(
            vec![issue_page(
                serde_json::json!([
                    issue_node(2, "2026-08-05T10:00:00Z", serde_json::json!([]), 0),
                    issue_node(1, "2026-08-04T10:00:00Z", serde_json::json!([]), 0),
                ]),
                false,
                "",
            )],
            vec![pr_page(serde_json::json!([]))],
        );

        let outcome = run_pass_with(&store, &transport, "owner/repo", "/repo", false).unwrap();
        assert_eq!(outcome.issues, 2);
        assert!(outcome.is_backfill());
        assert_eq!(outcome.cursor.as_deref(), Some("2026-08-05T10:00:00Z"));
        assert_eq!(
            transport.since_arguments(),
            vec![None],
            "a first pass must not send a `since`"
        );

        let state = store
            .index_state("/repo", SOURCE_GITHUB)
            .unwrap()
            .unwrap();
        assert_eq!(
            state.last_indexed_at.as_deref(),
            Some("2026-08-05T10:00:00Z"),
            "the cursor must be GitHub's timestamp, not this machine's clock"
        );
    }

    /// AC1: the second run asks only for what changed, and stores only that.
    #[test]
    fn a_second_pass_sends_the_cursor_and_stores_only_the_new_item() {
        let (_d, store) = store();

        let first = Recorded::new(
            vec![issue_page(
                serde_json::json!([issue_node(1, "2026-08-04T10:00:00Z", serde_json::json!([]), 0)]),
                false,
                "",
            )],
            vec![pr_page(serde_json::json!([]))],
        );
        run_pass_with(&store, &first, "owner/repo", "/repo", false).unwrap();

        let second = Recorded::new(
            vec![issue_page(
                serde_json::json!([issue_node(2, "2026-08-06T10:00:00Z", serde_json::json!([]), 0)]),
                false,
                "",
            )],
            vec![pr_page(serde_json::json!([]))],
        );
        let outcome = run_pass_with(&store, &second, "owner/repo", "/repo", false).unwrap();

        assert_eq!(
            second.since_arguments(),
            vec![Some("2026-08-04T10:00:00Z".to_string())],
            "the second pass must filter server-side by the stored cursor"
        );
        assert_eq!(outcome.issues, 1, "only the changed issue was indexed");
        assert!(!outcome.is_backfill());
        assert_eq!(
            store.doc_counts("/repo").unwrap(),
            vec![("issue".to_string(), 2)],
            "the untouched issue must still be in the index"
        );
        assert_eq!(
            store
                .index_state("/repo", SOURCE_GITHUB)
                .unwrap()
                .unwrap()
                .last_indexed_at
                .as_deref(),
            Some("2026-08-06T10:00:00Z")
        );
    }

    /// A pass that finds nothing must leave the cursor exactly where it was.
    #[test]
    fn an_empty_second_pass_changes_nothing() {
        let (_d, store) = store();
        let first = Recorded::new(
            vec![issue_page(
                serde_json::json!([issue_node(1, "2026-08-04T10:00:00Z", serde_json::json!([]), 0)]),
                false,
                "",
            )],
            vec![pr_page(serde_json::json!([]))],
        );
        run_pass_with(&store, &first, "owner/repo", "/repo", false).unwrap();

        let second = Recorded::new(vec![], vec![]);
        let outcome = run_pass_with(&store, &second, "owner/repo", "/repo", false).unwrap();
        assert_eq!(outcome.docs_total(), 0);
        assert_eq!(outcome.cursor, None);
        assert_eq!(
            store
                .index_state("/repo", SOURCE_GITHUB)
                .unwrap()
                .unwrap()
                .last_indexed_at
                .as_deref(),
            Some("2026-08-04T10:00:00Z")
        );
    }

    /// PRs have no server-side `since`, so paging must stop at the cursor —
    /// otherwise a delta pass walks the entire PR history every tick.
    #[test]
    fn pull_request_paging_stops_at_the_cursor() {
        let (_d, store) = store();
        let mut fresh = issue_node(9, "2026-08-06T00:00:00Z", serde_json::json!([]), 0);
        fresh["mergeCommit"] = serde_json::json!({"oid": "f".repeat(40)});
        fresh["commits"] =
            serde_json::json!({"totalCount": 1, "nodes": [{"commit": {"oid": "a".repeat(40)}}]});
        let stale = issue_node(8, "2026-08-01T00:00:00Z", serde_json::json!([]), 0);

        let transport = Recorded::new(
            vec![],
            vec![serde_json::json!({
                "repository": { "pullRequests": {
                    "pageInfo": {"hasNextPage": true, "endCursor": "next"},
                    "nodes": [fresh, stale]
                }}
            })],
        );

        // Seed a cursor without going through GitHub.
        store
            .upsert_docs("/repo", SOURCE_GITHUB, &[], Some("2026-08-04T00:00:00Z"), true)
            .unwrap();
        let outcome = run_pass_with(&store, &transport, "owner/repo", "/repo", false).unwrap();

        assert_eq!(outcome.pull_requests, 1, "the stale PR must be skipped");
        assert_eq!(
            transport
                .calls
                .borrow()
                .iter()
                .filter(|(k, _)| k == "pullRequests")
                .count(),
            1,
            "paging must stop rather than follow hasNextPage past the cursor"
        );

        let doc = store.get_doc("gh:pr:9").unwrap().unwrap();
        let refs: DocRefs = serde_json::from_str(doc.refs_json.as_deref().unwrap()).unwrap();
        assert_eq!(refs.merge_commit.as_deref(), Some("f".repeat(40).as_str()));
        assert_eq!(refs.pr_commits, vec!["a".repeat(40)]);
        assert_eq!(doc.doc_kind, "pr");
    }

    #[test]
    fn comments_become_their_own_docs_and_truncation_is_reported() {
        let (_d, store) = store();
        let comments = serde_json::json!([
            {
                "id": "C_1",
                "body": "see cas-9a38",
                "url": "https://github.test/c/1",
                "createdAt": "2026-08-02T00:00:00Z",
                "updatedAt": "2026-08-07T00:00:00Z",
                "author": {"login": "bob"}
            }
        ]);
        let transport = Recorded::new(
            vec![issue_page(
                // `totalCount` exceeds the returned nodes: truncation.
                serde_json::json!([issue_node(1, "2026-08-05T00:00:00Z", comments, 150)]),
                false,
                "",
            )],
            vec![],
        );

        let outcome = run_pass_with(&store, &transport, "owner/repo", "/repo", false).unwrap();
        assert_eq!(outcome.comments, 1);
        assert_eq!(
            outcome.comments_truncated, 1,
            "a truncated comment list must be reported, not absorbed"
        );
        assert_eq!(
            outcome.cursor.as_deref(),
            Some("2026-08-07T00:00:00Z"),
            "a comment newer than its issue must still advance the cursor"
        );

        let doc = store.get_doc("gh:comment:C_1").unwrap().unwrap();
        assert_eq!(doc.number, Some(1), "a comment must point at its thread");
        assert_eq!(doc.doc_kind, "comment");
        let refs: DocRefs = serde_json::from_str(doc.refs_json.as_deref().unwrap()).unwrap();
        assert_eq!(refs.tasks, vec!["cas-9a38".to_string()]);
    }

    #[test]
    fn issue_paging_follows_the_page_cursor() {
        let (_d, store) = store();
        let transport = Recorded::new(
            vec![
                issue_page(
                    serde_json::json!([issue_node(2, "2026-08-05T00:00:00Z", serde_json::json!([]), 0)]),
                    true,
                    "PAGE2",
                ),
                issue_page(
                    serde_json::json!([issue_node(1, "2026-08-04T00:00:00Z", serde_json::json!([]), 0)]),
                    false,
                    "",
                ),
            ],
            vec![],
        );
        let outcome = run_pass_with(&store, &transport, "owner/repo", "/repo", false).unwrap();
        assert_eq!(outcome.issues, 2);
        let after: Vec<Option<String>> = transport
            .calls
            .borrow()
            .iter()
            .filter(|(k, _)| k == "issues")
            .map(|(_, vars)| vars.iter().find(|(k, _)| k == "after").map(|(_, v)| v.clone()))
            .collect();
        assert_eq!(after, vec![None, Some("PAGE2".to_string())]);
    }

    /// `--force` re-fetches from the beginning without first destroying the
    /// cursor: a forced run that fails must not leave the index cursorless.
    #[test]
    fn force_drops_the_cursor_for_the_pass_only() {
        let (_d, store) = store();
        store
            .upsert_docs("/repo", SOURCE_GITHUB, &[], Some("2026-08-04T00:00:00Z"), true)
            .unwrap();

        let failing = Recorded::new(vec![], vec![]);
        // An empty forced pass returns no cursor; the stored one must survive.
        run_pass_with(&store, &failing, "owner/repo", "/repo", true).unwrap();
        assert_eq!(
            store
                .index_state("/repo", SOURCE_GITHUB)
                .unwrap()
                .unwrap()
                .last_indexed_at
                .as_deref(),
            Some("2026-08-04T00:00:00Z")
        );
        assert_eq!(
            failing.since_arguments(),
            vec![None],
            "`--force` must send no `since`"
        );
    }

    #[test]
    fn an_unconfigured_repo_is_a_boundary_before_any_call() {
        let (_d, store) = store();
        let transport = Recorded::new(vec![], vec![]);
        let error = run_pass_with(&store, &transport, "not-a-repo", "/repo", false).unwrap_err();
        assert_eq!(error, GhError::RepoNotConfigured);
        assert!(is_boundary(&error));
        assert!(
            transport.calls.borrow().is_empty(),
            "a malformed issues.repo must never reach the network"
        );
    }

    #[test]
    fn every_gh_failure_is_a_boundary_and_a_store_failure_is_not() {
        for error in [
            GhError::RepoNotConfigured,
            GhError::GhUnavailable,
            GhError::CallFailed("HTTP 401".into()),
            GhError::TimedOut,
        ] {
            assert!(is_boundary(&error), "{error:?} should be a boundary");
        }
        assert!(
            !is_boundary(&GhError::MalformedResponse("writing history docs: disk".into())),
            "a local store failure is a bug, not an absent-GitHub boundary"
        );
    }

    #[test]
    fn a_null_repository_is_malformed_not_empty() {
        struct NullRepo;
        impl GraphQlTransport for NullRepo {
            fn run(&self, _: &str, _: &[(&str, String)]) -> Result<Value, GhError> {
                Ok(serde_json::json!({"repository": Value::Null}))
            }
        }
        let (_d, store) = store();
        assert!(matches!(
            run_pass_with(&store, &NullRepo, "owner/repo", "/repo", false),
            Err(GhError::MalformedResponse(_))
        ));
    }
}
