//! GH #701 — measure the pull-side origin filter against real cloud payloads.
//!
//! `tests/fixtures/gh701_pull_origin_histogram.json` is a capture of three live
//! `GET /api/sync/pull` responses, reduced to the only two fields attribution
//! depends on: how many rows carried each `origin_project`, and what
//! `project_id` the server stamped them with. Bodies are deliberately not
//! captured — this is an identity measurement, not a content snapshot.
//!
//! The capture is the evidence for the issue's central claim: the server stamps
//! every returned row with the scope you asked for, so `project_id` cannot
//! distinguish a native row from a replicated one, and a client that reads it
//! first ingests the lot.
//!
//! This test reconstructs rows from that histogram, runs them through the
//! shipped ingest predicate, and pins the resulting refusal counts. If someone
//! reverts attribution to scope-first, the numbers move and this fails with the
//! real before/after in the message.

use std::collections::BTreeMap;

use cas::cloud::syncer_testing::{accepts_entity, accepts_task_dependency};

fn fixture() -> serde_json::Value {
    // Embedded at compile time: CI suite shards run on a different runner than
    // the build, so a CARGO_MANIFEST_DIR path does not exist there.
    serde_json::from_str(include_str!("fixtures/gh701_pull_origin_histogram.json"))
        .expect("fixture is valid JSON")
}

/// Rebuild one representative row per (entity kind, origin) bucket.
fn row(kind: &str, origin: &str, scope: &str) -> serde_json::Value {
    let mut row = serde_json::json!({
        "id": format!("{kind}-sample"),
        "project_id": scope,
    });
    if origin != "null" {
        row["origin_project"] = serde_json::Value::String(origin.to_string());
    }
    row
}

fn accepted(kind: &str, origin: &str, scope: &str) -> bool {
    let raw = row(kind, origin, scope);
    if kind == "task_dependencies" {
        accepts_task_dependency(&raw, scope)
    } else {
        accepts_entity(&raw, scope, kind)
    }
}

#[test]
fn the_origin_filter_refuses_exactly_the_replicated_rows_in_the_live_capture() {
    let doc = fixture();
    let mut summary: BTreeMap<String, (u64, u64)> = BTreeMap::new();

    for scope_doc in doc["scopes"].as_array().expect("scopes") {
        let scope = scope_doc["project_id"].as_str().expect("project_id");
        let mut refused = 0u64;
        let mut ingested = 0u64;

        for (kind, entity) in scope_doc["entities"].as_object().expect("entities") {
            // Every row in the capture was stamped with the requested scope —
            // the fact that makes `project_id` useless for attribution.
            let stamps = entity["project_id_stamps"].as_array().expect("stamps");
            assert_eq!(
                stamps.len(),
                1,
                "{scope}/{kind}: the server stamped more than one project_id"
            );
            assert_eq!(
                stamps[0].as_str(),
                Some(scope),
                "{scope}/{kind}: the stamp is not an echo of the requested scope"
            );

            for (origin, count) in entity["origin_project"].as_object().expect("origins") {
                let count = count.as_u64().expect("count");
                if accepted(kind, origin, scope) {
                    ingested += count;
                } else {
                    refused += count;
                }
            }
        }
        summary.insert(scope.to_string(), (ingested, refused));
    }

    // Measured 2026-09-03 against the live account. Before this change every
    // one of the refused rows was ingested as native, because the scope stamp
    // was read first.
    let expected: BTreeMap<String, (u64, u64)> = [
        ("cas-src".to_string(), (4665, 974)),
        ("gabber-studio".to_string(), (4221, 2493)),
        ("richards-llc-accounting".to_string(), (4, 3002)),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        summary, expected,
        "ingested/refused row counts moved against the live GH #701 capture"
    );
}

/// The single most legible number in the issue: pulling the accounting project
/// returns one real task and three thousand of another project's dependency
/// edges, every one stamped as the accounting project's own.
#[test]
fn the_accounting_scope_is_almost_entirely_another_projects_dependency_graph() {
    let doc = fixture();
    let scope = doc["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|s| s["project_id"] == "richards-llc-accounting")
        .expect("the accounting capture");

    let deps = &scope["entities"]["task_dependencies"];
    assert_eq!(deps["total"].as_u64(), Some(3002));
    assert_eq!(deps["origin_project"]["cas-src"].as_u64(), Some(3002));
    assert_eq!(scope["entities"]["tasks"]["total"].as_u64(), Some(1));

    assert!(
        !accepts_task_dependency(
            &row("task_dependencies", "cas-src", "richards-llc-accounting"),
            "richards-llc-accounting"
        ),
        "all 3,002 must now be refused"
    );
}

/// Rows predating `origin_project` must keep flowing, or the fix trades a
/// contamination bug for silent data loss. In this capture that is every entry,
/// every rule, and 897 of the 1,203 task rows.
#[test]
fn rows_without_an_origin_are_still_ingested_via_the_scope_stamp() {
    let doc = fixture();
    let mut legacy = 0u64;
    for scope_doc in doc["scopes"].as_array().expect("scopes") {
        let scope = scope_doc["project_id"].as_str().expect("project_id");
        for (kind, entity) in scope_doc["entities"].as_object().expect("entities") {
            if let Some(count) = entity["origin_project"]["null"].as_u64() {
                legacy += count;
                assert!(
                    accepted(kind, "null", scope),
                    "{scope}/{kind}: legacy rows without an origin must still be ingested"
                );
            }
        }
    }
    assert_eq!(legacy, 3761, "legacy row count moved against the capture");
}
