//! Integration tests for the cas-2cce push response contract.
//!
//! Aggregate-only `skipped` counts acknowledge local rows under the server's
//! last-write-wins semantics. A response that identifies a row-level
//! `rejected` outcome still parks that row with its actionable reason.

use std::sync::Arc;
use std::time::Duration;

mod common;
use common::make_cloud_config;

use cas::cloud::{CloudSyncer, CloudSyncerConfig, EntityType, SyncOperation, SyncQueue};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Minimal JSON payload for a queued entry. The push path parses the
/// payload via `serde_json::from_str` and only forwards the resulting
/// `Value` to the server, so any well-formed JSON object works for the
/// queue-behavior assertion this test makes — no need to track Entry
/// schema drift here.
fn entry_payload(id: &str) -> String {
    serde_json::json!({
        "id": id,
        "content": "x",
        "type": "learning",
        "scope": "project",
    })
    .to_string()
}

/// Aggregate-only skips are last-write-wins acknowledgements: the row is
/// removed from the local queue, counted as pushed, and does not become an
/// error or visible failed item.
#[tokio::test]
async fn skipped_response_is_acknowledged_as_lww() {
    let server = MockServer::start().await;

    // Live server shape: counts are nested under the entity key rather than
    // in the older proposed top-level `skipped` map.
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": { "inserted": 0, "updated": 0, "skipped": 1 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_dir = tmp.path();

    // Seed: one queued upsert for an entry. Use a fresh cas.db in TempDir
    // so this test can't collide with the worker's real sync queue.
    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "skipped-test-entry-001",
            SyncOperation::Upsert,
            Some(&entry_payload("skipped-test-entry-001")),
        )
        .unwrap();
    assert_eq!(
        queue.pending_count(5).unwrap(),
        1,
        "precondition: queue must contain the seeded entry",
    );

    // Build a CloudSyncer pointed at wiremock. `make_cloud_config` sets a
    // team_id, but the personal push path is what we want to exercise —
    // we'll use `push_with_sessions(&[])` which routes to `/api/sync/push`
    // regardless of the team field.
    let mut cfg = make_cloud_config(server.uri());
    // Clear team_id so `push` hits the personal `/api/sync/push` endpoint
    // (push_with_sessions checks team_id only when including it in the
    // request body — the matcher is path-only, so this just keeps the
    // routing intent explicit).
    cfg.team_id = None;
    let syncer_config = CloudSyncerConfig {
        timeout: Duration::from_secs(5),
        max_retries: 5,
        ..Default::default()
    };
    let syncer = CloudSyncer::new(Arc::new(queue), cfg, syncer_config);

    // `push` is sync + blocking ureq; the wiremock runtime needs us off
    // the executor thread to serve the POST.
    let (push_result, syncer) =
        tokio::task::spawn_blocking(move || (syncer.push().expect("push() returned Err"), syncer))
            .await
            .expect("spawn_blocking join");

    assert_eq!(
        push_result.pushed_entries, 1,
        "aggregate skips must count as acknowledged pushes",
    );
    assert!(
        push_result.errors.is_empty(),
        "aggregate skips are acknowledged without an error: {push_result:?}"
    );
    let queue_after = syncer.queue();
    assert_eq!(
        queue_after.pending_count(5).unwrap(),
        0,
        "acknowledged aggregate skips must be removed from the pending queue",
    );
    assert_eq!(queue_after.stats(5).unwrap().failed, 0);
    assert!(queue_after.list_all(10).unwrap().is_empty());
}

/// A row-level rejection is different from an aggregate skip: it identifies
/// the exact queue item and must remain visible with its server-provided
/// reason for operator repair.
#[tokio::test]
async fn per_row_rejected_outcome_stays_visible_with_reason() {
    let server = MockServer::start().await;
    let entry_id = "rejected-outcome-entry-001";
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {
                "inserted": 0,
                "updated": 0,
                "skipped": 1,
                "rows": [{
                    "id": entry_id,
                    "outcome": "rejected",
                    "reason": "project_mismatch"
                }]
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            entry_id,
            SyncOperation::Upsert,
            Some(&entry_payload(entry_id)),
        )
        .unwrap();

    let mut cfg = make_cloud_config(server.uri());
    cfg.team_id = None;
    let syncer = CloudSyncer::new(
        Arc::new(queue),
        cfg,
        CloudSyncerConfig {
            timeout: Duration::from_secs(5),
            max_retries: 5,
            ..Default::default()
        },
    );

    let (push_result, syncer) =
        tokio::task::spawn_blocking(move || (syncer.push().expect("push() returned Err"), syncer))
            .await
            .expect("spawn_blocking join");

    assert_eq!(push_result.pushed_entries, 0);
    assert_eq!(push_result.errors.len(), 1);
    assert!(push_result.errors[0].contains("cloud rejected 1 of 1 entries"));
    let queue_after = syncer.queue();
    assert_eq!(queue_after.pending_count(5).unwrap(), 0);
    assert_eq!(queue_after.stats(5).unwrap().failed, 1);
    let items = queue_after.list_all(10).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].entity_id, entry_id);
    assert!(
        items[0]
            .last_error
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("reason=project_mismatch")),
        "queue output must expose the rejection reason: {items:?}"
    );
}

/// A current cloud response itemizes genuine identity collisions. The client
/// must dequeue the owned neighbor in the same batch and retain only the named
/// rejection with its actionable reason for `cas cloud queue --verbose`.
#[tokio::test]
async fn itemized_rejection_syncs_owned_row_and_names_project_mismatch() {
    let server = MockServer::start().await;
    let body: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/cloud_push/personal-itemized-project-mismatch.json"
    ))
    .expect("personal itemized rejection fixture must be valid JSON");
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    for id in ["owned-project-entry-001", "rejected-project-entry-002"] {
        queue
            .enqueue(
                EntityType::Entry,
                id,
                SyncOperation::Upsert,
                Some(&entry_payload(id)),
            )
            .unwrap();
    }

    let mut cfg = make_cloud_config(server.uri());
    cfg.team_id = None;
    let syncer = CloudSyncer::new(
        Arc::new(queue),
        cfg,
        CloudSyncerConfig {
            timeout: Duration::from_secs(5),
            max_retries: 5,
            ..Default::default()
        },
    );
    let (first, second, syncer) = tokio::task::spawn_blocking(move || {
        let first = syncer.push();
        let second = syncer.push();
        (first, second, syncer)
    })
    .await
    .expect("spawn_blocking join");

    assert!(first.as_ref().is_ok_and(|result| !result.errors.is_empty()));
    assert!(
        second.as_ref().is_ok_and(|result| result.errors.is_empty()),
        "a parked permanent rejection must not recur on the next sync"
    );
    assert_eq!(syncer.queue().stats(5).unwrap().failed, 1);
    assert_eq!(syncer.queue().list_all(10).unwrap().len(), 1);
    let failed = &syncer.queue().list_all(10).unwrap()[0];
    assert_eq!(failed.entity_id, "rejected-project-entry-002");
    assert!(failed.last_error.as_deref().is_some_and(|error| {
        error.contains("project_mismatch")
            && error.contains("foreign-project")
            && !error.contains("server response")
    }));
}

/// Regression for the production 6/20 response: `skipped` includes benign
/// stale/no-op rows while `rejected` only names genuine identity collisions.
#[tokio::test]
async fn itemized_rejection_subset_settles_unrejected_rows_for_six_of_twenty() {
    let server = MockServer::start().await;
    let rejected_ids = [
        "cas-8284", "cas-03eb", "cas-3a9b", "cas-0a35", "cas-e225", "cas-b131",
    ];
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tasks": {
                "inserted": 0,
                "updated": 0,
                "skipped": 20,
                "rejected": rejected_ids.map(|id| serde_json::json!({
                    "id": id,
                    "reason": "scope_mismatch",
                    "existing_canonical_id": "cas-src",
                })),
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    let all_ids = rejected_ids
        .into_iter()
        .map(str::to_string)
        .chain((0..14).map(|index| format!("cas-owned-{index:02}")))
        .collect::<Vec<_>>();
    for id in &all_ids {
        queue
            .enqueue(
                EntityType::Task,
                id,
                SyncOperation::Upsert,
                Some(&entry_payload(id)),
            )
            .unwrap();
    }

    let mut cfg = make_cloud_config(server.uri());
    cfg.team_id = None;
    let syncer = CloudSyncer::new(
        Arc::new(queue),
        cfg,
        CloudSyncerConfig {
            max_retries: 1,
            ..Default::default()
        },
    );
    let (result, syncer) = tokio::task::spawn_blocking(move || (syncer.push(), syncer))
        .await
        .unwrap();
    let result = result.expect("push should return an itemized result");

    assert!(
        result
            .errors
            .iter()
            .all(|error| !error.contains("invalid itemized rejections")),
        "valid subset itemization must not fail the whole batch: {result:?}"
    );
    let remaining = syncer.queue().list_all(50).unwrap();
    assert_eq!(remaining.len(), 6, "the 14 benign skips must settle");
    assert_eq!(syncer.queue().stats(1).unwrap().failed, 6);
    for item in remaining {
        assert!(rejected_ids.contains(&item.entity_id.as_str()));
        assert!(item.last_error.as_deref().is_some_and(|error| {
            error.contains("scope_mismatch")
                && error.contains("existing_project=cas-src")
                && !error.contains("server response")
        }));
    }
}

/// Backward-compatibility guard: an older cloud build that does not yet
/// emit `skipped` must still trigger the legacy mark-synced path. This
/// keeps existing happy-path pushes unchanged while the server-side
/// `skipped` field rolls out (per cas-d656).
#[tokio::test]
async fn legacy_response_without_skipped_field_marks_items_synced() {
    let server = MockServer::start().await;

    // Older cloud build: 200 with an empty JSON body — no `skipped` field.
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1..)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_dir = tmp.path();

    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "legacy-test-entry-001",
            SyncOperation::Upsert,
            Some(&entry_payload("legacy-test-entry-001")),
        )
        .unwrap();
    assert_eq!(queue.pending_count(5).unwrap(), 1);

    let mut cfg = make_cloud_config(server.uri());
    cfg.team_id = None;
    let syncer_config = CloudSyncerConfig {
        timeout: Duration::from_secs(5),
        max_retries: 5,
        ..Default::default()
    };
    let syncer = CloudSyncer::new(Arc::new(queue), cfg, syncer_config);

    let (push_result, syncer) = tokio::task::spawn_blocking(move || (syncer.push(), syncer))
        .await
        .unwrap();
    let push_result = push_result.expect("push() returned Err");

    assert_eq!(
        push_result.pushed_entries, 1,
        "legacy response (no `skipped` field) must follow the mark-synced path",
    );
    assert_eq!(
        syncer.queue().pending_count(5).unwrap(),
        0,
        "queue must be drained on legacy response — happy-path unchanged",
    );
}
