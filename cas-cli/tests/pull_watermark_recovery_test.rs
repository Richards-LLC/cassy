//! Regression coverage for GH #192 / cas-ed01.
//!
//! A first pull can legitimately return an empty envelope when a new machine
//! initially resolves the wrong canonical project id. That empty response must
//! not create a watermark, otherwise correcting the id still sends `since=` and
//! permanently skips the historical backfill.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use cas::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};
use cas::store::{
    open_commit_link_store, open_event_store, open_file_change_store, open_prompt_store,
    open_rule_store_local, open_skill_store_local, open_spec_store, open_store_local,
    open_task_store_local,
};
use cas::types::{Entry, EntryType, Scope, Task, TaskStatus};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cas_cmd() -> Command {
    Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ))
}

#[test]
fn cloud_sync_help_exposes_full_repull_escape_hatch() {
    cas_cmd()
        .args(["cloud", "sync", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Ignore pull watermarks and re-pull all data",
        ));
}

#[tokio::test]
async fn empty_first_pull_then_corrected_bucket_backfills_without_metadata_surgery() {
    let server = MockServer::start().await;
    let project_id = cas::cloud::get_project_canonical_id()
        .expect("test must run inside a project with a canonical id");

    let entry = Entry {
        id: "recovered-after-id-fix".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: "historical row from the corrected project bucket".to_string(),
        ..Default::default()
    };
    let mut entry_json = serde_json::to_value(entry).unwrap();
    entry_json["project_id"] = serde_json::json!(project_id);

    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", project_id.as_str()))
        .and(query_param_is_missing("since"))
        .respond_with(move |_: &wiremock::Request| {
            let first_empty_bucket = responder_calls.fetch_add(1, Ordering::SeqCst) == 0;
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": if first_empty_bucket { Vec::new() } else { vec![entry_json.clone()] },
                "tasks": [], "rules": [], "skills": [],
                "specs": [], "events": [], "prompts": [],
                "file_changes": [], "commit_links": [],
                "pulled_at": if first_empty_bucket {
                    "2026-08-09T18:00:00Z"
                } else {
                    "2026-08-09T18:05:00Z"
                },
            }))
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", project_id.as_str()))
        .and(query_param("since", "2026-08-09T18:05:00Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": "2026-08-09T18:10:00Z",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let store = open_store_local(&cas_root).unwrap();
    let task_store = open_task_store_local(&cas_root).unwrap();
    let rule_store = open_rule_store_local(&cas_root).unwrap();
    let skill_store = open_skill_store_local(&cas_root).unwrap();
    let spec_store = open_spec_store(&cas_root).unwrap();
    let event_store = open_event_store(&cas_root).unwrap();
    let prompt_store = open_prompt_store(&cas_root).unwrap();
    let file_change_store = open_file_change_store(&cas_root).unwrap();
    let commit_link_store = open_commit_link_store(&cas_root).unwrap();
    let queue = Arc::new(SyncQueue::open(&cas_root).unwrap());
    queue.init().unwrap();

    let syncer = CloudSyncer::new(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
    );
    let pull = || {
        syncer.pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )
    };

    let empty = pull().expect("wrong project bucket returns a valid empty envelope");
    assert_eq!(empty.total_pulled(), 0);
    assert_eq!(
        queue.get_metadata("last_pull_at").unwrap(),
        None,
        "an empty first pull must not poison the next request with since="
    );

    let recovered = pull().expect("corrected project bucket must be fully re-pulled");
    assert_eq!(recovered.pulled_entries, 1);
    assert_eq!(
        queue.get_metadata("last_pull_at").unwrap().as_deref(),
        Some("2026-08-09T18:05:00Z"),
        "the first rows-applied pull establishes the incremental watermark"
    );
    assert_eq!(
        store.get("recovered-after-id-fix").unwrap().content,
        "historical row from the corrected project bucket"
    );

    let healthy_incremental = pull().expect("healthy incremental empty pull succeeds");
    assert_eq!(healthy_incremental.total_pulled(), 0);
    assert_eq!(
        queue.get_metadata("last_pull_at").unwrap().as_deref(),
        Some("2026-08-09T18:10:00Z"),
        "once a project has a successful watermark, empty incremental pulls still advance it"
    );
}

/// cas-b4fc / GH #516: a terminal task rejected for an unattributed remote
/// reopen must be durably journaled and must advance the pull watermark.  The
/// following incremental pull therefore cannot receive that old bad row again.
#[tokio::test]
async fn unattributed_terminal_reopen_is_journaled_once_and_not_retried_next_pull() {
    let server = MockServer::start().await;
    let project_id = cas::cloud::get_project_canonical_id()
        .expect("test must run inside a project with a canonical id");
    let initial_pulled_at = "2026-08-20T15:10:00Z";

    let mut remote = Task::new(
        "cas-unattributed-two-cycle".to_string(),
        "remote zombie reopen".to_string(),
    );
    remote.status = TaskStatus::Open;
    remote.updated_at = chrono::Utc::now() + chrono::Duration::minutes(1);
    remote.notes = "[2026-08-20 15:09] Reopened: missing audit fields".to_string();
    let mut remote_json = serde_json::to_value(remote).unwrap();
    remote_json["project_id"] = serde_json::json!(project_id);
    remote_json["project_canonical_id"] = serde_json::json!(project_id);

    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", project_id.as_str()))
        .and(query_param_is_missing("since"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [remote_json], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": initial_pulled_at,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", project_id.as_str()))
        .and(query_param("since", initial_pulled_at))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": "2026-08-20T15:11:00Z",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let store = open_store_local(&cas_root).unwrap();
    let task_store = open_task_store_local(&cas_root).unwrap();
    let rule_store = open_rule_store_local(&cas_root).unwrap();
    let skill_store = open_skill_store_local(&cas_root).unwrap();
    let spec_store = open_spec_store(&cas_root).unwrap();
    let event_store = open_event_store(&cas_root).unwrap();
    let prompt_store = open_prompt_store(&cas_root).unwrap();
    let file_change_store = open_file_change_store(&cas_root).unwrap();
    let commit_link_store = open_commit_link_store(&cas_root).unwrap();
    let queue = Arc::new(SyncQueue::open(&cas_root).unwrap());
    queue.init().unwrap();

    let mut local = Task::new(
        "cas-unattributed-two-cycle".to_string(),
        "locally closed task".to_string(),
    );
    local.status = TaskStatus::Closed;
    local.closed_at = Some(
        chrono::DateTime::parse_from_rfc3339("2026-08-20T15:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    task_store.add(&local).unwrap();

    let syncer = CloudSyncer::new(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
    );
    let pull = || {
        syncer.pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )
    };

    let rejected = pull().expect("unattributed reopen response is handled");
    assert_eq!(rejected.conflicts_resolved, 1);
    let conflicts = queue.list_conflicts(10).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].strategy, "terminal_status_guard");
    assert_eq!(
        queue.get_metadata("last_pull_at").unwrap().as_deref(),
        Some(initial_pulled_at),
        "the rejected row still advances the successful pull watermark"
    );
    assert_eq!(
        task_store.get("cas-unattributed-two-cycle").unwrap().status,
        TaskStatus::Closed,
    );

    let next_cycle = pull().expect("incremental follow-up pull succeeds");
    assert_eq!(next_cycle.conflicts_resolved, 0);
    assert_eq!(
        queue.list_conflicts(10).unwrap().len(),
        1,
        "the rejected terminal reopen is journaled once, not retried"
    );
    assert_eq!(
        queue.get_metadata("last_pull_at").unwrap().as_deref(),
        Some("2026-08-20T15:11:00Z"),
        "the healthy second cycle advances from the rejected row's watermark"
    );
}
