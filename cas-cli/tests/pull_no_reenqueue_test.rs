//! Regression for cas-7fbb: pull must not re-enqueue pulled rows.
//!
//! Root cause: `open_store` / `open_task_store` / etc. wrap writes in
//! Syncing* when logged in. Pull used those openers, so every remote
//! upsert re-entered SyncQueue → push → (echo) pull → unbounded growth.
//!
//! Fix: pull / team-pull / daemon cloud-sync apply via `open_*_local`.
//! These tests lock in:
//! 1. Behavioral: CloudSyncer::pull of N entries via local openers does
//!    not grow SyncQueue pending count by N.
//! 2. Behavioral: open_store (syncing wrap) still enqueues local edits.
//! 3. Source-level: execute_pull / execute_team_pull / daemon run_cloud_sync
//!    call the *_local openers (not the wrapping ones) for apply stores.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use cas::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};
use cas::store::{
    open_commit_link_store, open_event_store, open_file_change_store, open_prompt_store,
    open_rule_store_local, open_skill_store_local, open_spec_store, open_store, open_store_local,
    open_task_store_local,
};
use cas::types::{Entry, EntryType, Scope};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MAX_RETRIES: i32 = 5;
const FIXTURE_PROJECT_ID: &str = "p";

fn production_source_root() -> PathBuf {
    cas::test_paths::crate_root().join("src")
}

fn make_logged_in_cas_root(endpoint: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path();
    // Force SQLite store files into existence.
    let _ = open_store_local(cas_root).unwrap();
    let _ = open_task_store_local(cas_root).unwrap();
    let _ = open_rule_store_local(cas_root).unwrap();
    let _ = open_skill_store_local(cas_root).unwrap();

    let queue = SyncQueue::open(cas_root).unwrap();
    queue.init().unwrap();

    let mut cfg = CloudConfig::default();
    cfg.endpoint = endpoint.to_string();
    cfg.token = Some("test-token".to_string());
    cfg.save_to_cas_dir(cas_root).unwrap();

    tmp
}

fn entry_payload(id: &str, project_id: &str) -> serde_json::Value {
    let entry = Entry {
        id: id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: format!("remote content for {id}"),
        ..Default::default()
    };
    let mut v = serde_json::to_value(&entry).unwrap();
    v["project_id"] = serde_json::json!(project_id);
    v["project_canonical_id"] = serde_json::json!(project_id);
    v
}

/// cas-7fbb AC2: pull of N remote rows must not grow SyncQueue by N.
#[tokio::test]
async fn pull_via_local_openers_does_not_grow_sync_queue() {
    let server = MockServer::start().await;
    let project_id = FIXTURE_PROJECT_ID;

    let n = 5usize;
    let entries: Vec<serde_json::Value> = (0..n)
        .map(|i| entry_payload(&format!("remote-entry-{i:03}"), &project_id))
        .collect();

    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": entries,
            "tasks": [],
            "rules": [],
            "skills": [],
            "events": [],
            "prompts": [],
            "file_changes": [],
            "commit_links": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = make_logged_in_cas_root(&server.uri());
    let cas_root = tmp.path();

    // Logged-in so open_store *would* wrap — but pull uses local openers.
    let cfg = CloudConfig::load_from_cas_dir(cas_root).unwrap();
    assert!(cfg.is_logged_in(), "fixture must be logged in");

    let queue = SyncQueue::open(cas_root).unwrap();
    let pending_before = queue.pending_count(MAX_RETRIES).unwrap();
    assert_eq!(pending_before, 0, "queue starts empty");

    let store = open_store_local(cas_root).unwrap();
    let task_store = open_task_store_local(cas_root).unwrap();
    let rule_store = open_rule_store_local(cas_root).unwrap();
    let skill_store = open_skill_store_local(cas_root).unwrap();
    let spec_store = open_spec_store(cas_root).unwrap();
    let event_store = open_event_store(cas_root).unwrap();
    let prompt_store = open_prompt_store(cas_root).unwrap();
    let file_change_store = open_file_change_store(cas_root).unwrap();
    let commit_link_store = open_commit_link_store(cas_root).unwrap();

    let syncer = CloudSyncer::new(
        Arc::new(SyncQueue::open(cas_root).unwrap()),
        cfg,
        CloudSyncerConfig::default(),
    );

    let result = syncer
        .pull(
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
        .expect("pull should succeed");

    assert!(
        result.errors.is_empty(),
        "pull errors: {:?}",
        result.errors
    );
    assert_eq!(
        result.pulled_entries, n,
        "expected {n} entries pulled, got {}",
        result.pulled_entries
    );

    // Rows landed locally.
    let landed = open_store_local(cas_root).unwrap().list().unwrap();
    assert!(
        landed.len() >= n,
        "expected at least {n} local entries, got {}",
        landed.len()
    );

    let pending_after = SyncQueue::open(cas_root)
        .unwrap()
        .pending_count(MAX_RETRIES)
        .unwrap();
    assert_eq!(
        pending_after, pending_before,
        "cas-7fbb: pull of {n} rows must not grow SyncQueue (before={pending_before}, after={pending_after})"
    );
}

/// cas-7fbb AC5: legitimate local edits still enqueue when logged in.
#[test]
fn open_store_still_enqueues_local_writes_when_logged_in() {
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path();
    let _ = open_store_local(cas_root).unwrap();

    let mut cfg = CloudConfig::default();
    cfg.endpoint = "http://127.0.0.1:9".to_string();
    cfg.token = Some("test-token".to_string());
    cfg.save_to_cas_dir(cas_root).unwrap();

    let queue = SyncQueue::open(cas_root).unwrap();
    queue.init().unwrap();
    let before = queue.pending_count(MAX_RETRIES).unwrap();

    let store = open_store(cas_root).unwrap();
    let entry = Entry {
        id: "local-edit-001".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: "local user edit".to_string(),
        ..Default::default()
    };
    store.add(&entry).unwrap();

    let after = SyncQueue::open(cas_root)
        .unwrap()
        .pending_count(MAX_RETRIES)
        .unwrap();
    assert!(
        after > before,
        "local edit via open_store must enqueue (before={before}, after={after})"
    );
    assert!(
        after >= before + 1,
        "expected at least one new queue row for local edit"
    );
}

/// Contrast: using wrapping open_store for pull *would* re-enqueue (documents
/// the pre-fix failure mode). Guarded so we never ship a silent no-op local
/// opener that actually still wraps.
#[tokio::test]
async fn wrapping_open_store_would_reenqueue_on_pull_apply() {
    let server = MockServer::start().await;
    let project_id = FIXTURE_PROJECT_ID;

    let entry = entry_payload("echo-bait-001", &project_id);
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [entry],
            "tasks": [],
            "rules": [],
            "skills": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = make_logged_in_cas_root(&server.uri());
    let cas_root = tmp.path();
    let cfg = CloudConfig::load_from_cas_dir(cas_root).unwrap();

    let pending_before = SyncQueue::open(cas_root)
        .unwrap()
        .pending_count(MAX_RETRIES)
        .unwrap();

    // Deliberately use the *wrapping* opener — the pre-cas-7fbb bug path.
    let store = open_store(cas_root).unwrap();
    let task_store = open_task_store_local(cas_root).unwrap();
    let rule_store = open_rule_store_local(cas_root).unwrap();
    let skill_store = open_skill_store_local(cas_root).unwrap();
    let spec_store = open_spec_store(cas_root).unwrap();
    let event_store = open_event_store(cas_root).unwrap();
    let prompt_store = open_prompt_store(cas_root).unwrap();
    let file_change_store = open_file_change_store(cas_root).unwrap();
    let commit_link_store = open_commit_link_store(cas_root).unwrap();

    let syncer = CloudSyncer::new(
        Arc::new(SyncQueue::open(cas_root).unwrap()),
        cfg,
        CloudSyncerConfig::default(),
    );
    let result = syncer
        .pull(
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
        .expect("pull should succeed");
    assert_eq!(result.pulled_entries, 1);

    let pending_after = SyncQueue::open(cas_root)
        .unwrap()
        .pending_count(MAX_RETRIES)
        .unwrap();
    assert!(
        pending_after > pending_before,
        "wrapping open_store must still re-enqueue on pull apply (proves Syncing* is live); before={pending_before} after={pending_after}"
    );
}

/// Source guard: production pull apply sites must use *_local openers.
#[test]
fn pull_apply_sites_use_local_openers() {
    let cloud_rs = production_source_root().join("cli/cloud.rs");
    let daemon_rs = production_source_root().join("mcp/daemon.rs");
    let cloud = fs::read_to_string(&cloud_rs).expect("read cloud.rs");
    let daemon = fs::read_to_string(&daemon_rs).expect("read daemon.rs");

    // execute_pull + execute_team_pull must mention local openers.
    assert!(
        cloud.contains("open_store_local"),
        "cli/cloud.rs must call open_store_local for pull apply (cas-7fbb)"
    );
    assert!(
        cloud.contains("open_task_store_local"),
        "cli/cloud.rs must call open_task_store_local for pull apply (cas-7fbb)"
    );
    assert!(
        cloud.contains("open_rule_store_local"),
        "cli/cloud.rs must call open_rule_store_local for pull apply (cas-7fbb)"
    );
    assert!(
        cloud.contains("open_skill_store_local"),
        "cli/cloud.rs must call open_skill_store_local for pull apply (cas-7fbb)"
    );

    // Daemon cloud sync must use local openers and must not claim wrappers
    // while calling wrapping openers.
    assert!(
        daemon.contains("open_store_local"),
        "mcp/daemon.rs run_cloud_sync must use open_store_local (cas-7fbb)"
    );
    assert!(
        daemon.contains("open_task_store_local"),
        "mcp/daemon.rs must use open_task_store_local (cas-7fbb)"
    );
    // Stale comment must not claim "without cloud sync wrappers" while
    // calling open_store (the pre-fix lie). After the fix we either use
    // local openers or an accurate comment — both required.
    let stale = "Open stores without cloud sync wrappers (to avoid recursion)";
    assert!(
        !daemon.contains(stale),
        "stale daemon comment must be removed/rewritten (cas-7fbb AC3)"
    );

    // `open_store_still_enqueues_local_writes_when_logged_in` above is the
    // behavioral guard that SyncingStore remains live for local edits. Do not
    // couple that invariant to a particular CLI push implementation: push is
    // queue-driven and need not open an entry store itself.
}
