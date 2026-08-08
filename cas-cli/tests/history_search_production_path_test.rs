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

        let core = CasCore::with_daemon(cas_root, None, None);
        core.set_agent_id_for_testing("history-search-test".to_string());
        Self {
            _temp: temp,
            service: CasService::new(core, None),
        }
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
        // Commits are not dropped for lacking provenance (spec §6.4 Q3).
        assert!(hit["provenance"].is_null());
        assert!(
            hit["provenance_reason"].as_str().unwrap().contains("M5"),
            "a null provenance with no reason is indistinguishable from a bug"
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
        status["provenance_supported"], false,
        "M4 must never advertise provenance as supported"
    );
    assert!(status["provenance_note"].as_str().unwrap().contains("M5"));

    let declared = response["unsupported"].as_array().unwrap();
    assert!(
        declared
            .iter()
            .any(|u| u["feature"] == "include_provenance" && u["lands_in"] == "M5"),
        "asking for provenance must be answered with a declaration, not silence"
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
