//! The hub's idle-store sweep, end to end (cas-f1fb).
//!
//! Two registered stores, each pinned to its own project and linked to its
//! own mock cloud, go through one real sweep that runs the real `cas` binary.
//! Each cloud must see only its own store's identity and rows. The sweep
//! runs both stores from one process, which is the shape that once pushed
//! every project under the launch directory's identity (epic cas-5985).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use cas::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use cas::hub::idle_sync::{
    IdleSyncConfig, IdleSyncState, ProcessSyncRunner, STORE_LOG_FILE, SweepOutcome, run_sweep,
};
use cas::store::{open_store_local, open_task_store_local};
use flate2::read::GzDecoder;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn cas_binary() -> PathBuf {
    cas::test_paths::binary("cas", option_env!("CARGO_BIN_EXE_cas").map(Into::into))
}

async fn mock_cloud() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": "2026-10-16T13:00:00Z",
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "teams": [],
        })))
        .mount(&server)
        .await;
    server
}

/// A registered repo whose store is pinned to `project`, linked to
/// `endpoint`, and holds one queued entry named `entry`.
fn seed_store(parent: &Path, name: &str, endpoint: &str, project: &str, entry: &str) -> PathBuf {
    let repo = parent.join(name);
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(
        cas_root.join("config.toml"),
        format!("[project]\ncanonical_id = \"{project}\"\n"),
    )
    .unwrap();
    let mut cloud = CloudConfig::default();
    cloud.endpoint = endpoint.to_owned();
    cloud.token = Some(format!("{name}-token"));
    cloud.save_to_cas_dir(&cas_root).unwrap();
    open_store_local(&cas_root).unwrap();
    open_task_store_local(&cas_root).unwrap();
    let queue = SyncQueue::open(&cas_root).unwrap();
    queue.init().unwrap();
    let payload = serde_json::json!({
        "id": entry,
        "content": format!("{name} row"),
        "scope": "project",
        "origin_project": project,
    })
    .to_string();
    queue
        .enqueue(
            EntityType::Entry,
            entry,
            SyncOperation::Upsert,
            Some(&payload),
        )
        .unwrap();
    repo
}

fn gunzip_json(body: &[u8]) -> serde_json::Value {
    let mut decoded = Vec::new();
    match GzDecoder::new(body).read_to_end(&mut decoded) {
        Ok(_) => serde_json::from_slice(&decoded).unwrap(),
        Err(_) => serde_json::from_slice(body).unwrap(),
    }
}

fn query(request: &Request, key: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

/// Every request a store's cloud received carries that store's identity
/// and token, and its pushes carry only that store's rows.
async fn assert_only_own_identity(
    server: &MockServer,
    project: &str,
    token: &str,
    own_entry: &str,
    foreign: (&str, &str),
) {
    let requests = server.received_requests().await.unwrap();
    let mut pushes = 0;
    let mut pulls = 0;
    for request in &requests {
        let auth = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            auth,
            format!("Bearer {token}"),
            "{} {}",
            request.method,
            request.url
        );
        if let Some(project_id) = query(request, "project_id") {
            assert_eq!(project_id, project, "{} {}", request.method, request.url);
        }
        let body = String::from_utf8_lossy(&request.body).into_owned();
        assert!(
            !body.contains(foreign.0),
            "foreign project id in {}",
            request.url
        );
        if request.url.path() == "/api/sync/push" {
            pushes += 1;
            let body = gunzip_json(&request.body);
            assert_eq!(body["project_canonical_id"], project);
            let ids: Vec<&str> = body["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["id"].as_str())
                .collect();
            assert_eq!(ids, vec![own_entry]);
            assert!(!body.to_string().contains(foreign.1));
        }
        if request.url.path() == "/api/sync/pull" {
            pulls += 1;
        }
    }
    assert_eq!(pushes, 1, "the store's queued row was pushed once");
    assert!(pulls >= 1, "the store pulled in the same sweep");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_sweep_pushes_and_pulls_each_idle_store_under_its_own_identity() {
    let cloud_a = mock_cloud().await;
    let cloud_b = mock_cloud().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let alpha = seed_store(
        tmp.path(),
        "alpha",
        &cloud_a.uri(),
        "alpha-project",
        "alpha-entry",
    );
    let beta = seed_store(
        tmp.path(),
        "beta",
        &cloud_b.uri(),
        "beta-project",
        "beta-entry",
    );
    let state = tmp.path().join("hub").join("idle-sync.json");
    let runner = ProcessSyncRunner::new(cas_binary())
        .with_env("HOME", &home)
        .with_env("XDG_CONFIG_HOME", home.join(".config"))
        .with_env("XDG_DATA_HOME", home.join(".local/share"))
        .with_env("XDG_CACHE_HOME", home.join(".cache"));

    let repos = vec![alpha.clone(), beta.clone()];
    let sweep_state = state.clone();
    let report = tokio::task::spawn_blocking(move || {
        run_sweep(
            &IdleSyncConfig::default(),
            &repos,
            &runner,
            &sweep_state,
            chrono::Utc::now(),
            &AtomicBool::new(false),
        )
    })
    .await
    .unwrap()
    .unwrap();

    let log = |repo: &Path| {
        std::fs::read_to_string(repo.join(".cas/logs").join(STORE_LOG_FILE)).unwrap_or_default()
    };
    for repo in [&alpha, &beta] {
        let outcome = report
            .ran
            .iter()
            .find(|(swept, _)| swept == repo)
            .map(|(_, o)| *o);
        assert_eq!(outcome, Some(SweepOutcome::Synced), "{}", log(repo));
    }
    assert_only_own_identity(
        &cloud_a,
        "alpha-project",
        "alpha-token",
        "alpha-entry",
        ("beta-project", "beta-entry"),
    )
    .await;
    assert_only_own_identity(
        &cloud_b,
        "beta-project",
        "beta-token",
        "beta-entry",
        ("alpha-project", "alpha-entry"),
    )
    .await;

    for (repo, foreign) in [(&alpha, "beta-project"), (&beta, "alpha-project")] {
        let queue = SyncQueue::open(&repo.join(".cas")).unwrap();
        assert!(
            queue.pending(10, 5).unwrap().is_empty(),
            "the store's queue drained"
        );
        let foreign_keys: Vec<String> = queue
            .list_metadata()
            .unwrap()
            .into_iter()
            .map(|(key, _)| key)
            .filter(|key| key.contains(foreign))
            .collect();
        assert!(
            foreign_keys.is_empty(),
            "foreign watermark keys {foreign_keys:?}"
        );
    }
    let saved = IdleSyncState::load(&state).unwrap();
    assert_eq!(saved.record(&alpha).unwrap().outcome, SweepOutcome::Synced);
    assert_eq!(saved.record(&beta).unwrap().outcome, SweepOutcome::Synced);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_whose_lease_is_held_is_not_synced_twice() {
    let cloud = mock_cloud().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let alpha = seed_store(
        tmp.path(),
        "alpha",
        &cloud.uri(),
        "alpha-project",
        "alpha-entry",
    );
    let state = tmp.path().join("idle-sync.json");
    let runner = ProcessSyncRunner::new(cas_binary()).with_env("HOME", &home);
    let held = cas::cloud::sync_lease::SyncLease::try_acquire(&alpha.join(".cas"))
        .unwrap()
        .expect("the test holds the store's lease, as a serve mid-sync would");

    let repos = vec![alpha.clone()];
    let sweep_state = state.clone();
    let report = tokio::task::spawn_blocking(move || {
        run_sweep(
            &IdleSyncConfig::default(),
            &repos,
            &runner,
            &sweep_state,
            chrono::Utc::now(),
            &AtomicBool::new(false),
        )
    })
    .await
    .unwrap()
    .unwrap();
    drop(held);

    assert_eq!(report.ran, vec![(alpha.clone(), SweepOutcome::Busy)]);
    assert!(
        cloud.received_requests().await.unwrap().is_empty(),
        "nothing reached the cloud while another syncer held the store"
    );
    let queue = SyncQueue::open(&alpha.join(".cas")).unwrap();
    assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
}
