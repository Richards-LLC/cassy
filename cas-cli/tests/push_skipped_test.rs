//! Integration test for the cas-f645 client-side defense.
//!
//! When the cloud server's push route silently skips rows due to a
//! cross-project `project_canonical_id` conflict (via Postgres
//! `ON CONFLICT DO UPDATE ... WHERE false ... RETURNING`), the response
//! carries a per-entity-type `skipped` count. The client must inspect that
//! count and leave the corresponding local queue items un-marked-synced so
//! they remain retryable instead of being silently dropped.
//!
//! This test exercises the full `CloudSyncer::push` path through a
//! wiremock-backed `/api/sync/push` endpoint to lock that behavior in.
//! Companion server-side change is tracked under cas-d656 / cas-0bdc.

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

/// When the server reports `skipped > 0` for an entity type, the affected
/// queue items must remain in the local sync queue (retryable) instead of
/// being deleted by `mark_synced`. This is the cas-f645 contract.
#[tokio::test]
async fn skipped_response_leaves_queue_items_pending() {
    let server = MockServer::start().await;

    // Live server shape: counts are nested under the entity key rather than
    // in the older proposed top-level `skipped` map.
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": { "inserted": 0, "updated": 0, "skipped": 1 }
        })))
        .expect(1..)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
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
    let (push_result, syncer) = tokio::task::spawn_blocking(move || (syncer.push(), syncer))
        .await
        .expect("spawn_blocking join");
    let push_result = push_result.expect("push() returned Err");

    // Server reported 1 skipped → the client must NOT count this as
    // pushed. The legacy "trust the 200" path would have set pushed_entries
    // to 1; this assertion locks in the new behavior.
    assert_eq!(
        push_result.pushed_entries, 0,
        "client must not count server-skipped rows as pushed",
    );

    // Critical AC: the queue item is still present (retryable). If
    // `mark_synced` had been called, `pending_count` would be 0 and the
    // row would have been deleted from the sync_queue table.
    let queue_after = syncer.queue();
    assert_eq!(
        queue_after.pending_count(5).unwrap(),
        1,
        "queue items the server skipped must remain pending — cas-f645 contract",
    );
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
