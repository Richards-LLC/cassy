//! Regression coverage for GH #633: team-pull entries share IDs with the
//! author's personal rows and must reconcile by last-writer-wins.

mod common;

use cas::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig};
use cas::store::{
    open_rule_store_local, open_skill_store_local, open_store_local, open_task_store_local,
};
use cas::types::{Entry, EntryType, Scope};
use chrono::{DateTime, Utc};
use common::TEST_TEAM;
use std::sync::Arc;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn entry(id: &str, content: &str, created: &str) -> Entry {
    Entry {
        id: id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: content.to_string(),
        created: DateTime::parse_from_rfc3339(created)
            .unwrap()
            .with_timezone(&Utc),
        ..Default::default()
    }
}

fn with_project_id(entry: Entry, project_id: &str) -> serde_json::Value {
    let mut value = serde_json::to_value(&entry).unwrap();
    value["project_id"] = serde_json::json!(project_id);
    value
}

fn with_updated_at(mut entry: serde_json::Value, updated_at: DateTime<Utc>) -> serde_json::Value {
    entry["updated_at"] = serde_json::json!(updated_at.to_rfc3339());
    entry
}

#[tokio::test]
async fn team_pull_reconciles_same_id_entries_with_lww_and_skips_foreign_rows() {
    let server = MockServer::start().await;
    let project_id = "cas-633-project";
    let now = Utc::now();

    let remote_personal_newer = with_updated_at(
        with_project_id(
            entry(
                "same-id-personal-newer",
                "stale team copy",
                &(now - chrono::Duration::hours(2)).to_rfc3339(),
            ),
            project_id,
        ),
        now - chrono::Duration::minutes(30),
    );
    let remote_team_newer = with_updated_at(
        with_project_id(
            entry(
                "same-id-team-newer",
                "new team copy",
                &(now + chrono::Duration::hours(1)).to_rfc3339(),
            ),
            project_id,
        ),
        now + chrono::Duration::hours(1),
    );
    let remote_unchanged = with_updated_at(
        with_project_id(
            entry(
                "same-id-unchanged",
                "unchanged copy",
                &(now - chrono::Duration::hours(1)).to_rfc3339(),
            ),
            project_id,
        ),
        now - chrono::Duration::hours(1),
    );
    let foreign_same_id = with_updated_at(
        with_project_id(
            entry(
                "same-id-cross-project",
                "foreign project copy",
                &(now + chrono::Duration::hours(2)).to_rfc3339(),
            ),
            "other-project",
        ),
        now + chrono::Duration::hours(2),
    );

    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [
                remote_personal_newer,
                remote_team_newer,
                remote_unchanged,
                foreign_same_id,
            ],
            "tasks": [],
            "rules": [],
            "skills": [],
            "pulled_at": "2026-08-14T00:00:00Z",
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(cas::cloud::SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    let store = open_store_local(tmp.path()).unwrap();
    let task_store = open_task_store_local(tmp.path()).unwrap();
    let rule_store = open_rule_store_local(tmp.path()).unwrap();
    let skill_store = open_skill_store_local(tmp.path()).unwrap();

    store
        .add(&entry(
            "same-id-personal-newer",
            "new personal copy",
            &(now - chrono::Duration::hours(1)).to_rfc3339(),
        ))
        .unwrap();
    store
        .add(&entry(
            "same-id-team-newer",
            "old personal copy",
            &(now - chrono::Duration::hours(3)).to_rfc3339(),
        ))
        .unwrap();
    store
        .add(&entry(
            "same-id-unchanged",
            "unchanged copy",
            &(now - chrono::Duration::hours(1)).to_rfc3339(),
        ))
        .unwrap();

    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = Some("test-token".to_string());
    config.set_team(TEST_TEAM, "test-team");
    let syncer = CloudSyncer::new(queue, config, CloudSyncerConfig::default());

    let result = tokio::task::spawn_blocking(move || {
        syncer.pull_team(
            TEST_TEAM,
            project_id,
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
        )
    })
    .await
    .unwrap()
    .unwrap();

    assert!(
        result.errors.is_empty(),
        "unexpected pull errors: {:?}",
        result.errors
    );
    assert_eq!(result.pulled_entries, 1, "only the newer team copy applies");
    assert_eq!(
        result.conflicts_resolved_local, 2,
        "older and equal rows are no-ops"
    );
    assert_eq!(
        result.conflicts_resolved_remote, 1,
        "the newer team copy is a remote-win conflict"
    );
    assert_eq!(result.conflicts_resolved, 3, "total counts both directions");

    let store = open_store_local(tmp.path()).unwrap();
    assert_eq!(
        store.get("same-id-personal-newer").unwrap().content,
        "new personal copy"
    );
    assert_eq!(
        store.get("same-id-team-newer").unwrap().content,
        "new team copy"
    );
    assert_eq!(
        store.get("same-id-unchanged").unwrap().content,
        "unchanged copy"
    );
    assert!(
        store.get("same-id-cross-project").is_err(),
        "foreign project row must not be imported"
    );
}
