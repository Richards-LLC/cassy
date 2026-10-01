//! Cloud CLI journeys against a local HTTP boundary and real SQLite stores.

use std::path::PathBuf;
use std::process::{Command, Output};

use cas::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use cas::store::{init_cas_dir, open_store_local, open_task_store_local};
use cas::types::{Entry, EntryType, Scope, Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::fixtures::CloudMockServer;
use crate::test_env_guard::TestEnvGuard;

const PROJECT_ID: &str = "cloud-cli-journey";

struct CloudCliFixture {
    project: PathBuf,
    root: PathBuf,
}

impl CloudCliFixture {
    fn new(env: &mut TestEnvGuard, endpoint: &str) -> Self {
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = init_cas_dir(&project).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[project]\ncanonical_id = \"cloud-cli-journey\"\n",
        )
        .unwrap();
        env.set("CAS_ROOT", &root);
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        env.set("CAS_SKIP_FACTORY_TOOLING", "1");
        let config = CloudConfig {
            endpoint: endpoint.to_string(),
            token: Some("test-token".to_string()),
            // Personal scope is deliberate: no ambient memberships or team calls.
            team_auto_promote: Some(false),
            ..Default::default()
        };
        config.save_to_cas_dir(&root).unwrap();
        Self { project, root }
    }

    async fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(cas::test_paths::cas_binary());
        command.current_dir(&self.project).arg("--json").args(args);
        // Keep the server runtime free while the real child performs blocking HTTP.
        tokio::task::spawn_blocking(move || command.output().expect("run cas child"))
            .await
            .unwrap()
    }
}

fn successful_receipt(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "child failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).expect("JSON sync receipt");
    assert_eq!(receipt["status"], "ok", "{receipt}");
    assert_eq!(receipt["errors"], json!([]), "{receipt}");
    receipt
}

fn entry(id: &str, content: &str, created: &str) -> Entry {
    Entry {
        id: id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: content.to_string(),
        origin_project: Some(PROJECT_ID.to_string()),
        created: DateTime::parse_from_rfc3339(created)
            .unwrap()
            .with_timezone(&Utc),
        ..Default::default()
    }
}

fn scoped_row(row: impl serde::Serialize) -> Value {
    let mut value = serde_json::to_value(row).unwrap();
    value["project_id"] = json!(PROJECT_ID);
    value["origin_project"] = json!(PROJECT_ID);
    value
}

#[tokio::test]
async fn cloud_push_uploads_queued_entry_and_persists_acknowledgement() {
    let mut env = TestEnvGuard::temp_home();
    let server = CloudMockServer::start().await;
    server.mock_push_success(1, 0, 0, 0).await;
    let fixture = CloudCliFixture::new(&mut env, &server.endpoint);
    let local = entry(
        "push-entry",
        "Memory uploaded by CLI",
        "2020-01-01T00:00:00Z",
    );
    open_store_local(&fixture.root)
        .unwrap()
        .add(&local)
        .unwrap();
    assert_eq!(
        open_store_local(&fixture.root)
            .unwrap()
            .get(&local.id)
            .unwrap()
            .origin_project
            .as_deref(),
        Some(PROJECT_ID),
        "push attribution must exist on the stored entry before enqueueing"
    );
    let queue = SyncQueue::open(&fixture.root).unwrap();
    queue.init().unwrap();
    let payload = serde_json::to_value(&local).unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            &local.id,
            SyncOperation::Upsert,
            Some(&payload.to_string()),
        )
        .unwrap();
    assert_eq!(queue.stats(3).unwrap().pending, 1);
    drop(queue);

    let output = fixture.run(&["cloud", "push"]).await;

    let receipt = successful_receipt(&output);
    assert_eq!(receipt["total_pushed"], 1);
    server.server.verify().await;
    let uploaded = server.pushed_payload().await;
    assert_eq!(uploaded["project_canonical_id"], PROJECT_ID);
    assert_eq!(uploaded["entries"].as_array().unwrap().len(), 1);
    assert_eq!(uploaded["entries"][0]["id"], "push-entry");
    assert_eq!(uploaded["entries"][0]["content"], "Memory uploaded by CLI");
    assert_eq!(uploaded["entries"][0]["origin_project"], PROJECT_ID);
    let stored = open_store_local(&fixture.root)
        .unwrap()
        .get("push-entry")
        .unwrap();
    assert_eq!(stored.content, "Memory uploaded by CLI");
    assert_eq!(stored.origin_project.as_deref(), Some(PROJECT_ID));
    let queue = SyncQueue::open(&fixture.root).unwrap();
    assert_eq!(
        queue.stats(3).unwrap().total,
        0,
        "acknowledged row must leave the durable queue"
    );
    assert_eq!(
        queue
            .get_metadata("last_push_canonical_id")
            .unwrap()
            .as_deref(),
        Some(PROJECT_ID)
    );
}

#[tokio::test]
async fn cloud_pull_imports_remote_entry_and_task_into_store() {
    let mut env = TestEnvGuard::temp_home();
    let server = CloudMockServer::start().await;
    server
        .mock_pull_with_data(
            PROJECT_ID,
            vec![scoped_row(entry(
                "remote-entry",
                "Remote learning",
                "2020-01-01T00:00:00Z",
            ))],
            vec![scoped_row(Task::new(
                "cas-c001".to_string(),
                "Remote task".to_string(),
            ))],
        )
        .await;
    let fixture = CloudCliFixture::new(&mut env, &server.endpoint);
    assert!(
        open_store_local(&fixture.root)
            .unwrap()
            .get("remote-entry")
            .is_err()
    );
    assert!(
        open_task_store_local(&fixture.root)
            .unwrap()
            .get("cas-c001")
            .is_err()
    );

    let output = fixture.run(&["cloud", "pull"]).await;

    let receipt = successful_receipt(&output);
    assert_eq!(receipt["entries"], 1);
    assert_eq!(receipt["tasks"], 1);
    server.server.verify().await;
    let stored = open_store_local(&fixture.root)
        .unwrap()
        .get("remote-entry")
        .unwrap();
    assert_eq!(stored.content, "Remote learning");
    assert_eq!(stored.entry_type, EntryType::Learning);
    assert_eq!(stored.origin_project.as_deref(), Some(PROJECT_ID));
    let task = open_task_store_local(&fixture.root)
        .unwrap()
        .get("cas-c001")
        .unwrap();
    assert_eq!(task.title, "Remote task");
    assert_eq!(task.status, TaskStatus::Open);
    let queue = SyncQueue::open(&fixture.root).unwrap();
    assert_eq!(
        queue.get_metadata("last_pull_at").unwrap().as_deref(),
        Some("2026-09-30T00:00:00Z")
    );
    assert_eq!(
        queue.stats(3).unwrap().total,
        0,
        "pull must not enqueue echoes"
    );
}

#[tokio::test]
async fn cloud_pull_persists_remote_and_local_conflict_winners() {
    let mut env = TestEnvGuard::temp_home();
    let server = CloudMockServer::start().await;
    let mut newer = scoped_row(entry(
        "remote-wins",
        "New remote content",
        "2020-01-01T00:00:00Z",
    ));
    // Independent ordering fixtures: the remote update is later than the local
    // insertion; the other remote row predates both local insertions.
    newer["updated_at"] = json!("2099-01-01T00:00:00Z");
    let mut older = scoped_row(entry(
        "local-wins",
        "Stale remote content",
        "2000-01-01T00:00:00Z",
    ));
    older["updated_at"] = json!("2000-01-01T00:00:00Z");
    server
        .mock_pull_with_data(PROJECT_ID, vec![newer, older], vec![])
        .await;
    let fixture = CloudCliFixture::new(&mut env, &server.endpoint);
    let store = open_store_local(&fixture.root).unwrap();
    store
        .add(&entry(
            "remote-wins",
            "Old local content",
            "2020-01-01T00:00:00Z",
        ))
        .unwrap();
    store
        .add(&entry(
            "local-wins",
            "New local content",
            "2020-01-01T00:00:00Z",
        ))
        .unwrap();
    drop(store);

    let output = fixture.run(&["cloud", "pull"]).await;

    let receipt = successful_receipt(&output);
    assert_eq!(
        receipt["entries"], 1,
        "only the newer remote row is applied"
    );
    assert_eq!(receipt["conflicts_resolved_remote"], 1);
    assert_eq!(receipt["conflicts_resolved_local"], 1);
    server.server.verify().await;
    let store = open_store_local(&fixture.root).unwrap();
    assert_eq!(
        store.get("remote-wins").unwrap().content,
        "New remote content"
    );
    assert_eq!(
        store.get("local-wins").unwrap().content,
        "New local content"
    );
    for id in ["remote-wins", "local-wins"] {
        assert_eq!(
            store.get(id).unwrap().origin_project.as_deref(),
            Some(PROJECT_ID)
        );
    }
}

#[tokio::test]
async fn cloud_pull_without_login_is_refused_before_http() {
    let mut env = TestEnvGuard::temp_home();
    let server = CloudMockServer::start().await;
    server.expect_no_api_requests().await;
    let fixture = CloudCliFixture::new(&mut env, &server.endpoint);
    let mut config = CloudConfig::load_from_cas_dir(&fixture.root).unwrap();
    config.token = None;
    config.save_to_cas_dir(&fixture.root).unwrap();

    let output = fixture.run(&["cloud", "pull"]).await;

    assert!(
        !output.status.success(),
        "missing login must fail the child"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Not logged in"));
    server.server.verify().await;
    assert!(server.server.received_requests().await.unwrap().is_empty());
    let queue = SyncQueue::open(&fixture.root).unwrap();
    queue.init().unwrap();
    assert!(queue.get_metadata("last_pull_at").unwrap().is_none());
}

#[tokio::test]
async fn cloud_pull_rejected_token_fails_and_preserves_local_row() {
    let mut env = TestEnvGuard::temp_home();
    let server = CloudMockServer::start().await;
    server.mock_pull_refusal(PROJECT_ID, 401).await;
    let fixture = CloudCliFixture::new(&mut env, &server.endpoint);
    open_store_local(&fixture.root)
        .unwrap()
        .add(&entry(
            "protected-entry",
            "Keep local content",
            "2020-01-01T00:00:00Z",
        ))
        .unwrap();

    let output = fixture.run(&["cloud", "pull"]).await;

    assert!(
        !output.status.success(),
        "HTTP auth refusal must fail the child"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Pull failed with status 401"));
    server.server.verify().await;
    assert_eq!(
        open_store_local(&fixture.root)
            .unwrap()
            .get("protected-entry")
            .unwrap()
            .content,
        "Keep local content"
    );
    assert!(
        SyncQueue::open(&fixture.root)
            .unwrap()
            .get_metadata("last_pull_at")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn cloud_team_only_refuses_unavailable_personal_push_scope() {
    let mut env = TestEnvGuard::temp_home();
    let server = CloudMockServer::start().await;
    server.expect_no_api_requests().await;
    let fixture = CloudCliFixture::new(&mut env, &server.endpoint);
    let mut config = CloudConfig::load_from_cas_dir(&fixture.root).unwrap();
    config.team_auto_promote = None;
    config.set_team("550e8400-e29b-41d4-a716-446655440000", "fixture-team");
    config.team_only = true;
    config.save_to_cas_dir(&fixture.root).unwrap();

    let output = fixture.run(&["cloud", "push", "--entries-only"]).await;

    assert!(
        !output.status.success(),
        "unavailable personal scope must fail the child"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("scoped personal push flags are unavailable")
    );
    server.server.verify().await;
    assert!(server.server.received_requests().await.unwrap().is_empty());
}
