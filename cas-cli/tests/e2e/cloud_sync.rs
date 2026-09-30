//! Cloud CLI journeys against a local HTTP boundary and real SQLite stores.

use std::path::PathBuf;
use std::process::{Command, Output};

use cas::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use cas::store::{init_cas_dir, open_store_local};
use cas::types::{Entry, EntryType, Scope};
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
        created: DateTime::parse_from_rfc3339(created)
            .unwrap()
            .with_timezone(&Utc),
        ..Default::default()
    }
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
    let queue = SyncQueue::open(&fixture.root).unwrap();
    queue.init().unwrap();
    let mut payload = serde_json::to_value(&local).unwrap();
    payload["origin_project"] = json!(PROJECT_ID);
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
    let stored = open_store_local(&fixture.root)
        .unwrap()
        .get("push-entry")
        .unwrap();
    assert_eq!(stored.content, "Memory uploaded by CLI");
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
