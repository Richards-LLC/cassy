//! Regression tests for the sync-reports-success-but-unregistered defect
//! (cas-c117, EPIC cas-e0d9 — Ben's macOS clean-install field report).
//!
//! # The bug these lock down
//!
//! The server only learned about a project as a side effect of a *non-empty*
//! team push: `CloudSyncer::push_team` returns early when the team queue has
//! no rows (`cloud/syncer/team_push.rs`), so a clean install — nothing written
//! since the team was configured — issued no `POST /api/teams/{id}/sync/push`
//! at all. `cas cloud sync` printed "✓ Push complete / ✓ Pull complete" and
//! exited 0 while the project stayed unknown to the team, after which
//! `cas cloud team-memories` told the user to run the sync they had just run.
//!
//! After the fix, `cas cloud sync` verifies the registration against the
//! server, creates it when missing, and fails the whole command (non-zero)
//! when the server still does not list the project.

use std::path::Path;
use std::sync::Mutex;

mod common;
use common::{TEST_TEAM, make_cli_json, make_cloud_config};

use cas::cli::cloud::{CloudSyncArgs, ensure_team_project_registration, execute_sync};
use cas::cloud::{CloudConfig, SyncQueue, get_project_canonical_id};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Process-global lock for CAS_ROOT mutations — same rationale as
/// `team_pull_wiring_test.rs`: `cargo test` runs every `#[tokio::test]` in
/// one process, and the env var is shared.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct CasRootGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl CasRootGuard {
    fn set(cas_root: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("CAS_ROOT");
        // SAFETY: env mutation in an integration-test process, serialized by
        // ENV_LOCK for the guard's whole lifetime.
        unsafe { std::env::set_var("CAS_ROOT", cas_root) };
        Self { _lock: lock, prev }
    }
}

impl Drop for CasRootGuard {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK held for the entire guard lifetime.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("CAS_ROOT", v),
                None => std::env::remove_var("CAS_ROOT"),
            }
        }
    }
}

fn make_cas_root() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    tmp
}

/// The canonical id the production path resolves — the same
/// `get_project_canonical_id()` used by `ensure_team_project_registration`
/// and by the team push, so mock and product always agree.
fn project_id() -> String {
    get_project_canonical_id().expect("canonical project id must resolve in tests")
}

fn projects_path() -> String {
    format!("/api/teams/{TEST_TEAM}/projects")
}

fn team_push_path() -> String {
    format!("/api/teams/{TEST_TEAM}/sync/push")
}

fn project_list_body(canonical_id: &str) -> serde_json::Value {
    serde_json::json!({
        "projects": [{
            "id": "project-uuid-1",
            "canonical_id": canonical_id,
            "name": "Gabber Studio",
            "contributor_count": 1,
            "memory_count": 0,
        }]
    })
}

fn empty_project_list_body() -> serde_json::Value {
    serde_json::json!({ "projects": [] })
}

fn team_push_ok_body() -> serde_json::Value {
    serde_json::json!({
        "synced": {
            "entries": 0, "tasks": 0, "rules": 0, "skills": 0,
            "sessions": 0, "verifications": 0, "events": 0,
            "prompts": 0, "file_changes": 0, "commit_links": 0,
            "agents": 0, "worktrees": 0,
        }
    })
}

/// THE regression test for the reported defect: the server accepts everything
/// but never lists the project. Before the fix this was a green, exit-0 sync;
/// now `execute_sync` fails with a non-zero exit and names the real reason.
///
/// The `.expect(0)` on the personal push is load-bearing: it proves the
/// failure happens *before* anything can print "✓ Push complete", so a green
/// push line always implies a registered project.
#[tokio::test]
async fn sync_fails_loud_when_server_never_registers_the_project() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        // Once before the registration write, once to verify it.
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(team_push_ok_body()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    make_cloud_config(server.uri())
        .save_to_cas_dir(&cas_root)
        .unwrap();
    let _env = CasRootGuard::set(&cas_root);

    let expected_id = project_id();
    let args = CloudSyncArgs {
        dry_run: false,
        rehome: false,
        full: false,
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let result = tokio::task::spawn_blocking(move || execute_sync(&args, &cli, &cas_root_owned))
        .await
        .unwrap();

    let err = result.expect_err(
        "sync must fail when the project is not registered with the team — a silent \
         exit-0 here is the exact reported bug",
    );
    let msg = format!("{err:#}");
    assert!(
        msg.contains(&expected_id),
        "failure must name the canonical project id, got:\n{msg}"
    );
    assert!(
        msg.contains(TEST_TEAM),
        "failure must name the team, got:\n{msg}"
    );
    assert!(
        msg.contains("not registered"),
        "failure must state the real reason, got:\n{msg}"
    );
    assert!(
        msg.contains(&projects_path()) && msg.contains(&team_push_path()),
        "failure must document the exact failing interaction for escalation, got:\n{msg}"
    );

    // The registration must NOT be cached as successful — the next sync has
    // to try again rather than inherit a false "already registered".
    let queue = SyncQueue::open(&cas_root).unwrap();
    assert_eq!(
        queue
            .get_metadata(&format!("team_project_registered_{TEST_TEAM}_{expected_id}"))
            .unwrap(),
        None,
        "a failed registration must never be recorded as confirmed"
    );
}

/// Positive path for the clean-install case: nothing queued, project unknown
/// to the team → the sync registers it explicitly and the server confirms.
#[tokio::test]
async fn registration_creates_the_project_when_the_team_does_not_know_it() {
    let server = MockServer::start().await;
    let expected_id = project_id();

    // First lookup: unknown. `up_to_n_times(1)` retires this mock so the
    // post-write verification falls through to the registered response.
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(team_push_ok_body()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(&expected_id)))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let config = make_cloud_config(server.uri());
    config.save_to_cas_dir(&cas_root).unwrap();
    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        ensure_team_project_registration(&config, &cas_root_owned, &cli, false)
    })
    .await
    .unwrap();

    assert!(
        result.is_ok(),
        "registration must succeed once the server lists the project; got {result:?}"
    );

    let queue = SyncQueue::open(&cas_root).unwrap();
    assert!(
        queue
            .get_metadata(&format!("team_project_registered_{TEST_TEAM}_{expected_id}"))
            .unwrap()
            .is_some(),
        "a confirmed registration must be cached so steady-state syncs stay cheap"
    );
}

/// An already-registered project must not be written again, and the confirmed
/// state must be cached so routine syncs pay no extra round-trip.
#[tokio::test]
async fn already_registered_project_is_verified_once_then_cached() {
    let server = MockServer::start().await;
    let expected_id = project_id();

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(&expected_id)))
        // Exactly one lookup across BOTH calls: the second is served from the
        // local cache.
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let config = make_cloud_config(server.uri());
    config.save_to_cas_dir(&cas_root).unwrap();
    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let (first, second) = tokio::task::spawn_blocking(move || {
        let first = ensure_team_project_registration(&config, &cas_root_owned, &cli, false);
        let second = ensure_team_project_registration(&config, &cas_root_owned, &cli, false);
        (first, second)
    })
    .await
    .unwrap();

    assert!(first.is_ok(), "first verification must pass: {first:?}");
    assert!(second.is_ok(), "cached verification must pass: {second:?}");
}

/// `cas cloud sync --full` re-verifies rather than trusting the cache — the
/// escape hatch when a project was deleted server-side or re-homed.
#[tokio::test]
async fn full_sync_reverifies_registration_instead_of_trusting_the_cache() {
    let server = MockServer::start().await;
    let expected_id = project_id();

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(&expected_id)))
        .expect(2)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let config = make_cloud_config(server.uri());
    config.save_to_cas_dir(&cas_root).unwrap();
    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || {
        ensure_team_project_registration(&config, &cas_root_owned, &cli, false)
            .expect("first verification");
        ensure_team_project_registration(&config, &cas_root_owned, &cli, true)
            .expect("forced re-verification");
    })
    .await
    .unwrap();
}

/// An expired session must be reported as an expired session — not as a
/// mystery, and not as a green sync.
#[tokio::test]
async fn expired_session_fails_registration_with_a_login_instruction() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(401).set_body_string("{\"error\":\"unauthorized\"}"))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let config = make_cloud_config(server.uri());
    config.save_to_cas_dir(&cas_root).unwrap();
    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        ensure_team_project_registration(&config, &cas_root_owned, &cli, false)
    })
    .await
    .unwrap();

    let msg = format!("{:#}", result.expect_err("401 must fail the sync"));
    assert!(
        msg.contains("cas login"),
        "an expired session must tell the user to log in, got:\n{msg}"
    );
    assert!(
        msg.contains("401"),
        "the failing interaction must carry the status code, got:\n{msg}"
    );
}

/// Personal-only users must not pay for any of this: no team, no traffic.
#[tokio::test]
async fn registration_is_a_no_op_without_a_configured_team() {
    let server = MockServer::start().await;
    // Any request at all fails the test.
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = Some("test-token".to_string());
    config.save_to_cas_dir(&cas_root).unwrap();
    let _env = CasRootGuard::set(&cas_root);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        ensure_team_project_registration(&config, &cas_root_owned, &cli, false)
    })
    .await
    .unwrap();

    assert!(
        result.is_ok(),
        "no team configured must be a silent no-op; got {result:?}"
    );
}
