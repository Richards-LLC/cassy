//! Regression coverage for queue-driven, root-bound personal pushes (cas-cb6e).

mod common;

use std::io::Read;
use std::process::Command;
use std::sync::Arc;

use cas::cli::cloud::{CloudPushArgs, execute_push};
use cas::cloud::{
    CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType, PushScope, SyncOperation, SyncQueue,
};
use cas::store::{open_store_local, open_task_store_local};
use cas::types::{Entry, Task};
use flate2::read::GzDecoder;
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn seed_project(root: &TempDir, endpoint: &str, canonical_id: &str) {
    let mut config = CloudConfig::default();
    config.endpoint = endpoint.to_string();
    config.token = Some("test-token".to_string());
    config.save_to_cas_dir(root.path()).unwrap();
    std::fs::write(
        root.path().join("config.toml"),
        format!("[project]\ncanonical_id = \"{canonical_id}\"\n"),
    )
    .unwrap();
    SyncQueue::open(root.path()).unwrap().init().unwrap();
}

fn set_origin(root: &TempDir, remote: &str) {
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["remote", "add", "origin", remote])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
}

fn enqueue(queue: &SyncQueue, kind: EntityType, id: &str, body_bytes: usize) {
    let payload = serde_json::json!({
        "id": id,
        "content": "x".repeat(body_bytes),
        "scope": "project",
    })
    .to_string();
    queue
        .enqueue(kind, id, SyncOperation::Upsert, Some(&payload))
        .unwrap();
}

fn decode_gzip_json(body: &[u8]) -> serde_json::Value {
    let mut decoder = GzDecoder::new(body);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    serde_json::from_slice(&decoded).unwrap()
}

fn init_local_entity_tables(root: &TempDir) {
    open_store_local(root.path()).unwrap();
    open_task_store_local(root.path()).unwrap();
}

fn delete_syncer(root: &TempDir, endpoint: String) -> CloudSyncer {
    let mut config = CloudConfig::default();
    config.endpoint = endpoint;
    config.token = Some("test-token".to_string());
    CloudSyncer::new_for_project(
        Arc::new(SyncQueue::open(root.path()).unwrap()),
        config,
        CloudSyncerConfig::default(),
        "delete-project".to_string(),
        root.path(),
    )
}

#[tokio::test]
async fn personal_deletes_use_singular_task_and_entry_paths() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/sync/task/absent-task"))
        .and(query_param("project_id", "delete-project"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/sync/entry/absent-entry"))
        .and(query_param("project_id", "delete-project"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    init_local_entity_tables(&root);
    let queue = SyncQueue::open(root.path()).unwrap();
    queue.init().unwrap();
    queue
        .enqueue(EntityType::Task, "absent-task", SyncOperation::Delete, None)
        .unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "absent-entry",
            SyncOperation::Delete,
            None,
        )
        .unwrap();

    let syncer = delete_syncer(&root, server.uri());
    tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    assert!(queue.pending(10, 5).unwrap().is_empty());
}

#[tokio::test]
async fn failed_personal_delete_records_status_and_body_for_retry() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/sync/task/retry-task"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad tombstone"))
        .expect(1)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    init_local_entity_tables(&root);
    let queue = SyncQueue::open(root.path()).unwrap();
    queue.init().unwrap();
    queue
        .enqueue(EntityType::Task, "retry-task", SyncOperation::Delete, None)
        .unwrap();

    let syncer = delete_syncer(&root, server.uri());
    tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].retry_count, 1);
    let error = pending[0].last_error.as_deref().unwrap();
    assert!(error.contains("400"), "missing status in {error:?}");
    assert!(
        error.contains("bad tombstone"),
        "missing response body in {error:?}"
    );
}

#[tokio::test]
async fn personal_deletes_for_live_task_and_entry_are_neutralized_without_http() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let entry_store = open_store_local(root.path()).unwrap();
    let task_store = open_task_store_local(root.path()).unwrap();
    entry_store
        .add(&Entry::new(
            "live-entry".to_string(),
            "still here".to_string(),
        ))
        .unwrap();
    task_store
        .add(&Task::new(
            "live-task".to_string(),
            "still here".to_string(),
        ))
        .unwrap();
    let queue = SyncQueue::open(root.path()).unwrap();
    queue.init().unwrap();
    queue
        .enqueue(EntityType::Entry, "live-entry", SyncOperation::Delete, None)
        .unwrap();
    queue
        .enqueue(EntityType::Task, "live-task", SyncOperation::Delete, None)
        .unwrap();

    let syncer = delete_syncer(&root, server.uri());
    let result = tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.pushed_entries, 0);
    assert_eq!(result.pushed_tasks, 0);
    assert!(queue.pending(10, 5).unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn entries_only_push_is_root_and_project_bound() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let root_a = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root_a.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let root_b = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root_b.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    seed_project(&root_a, &server.uri(), "project-a");
    seed_project(&root_b, &server.uri(), "project-b");

    let queue_a = SyncQueue::open(root_a.path()).unwrap();
    enqueue(&queue_a, EntityType::Entry, "a-entry", 8);
    enqueue(&queue_a, EntityType::Task, "a-task", 8);
    let queue_b = SyncQueue::open(root_b.path()).unwrap();
    enqueue(&queue_b, EntityType::Entry, "b-entry", 8);

    let args = CloudPushArgs {
        entries_only: true,
        tasks_only: false,
        dry_run: false,
        max_batches: None,
        rehome: false,
    };
    let cli = common::make_cli_json();
    let root_a_path = root_a.path().to_path_buf();
    tokio::task::spawn_blocking(move || execute_push(&args, &cli, &root_a_path))
        .await
        .unwrap()
        .unwrap();

    assert!(
        queue_a
            .pending_for_entity_type(Some(EntityType::Entry), 10, 5)
            .unwrap()
            .is_empty(),
        "the selected root-A entry must be drained"
    );
    assert_eq!(
        queue_a
            .pending_for_entity_type(Some(EntityType::Task), 10, 5)
            .unwrap()
            .len(),
        1,
        "--entries-only must leave root-A tasks queued"
    );
    assert_eq!(
        queue_b
            .pending_for_entity_type(Some(EntityType::Entry), 10, 5)
            .unwrap()
            .len(),
        1,
        "a root-A push must never consume root-B rows"
    );

    let requests = server.received_requests().await.unwrap();
    let body = decode_gzip_json(&requests[0].body);
    assert_eq!(body["project_canonical_id"], "project-a");
    assert!(
        body.get("git_remote").is_none(),
        "a root without an origin must preserve the legacy envelope shape"
    );
    assert_eq!(body["entries"][0]["id"], "a-entry");
    assert!(body.get("tasks").is_none());
    assert_ne!(body["entries"][0]["id"], "b-entry");
}

#[tokio::test]
async fn personal_push_omits_team_id_even_when_the_project_has_an_active_team() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    seed_project(&root, &server.uri(), "personal-project");
    let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
    enqueue(&queue, EntityType::Entry, "personal-entry", 8);

    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = Some("test-token".to_string());
    config.team_id = Some("active-team-must-not-scope-personal-push".to_string());
    let syncer = CloudSyncer::new_for_project(
        queue,
        config,
        CloudSyncerConfig::default(),
        "personal-project".to_string(),
        root.path(),
    );
    tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let body = decode_gzip_json(&requests[0].body);
    assert_eq!(body["project_canonical_id"], "personal-project");
    assert_eq!(body["entries"][0]["id"], "personal-entry");
    assert!(
        body.get("team_id").is_none(),
        "the personal push path must remain account-scoped — got {body}"
    );
}

#[tokio::test]
async fn personal_push_drains_more_than_two_queue_batches_in_one_invocation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(3)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    seed_project(&root, &server.uri(), "drain-project");
    let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
    for index in 0..101 {
        enqueue(
            &queue,
            EntityType::Entry,
            &format!("drain-entry-{index}"),
            8,
        );
    }

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "drain-project".to_string(),
        root.path(),
    );
    let result = tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.pushed_entries, 101);
    assert!(queue.pending(200, 5).unwrap().is_empty());
}

#[tokio::test]
async fn personal_push_stops_after_a_failed_batch_without_replaying_forever() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
    queue.init().unwrap();
    enqueue(&queue, EntityType::Entry, "stalled-entry", 8);
    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "stalled-project".to_string(),
        root.path(),
    );

    let result = tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.batches_run, 1);
    assert_eq!(result.remaining_backlog.pending, 1);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("no progress"))
    );
    assert_eq!(queue.pending(10, 5).unwrap()[0].retry_count, 1);
}

#[tokio::test]
async fn max_batches_bounds_a_personal_push_without_changing_request_size() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
    queue.init().unwrap();
    for index in 0..101 {
        enqueue(
            &queue,
            EntityType::Entry,
            &format!("bounded-entry-{index}"),
            8,
        );
    }
    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "bounded-project".to_string(),
        root.path(),
    );

    let result =
        tokio::task::spawn_blocking(move || syncer.push_scoped_with_max_batches(PushScope::All, 1))
            .await
            .unwrap()
            .unwrap();

    assert_eq!(result.batches_run, 1);
    assert_eq!(result.pushed_entries, 50);
    assert_eq!(result.remaining_backlog.pending, 51);
    assert_eq!(queue.pending(200, 5).unwrap().len(), 51);
}

/// cas-7719: the shared personal-envelope builder must send the same
/// lowercased remote identity as team push for every supported URL form. Each
/// input also exercises the direct-session envelope, so neither personal
/// route can omit the additive field.
#[tokio::test]
async fn personal_push_envelopes_send_normalized_git_remote_for_all_supported_forms() {
    let server = MockServer::start().await;
    let remotes = [
        (
            "https://GitHub.com/Acme/Widget.git",
            "github.com/acme/widget",
        ),
        (
            "http://GitLab.example.com/Group/Widget/",
            "gitlab.example.com/group/widget",
        ),
        (
            "ssh://git@GitHub.com/Acme/Widget.git",
            "github.com/acme/widget",
        ),
        ("git@GitHub.com:Acme/Widget.git", "github.com/acme/widget"),
    ];
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect((remotes.len() * 2) as u64)
        .mount(&server)
        .await;

    for (index, (remote, _expected)) in remotes.iter().enumerate() {
        let root = TempDir::new().unwrap();
        // Pin the scratch root: the ephemeral-project guard refuses an unpinned
        // root under the temp directory, and a TempDir is exactly that.
        std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
        seed_project(&root, &server.uri(), "same-canonical-id");
        set_origin(&root, remote);
        let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
        enqueue(&queue, EntityType::Entry, &format!("entry-{index}"), 8);

        let mut config = CloudConfig::default();
        config.endpoint = server.uri();
        config.token = Some("test-token".to_string());
        let syncer = CloudSyncer::new_for_project(
            queue,
            config,
            CloudSyncerConfig::default(),
            "same-canonical-id".to_string(),
            root.path(),
        );
        let session = cas::types::Session::new(
            format!("personal-remote-{index}"),
            root.path().display().to_string(),
            None,
        );
        tokio::task::spawn_blocking(move || syncer.push_with_sessions(&[session]))
            .await
            .unwrap()
            .unwrap();
    }

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), remotes.len() * 2);
    for (expected_remote, expected_count) in remotes.iter().map(|(_, expected)| *expected).fold(
        std::collections::BTreeMap::new(),
        |mut counts, remote| {
            *counts.entry(remote).or_insert(0usize) += 2;
            counts
        },
    ) {
        assert_eq!(
            requests
                .iter()
                .map(|request| decode_gzip_json(&request.body))
                .filter(|body| body["git_remote"] == expected_remote)
                .count(),
            expected_count,
            "both queued and session envelopes must carry {expected_remote}"
        );
    }
    for request in requests {
        let body = decode_gzip_json(&request.body);
        assert_eq!(body["project_canonical_id"], "same-canonical-id");
        assert!(body.get("entries").is_some() || body.get("sessions").is_some());
    }
}

#[tokio::test]
async fn failed_cli_push_leaves_the_row_retryable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    seed_project(&root, &server.uri(), "retry-project");
    let queue = SyncQueue::open(root.path()).unwrap();
    enqueue(&queue, EntityType::Entry, "retry-entry", 8);

    let args = CloudPushArgs {
        entries_only: false,
        tasks_only: false,
        dry_run: false,
        max_batches: None,
        rehome: false,
    };
    let cli = common::make_cli_json();
    let root_path = root.path().to_path_buf();
    tokio::task::spawn_blocking(move || execute_push(&args, &cli, &root_path))
        .await
        .unwrap()
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].retry_count, 1, "one failed request is one retry");
    assert!(
        pending[0]
            .last_error
            .as_deref()
            .is_some_and(|e| !e.is_empty())
    );
}

#[test]
fn dry_run_plans_apply_scope_before_the_batch_limit() {
    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
    queue.init().unwrap();
    enqueue(&queue, EntityType::Task, "old-task", 8);
    enqueue(&queue, EntityType::Entry, "entry-1", 8);
    enqueue(&queue, EntityType::Entry, "entry-2", 8);

    let mut config = CloudConfig::default();
    config.token = Some("test-token".to_string());
    let syncer = CloudSyncer::new_for_project(
        queue,
        config,
        CloudSyncerConfig {
            batch_size: 2,
            ..Default::default()
        },
        "planned-project".to_string(),
        root.path(),
    );

    let entries = syncer.plan_push(PushScope::EntriesOnly).unwrap();
    assert_eq!(entries.counts.len(), 1);
    assert_eq!(entries.counts["entries"], 2);
    assert_eq!(entries.total_matching, 2);
    assert_eq!(entries.total_in_next_batch, 2);
    assert!(entries.batch_limit_reached, "count == LIMIT is saturated");
    assert!(!entries.counts.contains_key("tasks"));

    let tasks = syncer.plan_push(PushScope::TasksOnly).unwrap();
    assert_eq!(tasks.counts.len(), 1);
    assert_eq!(tasks.counts["tasks"], 1);
    assert_eq!(tasks.total_matching, 1);
    assert!(!tasks.batch_limit_reached);

    let all = syncer.plan_push(PushScope::All).unwrap();
    assert_eq!(all.total_in_next_batch, 2);
    assert_eq!(all.counts["tasks"], 1);
    assert_eq!(all.counts["entries"], 1);
    assert_eq!(all.total_matching, 3);
    assert!(all.batch_limit_reached);
}

#[tokio::test]
async fn personal_requests_respect_the_exact_serialized_byte_budget() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(3)
        .mount(&server)
        .await;

    let root = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(root.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(root.path()).unwrap());
    queue.init().unwrap();
    for index in 0..3 {
        enqueue(&queue, EntityType::Entry, &format!("large-{index}"), 700);
    }

    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = Some("test-token".to_string());
    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        config,
        CloudSyncerConfig {
            max_payload_bytes: 1_050,
            backoff_base_ms: 1,
            ..Default::default()
        },
        "byte-budget-project".to_string(),
        root.path(),
    );
    let result = tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.pushed_entries, 3);
    assert!(queue.pending(10, 5).unwrap().is_empty());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests {
        let body = decode_gzip_json(&request.body);
        assert!(serde_json::to_vec(&body).unwrap().len() <= 1_050);
        assert_eq!(body["entries"].as_array().unwrap().len(), 1);
    }
}
