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
use cas::cloud::{
    CloudConfig, EntityType, SyncOperation, SyncQueue, TeamInfo, canonical_id_from_config_toml,
    get_project_canonical_id, set_canonical_id_in_config_toml,
};
use cas::store::{
    open_commit_link_store, open_event_store, open_file_change_store, open_prompt_store,
    open_rule_store, open_skill_store, open_spec_store, open_store, open_task_store,
};
use flate2::read::GzDecoder;
use std::io::Read;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Process-global lock for CAS_ROOT mutations — same rationale as
/// `team_pull_wiring_test.rs`: `cargo test` runs every `#[tokio::test]` in
/// one process, and the env var is shared.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const FIXTURE_PROJECT_ID: &str = "p";

/// Sets one or more env vars for the duration of a test under a single
/// ENV_LOCK acquisition. One guard for all of them is required, not stylistic:
/// `std::sync::Mutex` is not reentrant, so two guards that each take ENV_LOCK
/// would deadlock the moment a test needs both `CAS_ROOT` and
/// `CAS_USER_CLOUD_JSON`.
struct ScopedEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl ScopedEnv {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut prev = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            prev.push((*key, std::env::var_os(key)));
            // SAFETY: env mutation in an integration-test process, serialized
            // by ENV_LOCK for the guard's whole lifetime.
            unsafe { std::env::set_var(key, value) };
        }
        Self { _lock: lock, prev }
    }

    fn cas_root(cas_root: &Path) -> Self {
        Self::set(&[("CAS_ROOT", &cas_root.to_string_lossy())])
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK held for the entire guard lifetime.
        unsafe {
            for (key, prev) in &self.prev {
                match prev {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn make_cas_root() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    set_canonical_id_in_config_toml(tmp.path(), FIXTURE_PROJECT_ID).unwrap();
    tmp
}

/// Create every SQLite store `execute_sync` opens, so a full-sync test
/// exercises the sync itself rather than store creation.
fn init_all_stores_at(cas_root: &Path) {
    let _ = open_store(cas_root).unwrap();
    let _ = open_task_store(cas_root).unwrap();
    let _ = open_rule_store(cas_root).unwrap();
    let _ = open_skill_store(cas_root).unwrap();
    let _ = open_spec_store(cas_root).unwrap();
    let _ = open_event_store(cas_root).unwrap();
    let _ = open_prompt_store(cas_root).unwrap();
    let _ = open_file_change_store(cas_root).unwrap();
    let _ = open_commit_link_store(cas_root).unwrap();
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

fn decode_gzip_json(body: &[u8]) -> serde_json::Value {
    let mut decoder = GzDecoder::new(body);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .expect("team request must be gzip-compressed JSON");
    serde_json::from_slice(&decoded).expect("team request must decode as JSON")
}

/// A move destination must already belong to the active team. The check is
/// deliberately read-only: unlike `ensure`, it must not register a typo or a
/// project the caller is not authorized to move into.
#[tokio::test]
async fn move_destination_verification_rejects_unregistered_project() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let error = cas::cloud::TeamRegistration::new(
        &endpoint,
        "test-token",
        TEST_TEAM,
        "unregistered-destination",
    )
    .verify_registered()
    .expect_err("an unregistered destination must be refused");
    assert!(error.reason.contains("not registered"), "{error}");
    assert!(error.interaction.contains(&projects_path()), "{error}");
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
    let _env = ScopedEnv::cas_root(&cas_root);

    let expected_id = FIXTURE_PROJECT_ID.to_string();
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
            .get_metadata(&format!(
                "team_project_registered_{TEST_TEAM}_{expected_id}"
            ))
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
    let expected_id = FIXTURE_PROJECT_ID.to_string();

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
    let _env = ScopedEnv::cas_root(&cas_root);

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
            .get_metadata(&format!(
                "team_project_registered_{TEST_TEAM}_{expected_id}"
            ))
            .unwrap()
            .is_some(),
        "a confirmed registration must be cached so steady-state syncs stay cheap"
    );
}

/// A server can resolve the registration push to an existing legacy-slug
/// bucket by its git remote. The client must verify and adopt *that* id, not
/// retry its sent remote-shaped id and call the server broken.
#[tokio::test]
async fn registration_adopts_the_server_resolved_existing_bucket() {
    let server = MockServer::start().await;
    let sent_id = "github.com/richards-llc/gabber-studio";
    let resolved_id = "gabber-studio";

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "canonical_id": resolved_id,
            "synced": { "entries": 0, "tasks": 0, "rules": 0, "skills": 0 },
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(resolved_id)))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let outcome = tokio::task::spawn_blocking(move || {
        cas::cloud::TeamRegistration::new(&endpoint, "test-token", TEST_TEAM, sent_id).ensure()
    })
    .await
    .unwrap()
    .expect("the resolved existing bucket must be treated as a successful registration");

    assert_eq!(outcome.project_uuid(), "project-uuid-1");
    assert!(
        !outcome.newly_registered(),
        "mapping to an existing server bucket must not be reported as creating a new one"
    );
    assert!(
        format!("{outcome:?}").contains("AdoptedExisting"),
        "the outcome must preserve that the server resolved a different canonical id: {outcome:?}"
    );
}

/// A previously registered project may be returned in its git-remote spelling
/// while this checkout has the server's short slug pinned. Registration must
/// recognize the existing row without issuing a duplicate registration push.
#[tokio::test]
async fn alias_registration_project_list_remote_alias_is_owned() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(
            "https://GitHub.com/Richards-LLC/gabber-studio.git/",
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let outcome = tokio::task::spawn_blocking(move || {
        cas::cloud::TeamRegistration::new(&endpoint, "test-token", TEST_TEAM, "gabber-studio")
            .with_pinned_canonical_id(Some("gabber-studio"))
            .ensure()
    })
    .await
    .unwrap()
    .expect("a remote alias in the team project list must count as registered");

    assert_eq!(outcome.project_uuid(), "project-uuid-1");
    assert!(!outcome.newly_registered());
}

/// A divergent server response is an identity-resolution result, not evidence
/// of a server-side defect. If its listed bucket disappears between the push
/// and the verification GET, name the resolved id honestly for recovery.
#[tokio::test]
async fn divergent_registration_failure_names_resolved_id_without_blame() {
    let server = MockServer::start().await;
    let sent_id = "github.com/richards-llc/gabber-studio";
    let resolved_id = "gabber-studio";

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "canonical_id": resolved_id,
            "synced": { "entries": 0, "tasks": 0, "rules": 0, "skills": 0 },
        })))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let failure = tokio::task::spawn_blocking(move || {
        cas::cloud::TeamRegistration::new(&endpoint, "test-token", TEST_TEAM, sent_id)
            .ensure()
            .expect_err("missing resolved bucket must fail registration")
    })
    .await
    .unwrap();
    let message = failure.to_string();

    assert!(
        message.contains(resolved_id),
        "failure must name the resolved id: {message}"
    );
    assert!(
        !message.contains("server-side defect"),
        "a divergent response is not proof of a server-side defect: {message}"
    );
}

/// End-to-end regression coverage for the live outage shape: an explicit
/// remote-form pin, a pre-contract bare-slug bucket, and a queued team row.
/// The same sync run must re-home its cache/config before team push and pull.
#[tokio::test]
async fn sync_adopts_resolved_id_before_its_team_push_and_pull() {
    let server = MockServer::start().await;
    let sent_id = "github.com/richards-llc/gabber-studio";
    let resolved_id = "gabber-studio";

    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(resolved_id)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "canonical_id": resolved_id,
            "synced": { "entries": 1, "tasks": 0, "rules": 0, "skills": 0,
                "sessions": 0, "verifications": 0, "events": 0, "prompts": 0,
                "file_changes": 0, "commit_links": 0, "agents": 0, "worktrees": 0 },
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [], "specs": [],
            "events": [], "prompts": [], "file_changes": [], "commit_links": [],
            "knowledge_pages": [], "pulled_at": "2026-08-18T00:00:00Z",
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": "2026-08-18T00:00:00Z", "team_id": TEST_TEAM, "status": "ok",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    init_all_stores_at(&cas_root);
    set_canonical_id_in_config_toml(&cas_root, sent_id).unwrap();
    let config = make_cloud_config(server.uri());
    config.save_to_cas_dir(&cas_root).unwrap();
    let queue = SyncQueue::open(&cas_root).unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "legacy-id-proof-entry",
            SyncOperation::Upsert,
            Some(r#"{"id":"legacy-id-proof-entry","scope":"project","content":"proof"}"#),
            TEST_TEAM,
        )
        .unwrap();

    let _env = ScopedEnv::set(&[
        ("CAS_ROOT", &cas_root.to_string_lossy()),
        (
            "CAS_USER_CLOUD_JSON",
            "/nonexistent/cas-test-isolation/cloud.json",
        ),
    ]);
    assert_eq!(
        project_id(),
        sent_id,
        "prime the process cache with the sent id"
    );

    let args = CloudSyncArgs {
        dry_run: false,
        rehome: false,
        full: false,
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || execute_sync(&args, &cli, &cas_root_owned))
        .await
        .unwrap()
        .expect("the same sync run must finish against the resolved bucket");

    assert_eq!(
        canonical_id_from_config_toml(&cas_root).as_deref(),
        Some(resolved_id),
        "registration must persist the server-resolved canonical id"
    );
    assert_eq!(
        get_project_canonical_id().as_deref(),
        Some(resolved_id),
        "registration must invalidate the stale process cache before team sync"
    );
    assert!(
        queue
            .get_metadata(&format!(
                "team_project_registered_{TEST_TEAM}_{resolved_id}"
            ))
            .unwrap()
            .is_some(),
        "registration cache must be keyed by the adopted id"
    );

    let requests = server.received_requests().await.unwrap();
    let team_push_ids: Vec<_> = requests
        .iter()
        .filter(|request| {
            request.method.as_str() == "POST" && request.url.path() == team_push_path()
        })
        .map(|request| {
            decode_gzip_json(&request.body)["project_canonical_id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        team_push_ids,
        vec![sent_id, resolved_id],
        "registration sends the original id once; the queued team push in the same run must use the adopted id"
    );
    let team_pull = requests
        .iter()
        .find(|request| {
            request.method.as_str() == "GET"
                && request.url.path() == format!("/api/teams/{TEST_TEAM}/sync/pull")
        })
        .expect("sync must perform the team pull");
    assert_eq!(
        team_pull
            .url
            .query_pairs()
            .find(|(key, _)| key == "project_id")
            .map(|(_, value)| value.into_owned())
            .as_deref(),
        Some(resolved_id),
        "same-run team pull must use the adopted id"
    );
}

/// An already-registered project must not be written again, and the confirmed
/// state must be cached so routine syncs pay no extra round-trip.
#[tokio::test]
async fn already_registered_project_is_verified_once_then_cached() {
    let server = MockServer::start().await;
    let expected_id = FIXTURE_PROJECT_ID.to_string();

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
    let _env = ScopedEnv::cas_root(&cas_root);

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
    let expected_id = FIXTURE_PROJECT_ID.to_string();

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
    let _env = ScopedEnv::cas_root(&cas_root);

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
    let _env = ScopedEnv::cas_root(&cas_root);

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

/// Operator directive (cas-c117 amendment): a logged-in user whose team
/// identity is already resolvable must NOT have to run `cas cloud team set`
/// or `cas cloud team auto on` first. `cas cloud sync` adopts the team for the
/// project and registers it — the whole flow Ben had to drive by hand.
///
/// The project config here has no team at all; only the user-level
/// `cloud.json` knows about the membership, exactly as it does after `cas
/// login` / the `/api/me` refresh.
#[tokio::test]
async fn sync_adopts_the_resolvable_team_without_any_manual_team_command() {
    let server = MockServer::start().await;
    let expected_id = FIXTURE_PROJECT_ID.to_string();

    // Registration: unknown first, listed after the registration write.
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_project_list_body()))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(projects_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_list_body(&expected_id)))
        .expect(1)
        .mount(&server)
        .await;
    // Registration write + the (empty) team drain later in the same sync.
    Mock::given(method("POST"))
        .and(path(team_push_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(team_push_ok_body()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": "2026-08-18T00:00:00Z",
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .mount(&server)
        .await;
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
            "knowledge_pages": [],
            "pulled_at": "2026-08-18T00:00:00Z",
        })))
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    init_all_stores_at(&cas_root);

    // Project config: a fresh clone. NO team of any kind, and — per cas-046d,
    // where `cas login` stores credentials machine-wide — no token of its own
    // either. This is Ben's actual starting state.
    let mut project_cfg = CloudConfig::default();
    project_cfg.endpoint = server.uri();
    project_cfg.save_to_cas_dir(&cas_root).unwrap();
    assert_eq!(project_cfg.team_id, None);
    assert_eq!(project_cfg.team_auto_promote, None);
    assert!(!project_cfg.is_logged_in());

    // User config: the machine-wide login plus the membership the CLI already
    // knows about.
    let user_dir = TempDir::new().unwrap();
    let mut user_cfg = CloudConfig::default();
    user_cfg.endpoint = server.uri();
    user_cfg.token = Some("test-token".to_string());
    user_cfg.teams = vec![TeamInfo {
        id: TEST_TEAM.to_string(),
        slug: "petra-stella".to_string(),
        name: "Petra Stella".to_string(),
        role: "member".to_string(),
    }];
    user_cfg.default_team_id = Some(TEST_TEAM.to_string());
    // Fresh cache + backfill already done, so the sync makes no /api/me call
    // and this test measures adoption alone.
    user_cfg.teams_fetched_at = Some(chrono::Utc::now());
    user_cfg.team_backfill_notified = true;
    user_cfg.save_to_cas_dir(user_dir.path()).unwrap();

    let _env = ScopedEnv::set(&[
        ("CAS_ROOT", &cas_root.to_string_lossy()),
        (
            "CAS_USER_CLOUD_JSON",
            &user_dir.path().join("cloud.json").to_string_lossy(),
        ),
    ]);

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

    assert!(
        result.is_ok(),
        "sync must adopt the resolvable team and register the project without any \
         manual team command; got {result:?}"
    );

    // The project is now team-scoped and stays that way for later commands.
    let saved = CloudConfig::load_from_cas_dir(&cas_root).unwrap();
    assert_eq!(
        saved.team_auto_promote,
        Some(true),
        "sync must persist the adopted team scope"
    );
    assert_eq!(
        saved
            .active_team_id_with_user_config(Some(&user_cfg))
            .as_deref(),
        Some(TEST_TEAM),
        "the adopted scope must resolve to the user's team"
    );

    // And the registration actually happened for that team — the mocks'
    // `.expect(1)` on the lookup pair and the registration write assert it on
    // MockServer drop.
    let queue = SyncQueue::open(&cas_root).unwrap();
    assert!(
        queue
            .get_metadata(&format!(
                "team_project_registered_{TEST_TEAM}_{expected_id}"
            ))
            .unwrap()
            .is_some(),
        "the adopted team must end the sync with a confirmed registration"
    );
}

/// The kill switch outranks adoption: `cas cloud team auto off` keeps a
/// project personal even when a team resolves, and no team endpoint is hit.
#[tokio::test]
async fn team_auto_off_keeps_a_project_personal_despite_a_resolvable_team() {
    let server = MockServer::start().await;

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

    let mut project_cfg = CloudConfig::default();
    project_cfg.endpoint = server.uri();
    project_cfg.token = Some("test-token".to_string());
    project_cfg.team_auto_promote = Some(false); // `cas cloud team auto off`
    project_cfg.save_to_cas_dir(&cas_root).unwrap();

    let user_dir = TempDir::new().unwrap();
    let mut user_cfg = CloudConfig::default();
    user_cfg.teams = vec![TeamInfo {
        id: TEST_TEAM.to_string(),
        slug: "petra-stella".to_string(),
        name: "Petra Stella".to_string(),
        role: "member".to_string(),
    }];
    user_cfg.default_team_id = Some(TEST_TEAM.to_string());
    user_cfg.teams_fetched_at = Some(chrono::Utc::now());
    user_cfg.team_backfill_notified = true;
    user_cfg.save_to_cas_dir(user_dir.path()).unwrap();

    let _env = ScopedEnv::set(&[
        ("CAS_ROOT", &cas_root.to_string_lossy()),
        (
            "CAS_USER_CLOUD_JSON",
            &user_dir.path().join("cloud.json").to_string_lossy(),
        ),
    ]);

    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    let config = CloudConfig::load_from_cas_dir(&cas_root).unwrap();
    tokio::task::spawn_blocking(move || {
        cas::cloud::maybe_adopt_team_scope(&cas_root_owned).expect("adoption must not error");
        ensure_team_project_registration(&config, &cas_root_owned, &cli, false)
            .expect("a personal project must not fail the sync")
    })
    .await
    .unwrap();

    let saved = CloudConfig::load_from_cas_dir(&cas_root).unwrap();
    assert_eq!(
        saved.team_auto_promote,
        Some(false),
        "adoption must never undo the explicit kill switch"
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
    let _env = ScopedEnv::cas_root(&cas_root);

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
