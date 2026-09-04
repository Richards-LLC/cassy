//! Integration tests for `cas cloud sync`'s team-queue drain path.
//!
//! Exercises `cas::cli::cloud::execute_team_push` — the helper wired into
//! `execute_sync` by cas-1f44 (T4) that drains the team sync queue into
//! `POST /api/teams/{uuid}/sync/push` when a team is configured.
//!
//! Coverage:
//! - Happy path: team configured + queued items → POST fires, queue drained.
//! - No team configured → early return, zero HTTP requests.
//! - `team_auto_promote=Some(false)` kill-switch → suppresses push even
//!   when team_id is set.
//! - Empty queue with team configured → silent early return.
//! - HTTP 500 failure → `push_team` leaves items pending by marking the
//!   attempted rows failed, while the helper still returns `Ok(())`
//!   (isolation contract — personal push and pull must not be blocked by
//!   team push errors).
//! - Large team upserts → `push_team` sends bounded gzip-compressed
//!   per-entity requests instead of one unbounded multi-entity payload.
//!
//! Lives in `cas-cli/tests/` (integration-test tree) rather than
//! co-located with the impl because the verifier flagged the inline
//! `#[cfg(test)] mod` as a test-first posture concern — tests are easier
//! to find in the integration tree than buried in a 2400-line impl file.

mod common;
use common::{TEST_TEAM, make_cli_json, make_cloud_config};

use cas::cli::cloud::execute_team_push;
use cas::cloud::{
    CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType, SyncOperation, SyncQueue,
};
use cas::store::open_task_store_local;
use cas::types::Task;
use flate2::read::GzDecoder;
use std::io::Read;
use std::sync::Arc;
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Every temporary queue root in this binary is pinned to this identity. Keep
// wire expectations tied to that fixture root rather than the test process's
// checkout identity.
const FIXTURE_PROJECT_ID: &str = "p";

/// Create a `.cas`-style directory and seed the sync queue with one
/// team-tagged entry upsert, returning the TempDir owning the files.
fn make_cas_root_with_team_item() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "p-test-001",
            SyncOperation::Upsert,
            Some(r#"{"id":"p-test-001","scope":"project","content":"hi"}"#),
            TEST_TEAM,
        )
        .unwrap();
    tmp
}

fn decode_gzip_json(body: &[u8]) -> serde_json::Value {
    let mut decoder = GzDecoder::new(body);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .expect("request body should be valid gzip");
    serde_json::from_slice(&decoded).expect("request body should decode to JSON")
}

fn seed_team_task_move(queue: &SyncQueue, task_id: &str) {
    queue
        .enqueue_team_move(
            EntityType::Task,
            task_id,
            "project-a",
            "project-b",
            &format!(
                r#"{{"id":"{task_id}","title":"moved","scope":"project","origin_project":"project-b"}}"#
            ),
            TEST_TEAM,
        )
        .unwrap();
}

#[tokio::test]
async fn team_task_move_deletes_old_project_before_upserting_new_owner() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/teams/{TEST_TEAM}/sync/task/move-order-task"
        )))
        .and(query_param("project_id", "project-a"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": { "tasks": { "inserted": 1, "updated": 0, "skipped": 0 } }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    seed_team_task_move(&queue, "move-order-task");
    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );

    tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .expect("move push should succeed");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method.as_str(), "DELETE");
    assert_eq!(requests[1].method.as_str(), "POST");
    let payload = decode_gzip_json(&requests[1].body);
    assert_eq!(payload["project_canonical_id"], "project-b");
    assert_eq!(payload["tasks"][0]["origin_project"], "project-b");
    assert!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().is_empty());
}

#[tokio::test]
async fn team_task_move_delete_failure_blocks_upsert_and_retains_both_rows() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/teams/{TEST_TEAM}/sync/task/move-delete-failure"
        )))
        .and(query_param("project_id", "project-a"))
        .respond_with(ResponseTemplate::new(500).set_body_string("delete failed"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    seed_team_task_move(&queue, "move-delete-failure");
    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );

    let result = tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .expect("push returns a result for an HTTP delete failure");

    assert!(!result.errors.is_empty());
    let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
    assert_eq!(pending.len(), 2, "both move rows must remain retryable");
    assert!(pending.iter().all(|item| item.retry_count > 0));
    assert!(pending.iter().all(|item| item.last_error.is_some()));
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| { request.method.as_str() == "DELETE" })
    );
}

#[tokio::test]
async fn team_task_move_upsert_failure_retains_only_upsert_for_retry() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/teams/{TEST_TEAM}/sync/task/move-upsert-failure"
        )))
        .and(query_param("project_id", "project-a"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(500).set_body_string("upsert failed"))
        .expect(3)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    seed_team_task_move(&queue, "move-upsert-failure");
    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );

    let result = tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .expect("push returns a result for an HTTP upsert failure");

    assert!(!result.errors.is_empty());
    let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
    assert_eq!(
        pending.len(),
        1,
        "successful old-key delete must be settled"
    );
    assert_eq!(pending[0].operation, SyncOperation::Upsert);
    assert!(pending[0].retry_count > 0);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].method.as_str(), "DELETE");
    assert!(
        requests[1..]
            .iter()
            .all(|request| request.method.as_str() == "POST")
    );
}

/// Happy path: team configured + queued items → POST fires against
/// `/api/teams/{uuid}/sync/push`, queue is drained.
///
/// NOTE: server contract includes `project_canonical_id` in the payload
/// so the server can auto-register the project. The payload is
/// gzip-compressed before send so wiremock body matchers can't cheaply
/// verify the field; that contract is covered by
/// `team_push_chunks_upserts_by_payload_budget` below.
#[tokio::test]
async fn team_push_drains_queue_when_team_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "entries": 1,
                "tasks": 0, "rules": 0, "skills": 0,
                "sessions": 0, "verifications": 0, "events": 0,
                "prompts": 0, "file_changes": 0, "commit_links": 0,
                "agents": 0, "worktrees": 0,
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = make_cloud_config(server.uri());
    let tmp = make_cas_root_with_team_item();
    let cas_root = tmp.path().to_path_buf();
    let cli = make_cli_json();

    // `execute_team_push` is sync and uses `ureq`; run on the blocking
    // pool so the wiremock tokio runtime can serve the request.
    let result = tokio::task::spawn_blocking(move || execute_team_push(&cfg, &cas_root, &cli))
        .await
        .unwrap();

    assert!(result.is_ok(), "execute_team_push returned Err: {result:?}");

    let queue = SyncQueue::open(tmp.path()).unwrap();
    let remaining = queue.pending_for_team(TEST_TEAM, 100, 5).unwrap();
    assert_eq!(remaining.len(), 0, "team queue should be drained");
}

#[tokio::test]
async fn team_push_nested_skip_response_acknowledges_lww_row() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "entries": { "inserted": 0, "updated": 0, "skipped": 1 }
            },
            "canonical_id": "cas-src",
            "git_remote": "github.com/pippenz/cas"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = make_cloud_config(server.uri());
    let tmp = make_cas_root_with_team_item();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    let syncer = CloudSyncer::new(queue.clone(), cfg, CloudSyncerConfig::default());

    let (results, queue) = tokio::task::spawn_blocking(move || {
        let results = (0..5)
            .map(|_| {
                syncer
                    .push_team(TEST_TEAM)
                    .expect("team push should return a result")
            })
            .collect::<Vec<_>>();
        (results, queue)
    })
    .await
    .unwrap();
    let result = &results[0];

    assert_eq!(result.pushed_entries, 1);
    assert!(
        result.errors.is_empty(),
        "aggregate LWW skip is an acknowledgement: {:?}",
        result.errors
    );
    assert_eq!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().len(), 0);
    assert_eq!(queue.stats(5).unwrap().failed, 0);
    assert!(queue.list_all(10).unwrap().is_empty());
}

#[tokio::test]
async fn team_itemized_rejection_syncs_owned_row_and_names_scope_mismatch() {
    let server = MockServer::start().await;
    let body: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/cloud_push/team-itemized-scope-mismatch.json"
    ))
    .expect("team itemized rejection fixture must be valid JSON");
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = make_cas_root_with_team_item();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "rejected-team-entry-002",
            SyncOperation::Upsert,
            Some(r#"{"id":"rejected-team-entry-002","scope":"project","content":"no"}"#),
            TEST_TEAM,
        )
        .unwrap();
    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );
    let (first, second, queue) = tokio::task::spawn_blocking(move || {
        let first = syncer.push_team(TEST_TEAM);
        let second = syncer.push_team(TEST_TEAM);
        (first, second, queue)
    })
    .await
    .expect("spawn_blocking join");

    assert!(first.as_ref().is_ok_and(|result| !result.errors.is_empty()));
    assert!(
        second.as_ref().is_ok_and(|result| result.errors.is_empty()),
        "a parked permanent rejection must not recur on the next sync"
    );
    assert_eq!(queue.stats(5).unwrap().failed, 1);
    assert_eq!(queue.list_all(10).unwrap().len(), 1);
    let failed = &queue.list_all(10).unwrap()[0];
    assert_eq!(failed.entity_id, "rejected-team-entry-002");
    assert!(failed.last_error.as_deref().is_some_and(|error| {
        error.contains("scope_mismatch")
            && error.contains("existing_project=cas-src")
            && !error.contains("server response")
    }));
}

/// Regression for the production 14/17 team response. The server's aggregate
/// skip count includes stale/no-op writes that intentionally have no rejection
/// detail; only the 14 named scope collisions should remain failed.
#[tokio::test]
async fn team_itemized_rejection_subset_settles_unrejected_rows_for_fourteen_of_seventeen() {
    let server = MockServer::start().await;
    let rejected_ids = [
        "cas-bcf5", "cas-f21c", "cas-ff65", "cas-2327", "cas-058e", "cas-b4bb", "cas-607a",
        "cas-1a4e", "cas-5793", "cas-545e", "cas-0f08", "cas-9f43", "cas-fa64", "cas-4fa4",
    ];
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "tasks": {
                    "inserted": 0,
                    "updated": 0,
                    "skipped": 17,
                    "rejected": rejected_ids.map(|id| serde_json::json!({
                        "id": id,
                        "reason": "scope_mismatch",
                        "existing_canonical_id": "cas-src",
                    })),
                }
            },
            "canonical_id": "cas-src",
            "git_remote": "github.com/pippenz/cas"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    let all_ids = rejected_ids
        .into_iter()
        .map(str::to_string)
        .chain((0..3).map(|index| format!("cas-team-owned-{index:02}")))
        .collect::<Vec<_>>();
    for id in &all_ids {
        queue
            .enqueue_for_team(
                EntityType::Task,
                id,
                SyncOperation::Upsert,
                Some(&serde_json::json!({"id": id, "title": id}).to_string()),
                TEST_TEAM,
            )
            .unwrap();
    }

    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig {
            max_retries: 1,
            ..Default::default()
        },
    );
    let result = tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .expect("team push should return an itemized result");

    assert_eq!(result.pushed_tasks, 3);
    assert!(
        result
            .errors
            .iter()
            .all(|error| !error.contains("invalid itemized rejections")),
        "valid subset itemization must not fail the whole team batch: {result:?}"
    );
    let remaining = queue.list_all(50).unwrap();
    assert_eq!(remaining.len(), 14, "the three benign skips must settle");
    assert_eq!(queue.stats(1).unwrap().failed, 14);
    for item in remaining {
        assert!(rejected_ids.contains(&item.entity_id.as_str()));
        assert!(item.last_error.as_deref().is_some_and(|error| {
            error.contains("scope_mismatch")
                && error.contains("existing_project=cas-src")
                && !error.contains("server response")
        }));
    }
}

/// No team configured → early return, zero HTTP traffic, `Ok(())`.
#[tokio::test]
async fn team_push_no_op_when_no_team_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let mut cfg = CloudConfig::default();
    cfg.endpoint = server.uri();
    cfg.token = Some("test-token".to_string());
    // Deliberately no set_team — active_team_id() returns None.

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path().to_path_buf();
    let cli = make_cli_json();

    let result = tokio::task::spawn_blocking(move || execute_team_push(&cfg, &cas_root, &cli))
        .await
        .unwrap();
    assert!(result.is_ok());
}

/// Kill-switch: `team_auto_promote=Some(false)` must suppress the push
/// exactly like no-team-configured does — even when team_id is set.
#[tokio::test]
async fn team_push_suppressed_by_auto_promote_kill_switch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let mut cfg = make_cloud_config(server.uri());
    cfg.team_auto_promote = Some(false);

    let tmp = make_cas_root_with_team_item();
    let cas_root = tmp.path().to_path_buf();
    let cli = make_cli_json();

    let result = tokio::task::spawn_blocking(move || execute_team_push(&cfg, &cas_root, &cli))
        .await
        .unwrap();
    assert!(result.is_ok());
}

/// Empty team queue + team configured (steady state after a full sync)
/// → no output noise, no error, zero HTTP.
#[tokio::test]
async fn team_push_silent_when_queue_empty() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let cfg = make_cloud_config(server.uri());
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    // Deliberately no enqueue_for_team — queue is empty.
    let cas_root = tmp.path().to_path_buf();
    let cli = make_cli_json();

    let result = tokio::task::spawn_blocking(move || execute_team_push(&cfg, &cas_root, &cli))
        .await
        .unwrap();
    assert!(result.is_ok());
}

#[tokio::test]
async fn team_delete_for_live_task_is_neutralized_without_http() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    open_task_store_local(tmp.path())
        .unwrap()
        .add(&Task::new(
            "live-team-task".to_string(),
            "still here".to_string(),
        ))
        .unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "live-team-task",
            SyncOperation::Delete,
            None,
            TEST_TEAM,
        )
        .unwrap();

    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );
    let result = tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .unwrap();

    assert!(result.errors.is_empty());
    assert!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn team_delete_uses_singular_entity_path() {
    let server = MockServer::start().await;
    let expected_project_id = FIXTURE_PROJECT_ID;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/teams/{TEST_TEAM}/sync/task/absent-team-task"
        )))
        .and(query_param("project_id", expected_project_id))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    open_task_store_local(tmp.path()).unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "absent-team-task",
            SyncOperation::Delete,
            None,
            TEST_TEAM,
        )
        .unwrap();

    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );
    tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .unwrap();

    assert!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().is_empty());
}

/// Legacy queued tasks may predate the stored `scope` field. Team pushes must
/// repair that wire identity before sending: the cloud accepts the batch with
/// HTTP 200 but skips task rows whose explicit scope is absent.
#[tokio::test]
async fn team_task_upsert_includes_explicit_project_scope() {
    let server = MockServer::start().await;
    let expected_project_id = FIXTURE_PROJECT_ID;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "tasks": { "inserted": 1, "updated": 0, "skipped": 0 }
            },
            "canonical_id": expected_project_id,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "legacy-team-task",
            SyncOperation::Upsert,
            Some(r#"{"id":"legacy-team-task","title":"legacy payload"}"#),
            TEST_TEAM,
        )
        .unwrap();

    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );
    tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .expect("team task push should succeed");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let payload = decode_gzip_json(&requests[0].body);
    assert_eq!(payload["project_canonical_id"], expected_project_id);
    assert_eq!(payload["tasks"][0]["scope"], "project");
    assert_eq!(payload["tasks"][0]["origin_project"], expected_project_id);
}

/// Terminally parked rows are intentionally excluded from normal syncs. The
/// existing `cas cloud queue --retry` recovery path requeues them so a fixed
/// team-delete endpoint can flush the retained tombstone.
#[tokio::test]
async fn parked_team_delete_can_be_requeued_and_flushed_after_scope_fix() {
    let server = MockServer::start().await;
    let expected_project_id = FIXTURE_PROJECT_ID;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/teams/{TEST_TEAM}/sync/task/parked-team-task"
        )))
        .and(query_param("project_id", expected_project_id))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    open_task_store_local(tmp.path()).unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "parked-team-task",
            SyncOperation::Delete,
            None,
            TEST_TEAM,
        )
        .unwrap();
    let parked_id = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap()[0].id;
    queue
        .park_failed(
            parked_id,
            "permanent cloud rejection: reason=project_mismatch; entity=tasks; id=parked-team-task; existing_project=other",
            5,
        )
        .unwrap();

    assert!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().is_empty());
    assert_eq!(queue.retry_failed(5).unwrap(), 1);

    let syncer = CloudSyncer::new(
        queue.clone(),
        make_cloud_config(server.uri()),
        CloudSyncerConfig::default(),
    );
    let result = tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .unwrap();

    assert!(
        result.errors.is_empty(),
        "requeued delete should flush: {result:?}"
    );
    assert!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().is_empty());
}

/// Isolation contract: team push HTTP failure must not block the caller's
/// pull step. `execute_team_push` returns `Ok(())` even on 5xx; push_team
/// marks the attempted items failed so they survive the failed attempt for
/// the next sync cycle until retry limits are exhausted.
#[tokio::test]
async fn team_push_http_failure_is_isolated() {
    let server = MockServer::start().await;
    // push_team retries 3 times internally on 5xx.
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let cfg = make_cloud_config(server.uri());
    let tmp = make_cas_root_with_team_item();
    let cas_root = tmp.path().to_path_buf();
    let cli = make_cli_json();

    let result = tokio::task::spawn_blocking(move || execute_team_push(&cfg, &cas_root, &cli))
        .await
        .unwrap();
    assert!(
        result.is_ok(),
        "helper must return Ok even when team push fails (partial-failure isolation): {result:?}"
    );

    // Item remains pending after push_team records the failed attempt.
    let queue = SyncQueue::open(tmp.path()).unwrap();
    let remaining = queue.pending_for_team(TEST_TEAM, 100, 5).unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "team items remain pending on http failure (preserves data for next sync)"
    );
    assert!(
        remaining[0].retry_count > 0,
        "an attempted team push must increment retry_count"
    );
    assert!(
        remaining[0]
            .last_error
            .as_deref()
            .is_some_and(|error| !error.is_empty()),
        "an attempted team push must persist a non-empty last_error"
    );
}

#[tokio::test]
async fn team_push_chunks_upserts_by_payload_budget() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "entries": 1,
                "tasks": 0, "rules": 0, "skills": 0,
                "sessions": 0, "verifications": 0, "events": 0,
                "prompts": 0, "file_changes": 0, "commit_links": 0,
                "agents": 0, "worktrees": 0,
            }
        })))
        .expect(3)
        .mount(&server)
        .await;

    let cfg = make_cloud_config(server.uri());
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();

    for i in 0..3 {
        let id = format!("p-large-{i}");
        let payload = serde_json::json!({
            "id": id,
            "scope": "project",
            "content": "x".repeat(900),
        })
        .to_string();
        queue
            .enqueue_for_team(
                EntityType::Entry,
                &id,
                SyncOperation::Upsert,
                Some(&payload),
                TEST_TEAM,
            )
            .unwrap();
    }

    let mut sync_config = CloudSyncerConfig::default();
    sync_config.max_payload_bytes = 1_250;
    sync_config.backoff_base_ms = 1;

    let syncer = CloudSyncer::new(queue.clone(), cfg, sync_config);
    let result = tokio::task::spawn_blocking(move || syncer.push_team(TEST_TEAM))
        .await
        .unwrap()
        .expect("team push should succeed");

    assert_eq!(result.pushed_entries, 3);
    assert!(result.errors.is_empty());
    assert_eq!(
        queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().len(),
        0,
        "all chunks should be marked synced"
    );

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3, "team push should split into 3 requests");

    for request in requests {
        let payload = decode_gzip_json(&request.body);
        let encoded_len = serde_json::to_vec(&payload).unwrap().len();
        assert!(
            encoded_len <= 1_250,
            "team push request should stay under max_payload_bytes; got {encoded_len}"
        );
        assert_eq!(
            payload["entries"].as_array().unwrap().len(),
            1,
            "each request should contain only one large entry"
        );
        assert!(
            payload.get("tasks").is_none(),
            "chunked team push should send one entity type per request"
        );
        assert!(
            payload["project_canonical_id"].as_str().is_some(),
            "team push chunks should include project_canonical_id"
        );
    }
}
