//! GH #909 (cas-3a90): pulled entries, rules and skills keep their origin and
//! are never re-pushed under the puller's project.
//!
//! Project B ("p") pulls rows authored in project A. The cloud stamps every
//! returned row with `project_id = p` (it echoes the requested scope), so a
//! legacy row without `origin_project` looks native. Local writes then enqueue
//! those rows, and the next push must not publish any of them under B.

mod common;

use std::io::Read;
use std::sync::Arc;

use cas::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType, SyncOperation};
use cas::store::{
    open_rule_store_local, open_skill_store_local, open_store_local, open_task_store_local,
};
use cas::types::{Entry, EntryType, Rule, Scope, Skill};
use common::TEST_TEAM;
use flate2::read::GzDecoder;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const B: &str = "p";

fn entry_json(id: &str, origin: Option<&str>) -> serde_json::Value {
    let entry = Entry {
        id: id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: format!("content of {id}"),
        created: chrono::Utc::now() - chrono::Duration::hours(1),
        ..Default::default()
    };
    stamped(serde_json::to_value(entry).unwrap(), origin)
}

/// The cloud echoes the requested scope into `project_id`; only the author's
/// push can supply `origin_project`.
fn stamped(mut row: serde_json::Value, origin: Option<&str>) -> serde_json::Value {
    row["project_id"] = serde_json::json!(B);
    if let Some(origin) = origin {
        row["origin_project"] = serde_json::json!(origin);
    }
    row
}

fn decoded_bodies(requests: &[wiremock::Request]) -> Vec<serde_json::Value> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .map(|request| {
            let mut decoded = Vec::new();
            GzDecoder::new(request.body.as_slice())
                .read_to_end(&mut decoded)
                .unwrap();
            serde_json::from_slice(&decoded).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn rows_pulled_from_another_project_are_never_pushed_under_the_puller() {
    let server = MockServer::start().await;
    let legacy_rule = stamped(
        serde_json::to_value(Rule::new("rule-909-a".to_string(), "A's rule".to_string())).unwrap(),
        None,
    );
    let legacy_skill = stamped(
        serde_json::to_value(Skill::new("skill-909-a".to_string(), "a-skill".to_string())).unwrap(),
        None,
    );
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [
                entry_json("entry-909-a-legacy", None),
                entry_json("entry-909-a-stamped", Some("project-a")),
                entry_json("entry-909-b-own", Some(B)),
            ],
            "tasks": [],
            "rules": [legacy_rule],
            "skills": [legacy_skill],
            "pulled_at": "2026-09-24T12:00:00Z",
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(&server)
        .await;
    for push_path in [
        format!("/api/teams/{TEST_TEAM}/sync/push"),
        "/api/sync/push".to_string(),
    ] {
        Mock::given(method("POST"))
            .and(path(push_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
    }

    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        format!("[project]\ncanonical_id = \"{B}\"\n"),
    )
    .unwrap();
    let queue = Arc::new(cas::cloud::SyncQueue::open(tmp.path()).unwrap());
    queue.init().unwrap();
    let store = open_store_local(tmp.path()).unwrap();
    let task_store = open_task_store_local(tmp.path()).unwrap();
    let rule_store = open_rule_store_local(tmp.path()).unwrap();
    let skill_store = open_skill_store_local(tmp.path()).unwrap();

    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = Some("test-token".to_string());
    config.set_team(TEST_TEAM, "test-team");
    let syncer = Arc::new(CloudSyncer::new_for_project(
        Arc::clone(&queue),
        config,
        CloudSyncerConfig::default(),
        B.to_string(),
        tmp.path(),
    ));

    let pull_syncer = Arc::clone(&syncer);
    let (result, stores) = tokio::task::spawn_blocking(move || {
        let result = pull_syncer
            .pull_team(
                TEST_TEAM,
                B,
                store.as_ref(),
                task_store.as_ref(),
                rule_store.as_ref(),
                skill_store.as_ref(),
            )
            .unwrap();
        (result, (store, rule_store, skill_store))
    })
    .await
    .unwrap();
    assert!(result.errors.is_empty(), "pull errors: {:?}", result.errors);
    let (store, rule_store, skill_store) = stores;

    // A's stamped row is refused; A's legacy rows are admitted but recorded.
    assert!(store.get("entry-909-a-stamped").is_err());
    assert!(store.get("entry-909-a-legacy").is_ok());
    assert!(store.get("entry-909-b-own").is_ok());
    assert!(rule_store.get("rule-909-a").is_ok());
    assert!(skill_store.get("skill-909-a").is_ok());
    assert!(
        queue
            .is_unauthored_pull("entry", "entry-909-a-legacy")
            .unwrap()
    );
    assert!(queue.is_unauthored_pull("rule", "rule-909-a").unwrap());
    assert!(queue.is_unauthored_pull("skill", "skill-909-a").unwrap());
    assert!(
        !queue
            .is_unauthored_pull("entry", "entry-909-b-own")
            .unwrap()
    );
    let counts = queue.unauthored_pull_counts().unwrap();
    assert_eq!(counts.get("entry"), Some(&1));
    assert_eq!(counts.get("rule"), Some(&1));
    assert_eq!(counts.get("skill"), Some(&1));

    // Local writes after the pull (a decay pass, an access bump) enqueue every
    // row for both the personal and the team push.
    let enqueue = |entity_type: EntityType, id: &str, payload: serde_json::Value| {
        let payload = payload.to_string();
        queue
            .enqueue(entity_type, id, SyncOperation::Upsert, Some(&payload))
            .unwrap();
        queue
            .enqueue_for_team(
                entity_type,
                id,
                SyncOperation::Upsert,
                Some(&payload),
                TEST_TEAM,
            )
            .unwrap();
    };
    for id in ["entry-909-a-legacy", "entry-909-b-own"] {
        enqueue(
            EntityType::Entry,
            id,
            serde_json::to_value(store.get(id).unwrap()).unwrap(),
        );
    }
    enqueue(
        EntityType::Rule,
        "rule-909-a",
        serde_json::to_value(rule_store.get("rule-909-a").unwrap()).unwrap(),
    );
    enqueue(
        EntityType::Skill,
        "skill-909-a",
        serde_json::to_value(skill_store.get("skill-909-a").unwrap()).unwrap(),
    );

    let push_syncer = Arc::clone(&syncer);
    tokio::task::spawn_blocking(move || {
        let _ = push_syncer.push_team(TEST_TEAM);
        let _ = push_syncer.push();
    })
    .await
    .unwrap();

    let bodies = decoded_bodies(&server.received_requests().await.unwrap());
    assert!(
        !bodies.is_empty(),
        "the project's own entry is still pushed"
    );
    let everything = serde_json::to_string(&bodies).unwrap();
    for foreign in [
        "entry-909-a-legacy",
        "entry-909-a-stamped",
        "rule-909-a",
        "skill-909-a",
    ] {
        assert!(
            !everything.contains(foreign),
            "{foreign} must never be pushed under {B}: {everything}"
        );
    }
    // The project's own entry goes out, stamped with the project that authored it.
    let own: Vec<&serde_json::Value> = bodies
        .iter()
        .filter_map(|body| body["entries"].as_array())
        .flatten()
        .filter(|row| row["id"] == "entry-909-b-own")
        .collect();
    assert_eq!(own.len(), 2, "one personal and one team push: {everything}");
    for row in own {
        assert_eq!(row["origin_project"], B);
    }
}

#[test]
fn a_later_pull_with_this_projects_origin_clears_the_marker() {
    let tmp = TempDir::new().unwrap();
    let queue = cas::cloud::SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    assert!(
        queue
            .record_unauthored_pull("entry", "e-1", "test")
            .unwrap()
    );
    assert!(
        !queue
            .record_unauthored_pull("entry", "e-1", "again")
            .unwrap()
    );
    queue
        .enqueue(EntityType::Entry, "e-1", SyncOperation::Upsert, Some("{}"))
        .unwrap();
    queue
        .enqueue(EntityType::Entry, "e-2", SyncOperation::Upsert, Some("{}"))
        .unwrap();
    assert_eq!(queue.drop_queued_pushes_for_unauthored_pulls().unwrap(), 1);
    assert!(queue.forget_unauthored_pull("entry", "e-1").unwrap());
    assert!(!queue.is_unauthored_pull("entry", "e-1").unwrap());
    assert!(queue.unauthored_pull_counts().unwrap().is_empty());
}
