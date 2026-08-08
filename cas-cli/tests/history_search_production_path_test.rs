//! The §6.3 acceptance gate for the code-history query surface
//! (EPIC cas-6212 / cas-7f40, M4).
//!
//! # Why this test exists, stated precisely
//!
//! Spec §6.3 records that the knowledge channel is **inert in production**:
//! every `HybridSearch` constructor passes `knowledge_store: None`, so
//! `knowledge_weight: 0.25` does nothing, and unit tests that hand-build a
//! `HybridSearch` with a store attached pass anyway. The channel looks tested
//! and is dead. §6.3 therefore makes it an acceptance requirement that "an
//! integration test must assert that the *production* construction path returns
//! a history result for a known commit — not merely that the channel returns
//! results when hand-constructed in a unit test."
//!
//! So nothing here is hand-constructed:
//!
//! - a **real** git repository with real commits;
//! - the **real** M1 walker (`cas::history::run_index_pass`) indexing it;
//! - the **real** MCP surface (`CasService::search` with `action=history`),
//!   the same dispatch a live `cas serve` runs, which builds its own
//!   `HybridSearch` internally with no help from the test.
//!
//! If someone later removes `set_history_store_from_path` from the production
//! path, every unit test in `hybrid.rs` still passes and this file fails.
//!
//! The §6.4 example queries Q2, Q3 and Q7 are exercised by name below: they are
//! the spec's flip-trigger evidence base (§12 Q6) — the queries that must work
//! on M1 data alone for the "ship M1+M4 without provenance" decision to hold.

use std::path::Path;
use std::process::Command;

use cas::mcp::{CasCore, CasService};
use cas::store::init_cas_dir;
use cas_mcp::types::SearchContextRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

/// A repository with a known, asserted-on history.
struct Fixture {
    _temp: TempDir,
    repo: std::path::PathBuf,
    cas_root: std::path::PathBuf,
    service: CasService,
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(repo: &Path, path: &str, contents: &str, message: &str) {
    let full = repo.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, contents).unwrap();
    git(repo, &["add", path]);
    git(repo, &["commit", "-m", message]);
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().to_path_buf();

        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "fixture@example.com"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        // Signing would prompt or fail in CI; the fixture only needs history.
        git(&repo, &["config", "commit.gpgsign", "false"]);

        // Three commits in two areas. The subjects deliberately avoid sharing
        // vocabulary with the paths, so a path filter cannot be accidentally
        // satisfied by the text index and vice versa.
        commit(
            &repo,
            "src/delivery/retry.rs",
            "fn retry() {}\n",
            "stop re-emitting on every poll tick",
        );
        commit(
            &repo,
            "src/delivery/retry.rs",
            "fn retry() { /* backoff */ }\n",
            "add exponential backoff to the drain",
        );
        commit(
            &repo,
            "src/ui/pane.rs",
            "fn pane() {}\n",
            "widen the transcript pane",
        );

        let cas_root = init_cas_dir(&repo).unwrap();

        // The REAL M1 indexing pass — not seeded rows.
        let outcome = cas::history::run_index_pass(&cas_root, &repo).expect("index pass");
        assert_eq!(
            outcome.commits_indexed, 3,
            "fixture history did not index as expected"
        );

        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing("history-search-test".to_string());
        Self {
            _temp: temp,
            repo,
            cas_root,
            service: CasService::new(core, None),
        }
    }

    /// The full SHA of the commit whose subject is `subject`.
    fn sha_of(&self, subject: &str) -> String {
        let out = Command::new("git")
            .args(["log", "--format=%H %s"])
            .current_dir(&self.repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|l| l.contains(subject))
            .unwrap_or_else(|| panic!("no commit with subject {subject:?}"))
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.cas_root.join("cas.db")).unwrap()
    }

    /// Seed the exact commit→task edge (`tasks.deliverables.factory_branch_anchor`).
    fn seed_anchor(&self, task_id: &str, sha: &str) {
        self.db()
            .execute(
                "UPDATE tasks SET deliverables = json_object('factory_branch_anchor', ?2)
                  WHERE id = ?1",
                rusqlite::params![task_id, sha],
            )
            .unwrap();
    }

    fn insert_task(&self, task_id: &str, title: &str) {
        self.db()
            .execute(
                "INSERT INTO tasks (id, title, status, priority, task_type, created_at, updated_at)
                 VALUES (?1, ?2, 'closed', 2, 'task', '2026-08-08T00:00:00Z', '2026-08-08T00:00:00Z')",
                rusqlite::params![task_id, title],
            )
            .unwrap();
    }

    /// Seed a `worker_git_commit` event carrying an abbreviated head_sha of the
    /// given width — the §5.2 edge, at whatever dynamic width git produced.
    fn seed_worker_event(&self, sha: &str, width: usize, session: &str) {
        self.db()
            .execute(
                "INSERT INTO events (event_type, entity_type, entity_id, summary, metadata, created_at, session_id)
                 VALUES ('worker_git_commit', 'worker', 'fixture-worker', 'final git state', ?1,
                         '2026-08-08T01:00:00Z', ?2)",
                rusqlite::params![
                    format!(r#"{{"branch":"main","head_sha":"{}"}}"#, &sha[..width]),
                    session
                ],
            )
            .unwrap();
    }

    /// Drive the real MCP dispatch and parse its JSON response.
    async fn history(&self, req: serde_json::Value) -> serde_json::Value {
        let req: SearchContextRequest =
            serde_json::from_value(req).expect("SearchContextRequest");
        let result = self
            .service
            .search(Parameters(req))
            .await
            .unwrap_or_else(|e| panic!("MCP search failed: {e}"));
        let text = result
            .content
            .iter()
            .filter_map(|c| match &c.raw {
                RawContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("non-JSON response: {e}\n{text}"))
    }
}

fn subjects(response: &serde_json::Value) -> Vec<String> {
    response["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| r["subject"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// THE GATE: the production MCP path returns a real commit for a real query.
///
/// A wiring regression (history store not attached, channel not registered,
/// action not dispatched) fails here and nowhere else.
#[tokio::test]
async fn the_production_mcp_path_returns_a_known_commit() {
    let fx = Fixture::new();
    let response = fx
        .history(serde_json::json!({"action": "history", "query": "backoff"}))
        .await;

    let subjects = subjects(&response);
    assert_eq!(
        subjects.len(),
        1,
        "production path returned {subjects:?} for a term in exactly one commit"
    );
    assert_eq!(subjects[0], "add exponential backoff to the drain");

    let hit = &response["results"][0];
    assert_eq!(hit["sha"].as_str().unwrap().len(), 40, "full SHA expected");
    assert!(
        hit["score"].as_f64().unwrap() > 0.0,
        "a zero score is what an inert channel returns"
    );
    assert!(
        !hit["files"].as_array().unwrap().is_empty(),
        "the structural diff must come back with the commit"
    );
}

/// §6.4 Q2 — "What changed in <area> in the last two weeks?"
/// Structural + temporal, no text and no embedding dependence.
#[tokio::test]
async fn q2_what_changed_in_this_area_recently() {
    let fx = Fixture::new();
    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "path": "src/delivery",
            "since": "14d",
        }))
        .await;

    let subjects = subjects(&response);
    assert_eq!(
        subjects.len(),
        2,
        "expected both delivery commits, got {subjects:?}"
    );
    assert!(subjects.iter().all(|s| !s.contains("pane")));
    // The window is echoed back resolved, so a caller can tell what it covered.
    assert!(response["filters"]["since"].is_string());

    // A window that predates the fixture must return nothing rather than
    // silently ignoring the bound.
    let empty = fx
        .history(serde_json::json!({
            "action": "history",
            "path": "src/delivery",
            "until": "2020-01-01",
        }))
        .await;
    assert_eq!(empty["count"], 0);
}

/// §6.4 Q3 — "Show me every commit that touched <file>, and what it belonged
/// to." The path half is answered; the provenance half is declared unsupported
/// with the measured coverage beside it, never faked.
#[tokio::test]
async fn q3_every_commit_touching_a_path_with_honest_provenance() {
    let fx = Fixture::new();
    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "path": "src/delivery/retry.rs",
            "include_provenance": true,
        }))
        .await;

    assert_eq!(response["count"], 2);
    for hit in response["results"].as_array().unwrap() {
        // Spec §6.4 Q3's load-bearing requirement: a commit with NO populated
        // edge is still in the answer, carrying the reason it has none. This
        // fixture seeds no edges, so every hit exercises exactly that path.
        assert!(
            hit["provenance"].as_array().unwrap().is_empty(),
            "an unlinked commit must be returned with an empty edge list"
        );
        assert!(
            hit["provenance_reason"]
                .as_str()
                .unwrap()
                .contains("no populated edge"),
            "an empty provenance with no reason is indistinguishable from a bug: {hit}"
        );
        // Files are narrowed to the queried path.
        for file in hit["files"].as_array().unwrap() {
            assert!(
                file["file_path"]
                    .as_str()
                    .unwrap()
                    .contains("src/delivery/retry.rs")
            );
        }
    }

    let status = &response["index_status"];
    assert_eq!(
        status["provenance_supported"], true,
        "M5 (cas-519f) resolves provenance; the surface must say so"
    );
    // Supported is not the same claim as complete: the coverage numbers are
    // still measured and still reported, split by confidence (spec §10.1).
    assert!(
        status["provenance_coverage_pct"].is_number(),
        "coverage must be measured even when it is 0: {status}"
    );
    assert!(status["provenance_any_coverage_pct"].is_number());

    assert!(
        response["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["feature"] != "include_provenance"),
        "a supported filter must not be declared unsupported"
    );
}

/// §6.4 Q7 — "What files does a change to X usually come with?" Pure SQL
/// co-change over the structural index.
#[tokio::test]
async fn q7_co_change_over_the_structural_index() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "fixture@example.com"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);

    // scorer.rs changes twice with hybrid.rs and once with docs.md.
    for (n, extra) in [(1, "hybrid.rs"), (2, "hybrid.rs"), (3, "docs.md")] {
        std::fs::write(repo.join("scorer.rs"), format!("// {n}\n")).unwrap();
        std::fs::write(repo.join(extra), format!("// {n}\n")).unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", &format!("change {n}")]);
    }

    let cas_root = init_cas_dir(&repo).unwrap();
    cas::history::run_index_pass(&cas_root, &repo).expect("index pass");
    let core = CasCore::with_daemon(cas_root, None, None);
    core.set_agent_id_for_testing("co-change-test".to_string());
    let service = CasService::new(core, None);

    let req: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "history",
        "path": "scorer.rs",
    }))
    .unwrap();
    let result = service.search(Parameters(req)).await.expect("search");
    let text = result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let response: serde_json::Value = serde_json::from_str(&text).unwrap();

    let co = response["co_changed_files"].as_array().unwrap();
    assert!(!co.is_empty(), "co-change returned nothing for a file that has it");
    // hybrid.rs shares commits 1 and 2 with scorer.rs; commit 3 leaves it
    // untouched, so it is not in that commit at all.
    assert_eq!(co[0]["file_path"], "hybrid.rs");
    assert_eq!(co[0]["commits_together"], 2);
    assert_eq!(co[1]["file_path"], "docs.md");
    assert_eq!(co[1]["commits_together"], 1);
    assert!(
        !co.iter().any(|c| c["file_path"] == "scorer.rs"),
        "a file must not co-change with itself"
    );
}

/// Every response carries the §6.5 status block — including one that matched
/// nothing, where the difference between "no match" and "no index" lives.
#[tokio::test]
async fn every_response_carries_the_index_status_contract() {
    let fx = Fixture::new();
    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "query": "a term that appears in no commit whatsoever",
        }))
        .await;

    assert_eq!(response["count"], 0);
    let status = &response["index_status"];
    for field in [
        "last_indexed_sha",
        "head_sha",
        "lag_commits",
        "lag_seconds",
        "backfill_complete",
        "provenance_coverage_pct",
        "provenance_supported",
        "semantic_available",
        "last_error",
    ] {
        assert!(
            status.get(field).is_some(),
            "index_status is missing the contracted field {field}"
        );
    }
    assert_eq!(status["backfill_complete"], true);
    assert_eq!(status["indexed_commits"], 3);
    assert_eq!(status["lag_commits"], 0);
    assert_eq!(
        status["lag_seconds"], 0,
        "a watermark at HEAD is zero seconds behind, not unknown"
    );
    assert_eq!(
        status["semantic_available"], false,
        "there are no history vectors until M7; claiming otherwise is the lie this field prevents"
    );
    assert!(status["last_error"].is_null());
}

/// A `kind` this surface cannot serve must not be answered with commits.
#[tokio::test]
async fn an_unsupported_kind_returns_no_results_and_says_why() {
    let fx = Fixture::new();
    let response = fx
        .history(serde_json::json!({"action": "history", "kind": "issue"}))
        .await;

    assert_eq!(
        response["count"], 0,
        "asking for issues must not be answered with commits"
    );
    assert!(
        response["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["lands_in"] == "M6")
    );
}

/// A symbol filter has nothing to filter on until M3, so it is declared rather
/// than silently dropped — a dropped filter returns a wider result set that
/// looks like a confident answer.
#[tokio::test]
async fn a_symbol_filter_is_declared_not_ignored() {
    let fx = Fixture::new();
    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "query": "backoff",
            "symbol": "retry",
        }))
        .await;

    assert!(
        response["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["feature"] == "symbol filter" && u["lands_in"] == "M3")
    );
}

/// A mistyped time bound must fail loudly. Widening silently to all of history
/// returns confident results for the wrong window.
#[tokio::test]
async fn a_malformed_time_bound_is_an_error_not_a_wider_search() {
    let fx = Fixture::new();
    let req: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "history",
        "since": "last tuesday",
    }))
    .unwrap();
    let err = fx.service.search(Parameters(req)).await;
    assert!(err.is_err(), "an unparseable window was silently accepted");
}

// =========================================================================
// M5 — provenance join through the production path (cas-519f, spec §5)
// =========================================================================

/// §6.4 Q3, now with a populated edge: the exact `factory_branch_anchor` join
/// resolves the task through the REAL MCP dispatch, carrying its link method
/// and confidence.
///
/// The §6.3 argument applies unchanged here: a unit test on the resolver would
/// pass even if `run()` never called it.
#[tokio::test]
async fn the_exact_anchor_edge_resolves_through_the_production_path() {
    let fx = Fixture::new();
    let sha = fx.sha_of("add exponential backoff to the drain");
    fx.insert_task("cas-fixture-1", "the task that shipped the backoff");
    fx.seed_anchor("cas-fixture-1", &sha);

    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "path": "src/delivery/retry.rs",
            "include_provenance": true,
        }))
        .await;

    let hit = response["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["sha"] == sha)
        .expect("the anchored commit is in the answer");
    let edges = hit["provenance"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(edges[0]["link_method"], "factory_branch_anchor");
    assert_eq!(edges[0]["confidence"], "high");
    assert_eq!(edges[0]["task_id"], "cas-fixture-1");
    assert_eq!(edges[0]["ambiguous"], false);

    // The other commit in the same answer has no edge — and is still returned
    // with its reason, in the same response. Both halves of Q3 at once.
    let unlinked = response["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["sha"] != sha)
        .expect("the unanchored commit is not dropped");
    assert!(unlinked["provenance"].as_array().unwrap().is_empty());
    assert!(unlinked["provenance_reason"].is_string());

    // Coverage is measured, not asserted: 1 of 3 indexed commits.
    let status = &response["index_status"];
    assert_eq!(status["provenance_high_confidence_links"], 1);
    assert!(
        (status["provenance_coverage_pct"].as_f64().unwrap() - 33.33).abs() < 0.1,
        "coverage must reflect the seeded reality: {status}"
    );
}

/// §5.2 consequence 1, through production: a 7-char abbreviation resolves.
///
/// This is the width `sha[0..8]` silently drops, and 594 of the 1,018 usable
/// rows on the live database carry it.
#[tokio::test]
async fn a_seven_char_worker_event_prefix_resolves_through_the_production_path() {
    let fx = Fixture::new();
    let sha = fx.sha_of("widen the transcript pane");
    fx.seed_worker_event(&sha, 7, "session-seven");

    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "path": "src/ui/pane.rs",
            "include_provenance": true,
        }))
        .await;

    let hit = &response["results"].as_array().unwrap()[0];
    assert_eq!(hit["sha"], sha);
    let edge = &hit["provenance"].as_array().unwrap()[0];
    assert_eq!(edge["link_method"], "worker_git_commit_prefix");
    assert_eq!(edge["session_id"], "session-seven");
    assert_eq!(edge["confidence"], "high", "unique prefix, no collision");
    assert_eq!(edge["ambiguous"], false);
    assert_eq!(
        edge["matched_prefix"].as_str().unwrap().len(),
        7,
        "the join must report the width it actually matched on"
    );
}

/// The `task_id` filter narrows to the task's commits, in SQL, before LIMIT —
/// and an unknown task returns nothing rather than everything.
///
/// The second half is the one worth the test: a filter that fails open is worse
/// than a filter that does not exist, because the answer still looks scoped.
#[tokio::test]
async fn the_task_id_filter_narrows_and_fails_closed() {
    let fx = Fixture::new();
    let sha = fx.sha_of("stop re-emitting on every poll tick");
    fx.insert_task("cas-fixture-2", "the poll-tick task");
    fx.seed_anchor("cas-fixture-2", &sha);

    let scoped = fx
        .history(serde_json::json!({
            "action": "history",
            "task_id": "cas-fixture-2",
            "include_provenance": true,
        }))
        .await;
    assert_eq!(scoped["count"], 1, "{scoped}");
    assert_eq!(scoped["results"][0]["sha"], sha);

    let unknown = fx
        .history(serde_json::json!({
            "action": "history",
            "task_id": "cas-does-not-exist",
        }))
        .await;
    assert_eq!(
        unknown["count"], 0,
        "an unresolvable task filter must match nothing, never widen to all commits"
    );
}

/// The session filter resolves through `events.worker_git_commit`, which is the
/// only edge that carries a session id at all.
#[tokio::test]
async fn the_session_id_filter_resolves_through_the_worker_event_edge() {
    let fx = Fixture::new();
    let sha = fx.sha_of("widen the transcript pane");
    fx.seed_worker_event(&sha, 8, "session-abc");

    let scoped = fx
        .history(serde_json::json!({
            "action": "history",
            "session_id": "session-abc",
        }))
        .await;
    assert_eq!(scoped["count"], 1, "{scoped}");
    assert_eq!(scoped["results"][0]["sha"], sha);

    let unknown = fx
        .history(serde_json::json!({
            "action": "history",
            "session_id": "session-nobody",
        }))
        .await;
    assert_eq!(unknown["count"], 0);
}

/// AC3 through the production path: the indexer repairs `commit_links` for a
/// commit the PostToolUse hook never saw, and the repaired spine then shows up
/// as a provenance edge on the query surface.
///
/// This is the end-to-end shape of spec §5.3 — the empty spine filling itself
/// from the index rather than from a harness-specific hook.
#[tokio::test]
async fn the_indexer_repairs_the_commit_links_spine_and_the_surface_sees_it() {
    let fx = Fixture::new();
    let sha = fx.sha_of("widen the transcript pane");
    fx.seed_worker_event(&sha, 8, "session-repair");

    let before: i64 = fx
        .db()
        .query_row("SELECT COUNT(*) FROM commit_links", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, 0, "the spine starts empty, as it is on the live DB");

    let outcome = cas::history::provenance::repair_commit_links(&fx.cas_root, &fx.repo, 100)
        .expect("repair pass");
    assert_eq!(outcome.examined, 3, "{outcome:?}");
    assert_eq!(outcome.written, 1, "{outcome:?}");
    assert_eq!(
        outcome.no_session_edge, 2,
        "the two commits with no event must be counted, not silently skipped"
    );

    let (session, method): (String, Option<String>) = fx
        .db()
        .query_row(
            "SELECT session_id, link_method FROM commit_links WHERE commit_hash = ?1",
            rusqlite::params![sha],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(session, "session-repair");
    assert_eq!(
        method.as_deref(),
        Some("indexer_worker_git_commit"),
        "a reconstructed link must be stamped as reconstructed (spec §5.3)"
    );

    // And the repaired row is now visible to the query surface, at MEDIUM —
    // it lives in the same table as an observation but does not claim to be one.
    let response = fx
        .history(serde_json::json!({
            "action": "history",
            "path": "src/ui/pane.rs",
            "include_provenance": true,
        }))
        .await;
    let edges = response["results"][0]["provenance"].as_array().unwrap();
    let spine = edges
        .iter()
        .find(|e| e["link_method"] == "indexer_worker_git_commit")
        .unwrap_or_else(|| panic!("the repaired spine row is not surfaced: {edges:?}"));
    assert_eq!(spine["confidence"], "medium");
    assert_eq!(spine["session_id"], "session-repair");
}

/// The measured trap this assertion exists for: on the live corpus a session's
/// only linked commit is frequently a **merge**, and merges are excluded by
/// default (§7.1). "no commits matched" would then report that the session
/// shipped nothing — a different and wrong claim from "the filter resolved, and
/// another filter emptied the answer".
#[tokio::test]
async fn a_filter_that_resolved_but_returned_nothing_says_which_it_was() {
    let fx = Fixture::new();
    // A merge commit, linked to a session, on top of the fixture history.
    git(&fx.repo, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    commit(&fx.repo, "src/side.rs", "fn side() {}\n", "side work");
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(&fx.repo, &["merge", "--no-ff", "-q", "side", "-m", "merge side"]);
    cas::history::run_index_pass(&fx.cas_root, &fx.repo).expect("delta pass");

    let merge_sha = fx.sha_of("merge side");
    fx.seed_worker_event(&merge_sha, 8, "session-merge-only");

    let default = fx
        .history(serde_json::json!({
            "action": "history",
            "session_id": "session-merge-only",
        }))
        .await;
    assert_eq!(default["count"], 0, "merges are excluded by default");
    assert_eq!(
        default["filters"]["identity_filter_matched"], 1,
        "the response must show the filter RESOLVED, so an empty answer is not \
         mistaken for 'this session produced nothing': {default}"
    );

    // And an identity filter that genuinely resolves to nothing reports zero,
    // which is the honest distinction the field exists to make.
    let unknown = fx
        .history(serde_json::json!({
            "action": "history",
            "session_id": "session-nobody",
        }))
        .await;
    assert_eq!(unknown["filters"]["identity_filter_matched"], 0);

    // With merges included, the same filter answers.
    let with_merges = fx
        .history(serde_json::json!({
            "action": "history",
            "session_id": "session-merge-only",
            "include_merges": true,
        }))
        .await;
    assert_eq!(with_merges["count"], 1, "{with_merges}");
    assert_eq!(with_merges["results"][0]["sha"], merge_sha);
}
