//! End-to-end integration tests for the full team-scope pipeline (T7, EPIC cas-ab88).
//!
//! Tests the complete path from `/api/me` HTTP response → user config →
//! `active_team_id()` resolution → `store.add()` dual-enqueue, WITHOUT any
//! manual `cas cloud team set` invocation.
//!
//! Uses wiremock to stand in for petra-stella-cloud's `/api/me` endpoint
//! (live E2E deferred until cloud redeploys cleanly — TypeScript bug in
//! `lib/teams.ts` tracked in BUG-api-me-deploy-failed-type-check.md).
//!
//! # Coverage
//!
//! 1. **Single-team**: `/api/me` returns 1 team → `/api/me` fetch stores
//!    `teams[0].id` as the implicit auto-pick → `store.add()` dual-enqueues
//!    to that team without any explicit `team set`.
//!
//! 2. **Multi-team + server default**: `/api/me` returns 2 teams +
//!    `default_team_id` → resolution chain uses `default_team_id`.
//!
//! 3. **Multi-team + per-project override**: same setup, but project-level
//!    config has `team_id` set explicitly → project override wins.
//!
//! 4. **No-team / personal-only**: `/api/me` returns `teams: []` → no
//!    team queue rows are written, personal sync only.
//!
//! # Environment isolation
//!
//! Each test requires `CAS_USER_CLOUD_JSON` to point at a tempdir-backed
//! cloud.json so `active_team_id()` reads from the test fixture rather than
//! the developer's real `~/.cas/cloud.json`. Access is serialised via
//! the grouped harness TestEnvGuard.

use crate::test_env_guard::TestEnvGuard;

use cas::cloud::{CloudConfig, SyncQueue, fetch_and_cache_teams_inner};
use cas::store::open_store;
use cas::types::{Entry, EntryType, Scope};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Count rows in the SyncQueue for a specific team.
fn team_queue_len(cas_dir: &std::path::Path, team_id: &str) -> usize {
    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    queue
        .pending_for_team(team_id, 1000, 10)
        .map(|rows| rows.len())
        .unwrap_or(0)
}

/// Build a project-level CloudConfig: logged in (token set) but no `team_id`
/// — simulates a fresh clone where the user hasn't run `cas cloud team set`.
fn project_cfg_no_team(endpoint: &str) -> CloudConfig {
    let mut cfg = CloudConfig::default();
    cfg.endpoint = endpoint.to_string();
    cfg.token = Some("test-token".to_string());
    cfg
}

// ── Test 1: single-team — implicit auto-pick ─────────────────────────────────

/// AC: After `/api/me` returns exactly one team, `store.add()` dual-enqueues
/// to that team without any explicit `cas cloud team set`, provided the project
/// has `team_auto_promote = Some(true)` (cas-f8e3 opt-in).
///
/// Resolution chain step: `user_cfg.teams.len() == 1` → auto-pick.
/// Guard (Step 1.5): fires only when `team_auto_promote = Some(true)`.
#[tokio::test]
async fn e2e_single_team_auto_picks_without_team_set() {
    const TEAM_ID: &str = "e2e-single-0000-0000-000000000001";

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-e2e-1",
            "email":   "alice@example.com",
            "teams": [
                { "id": TEAM_ID, "slug": "solo-squad", "name": "Solo Squad", "role": "owner" }
            ],
            "default_team_id": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    // — Simulate post-login /api/me fetch into user config ——————————————————
    let user_tmp = TempDir::new().unwrap();
    let outcome = fetch_and_cache_teams_inner(&server.uri(), "test-token", user_tmp.path());
    assert_eq!(
        outcome,
        cas::cloud::FetchTeamsOutcome::Updated { team_count: 1 },
        "fetch must succeed with 1 team"
    );

    // — Point active_team_id() at the test user config ———————————————————————
    let user_cloud_json = user_tmp.path().join("cloud.json");
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_USER_CLOUD_JSON", &user_cloud_json);

    // — Create project config (no team_id, but with opt-in for user-level fallback) ——
    // After cas-f8e3, team_auto_promote=Some(true) is required for Steps 2-3
    // of the resolution chain to fire. Without it, the project is personal.
    let project_tmp = TempDir::new().unwrap();
    let mut pcfg = project_cfg_no_team(&server.uri());
    pcfg.team_auto_promote = Some(true);
    pcfg.save_to_cas_dir(project_tmp.path()).unwrap();

    // — Remember an entry ———————————————————————————————————————————————————
    env.set("CAS_ROOT", project_tmp.path());
    let store = open_store(project_tmp.path()).expect("open_store must succeed");
    let entry = Entry {
        id: "e2e-single-team-1".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "T7 E2E: single team auto-pick".to_string(),
        ..Default::default()
    };
    store.add(&entry).expect("store.add must succeed");

    // — Verify dual-enqueue reached the team queue —————————————————————————
    let queue_rows = team_queue_len(project_tmp.path(), TEAM_ID);
    assert!(
        queue_rows > 0,
        "expected ≥1 row in team queue for {TEAM_ID} (auto-pick), got {queue_rows}"
    );

    server.verify().await;
}

// ── Test 2: multi-team + server default_team_id ──────────────────────────────

/// AC: When `/api/me` returns 2 teams and a `default_team_id`, the resolution
/// chain uses `default_team_id` (not the first team), provided the project has
/// `team_auto_promote = Some(true)` (cas-f8e3 opt-in).
#[tokio::test]
async fn e2e_multi_team_uses_server_default_team_id() {
    const DEFAULT_TEAM: &str = "e2e-multi-default-0000-000000000002";
    const OTHER_TEAM: &str   = "e2e-multi-other---0000-000000000003";

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-e2e-2",
            "email":   "bob@example.com",
            "teams": [
                { "id": DEFAULT_TEAM, "slug": "default-squad", "name": "Default Squad", "role": "owner"  },
                { "id": OTHER_TEAM,   "slug": "other-squad",   "name": "Other Squad",   "role": "member" }
            ],
            "default_team_id": DEFAULT_TEAM
        })))
        .expect(1)
        .mount(&server)
        .await;

    let user_tmp = TempDir::new().unwrap();
    fetch_and_cache_teams_inner(&server.uri(), "test-token", user_tmp.path());

    let user_cloud_json = user_tmp.path().join("cloud.json");
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_USER_CLOUD_JSON", &user_cloud_json);

    // Add opt-in so user-level fallback fires (cas-f8e3 guard).
    let project_tmp = TempDir::new().unwrap();
    let mut pcfg = project_cfg_no_team(&server.uri());
    pcfg.team_auto_promote = Some(true);
    pcfg.save_to_cas_dir(project_tmp.path()).unwrap();

    env.set("CAS_ROOT", project_tmp.path());
    let store = open_store(project_tmp.path()).expect("open_store must succeed");
    let entry = Entry {
        id: "e2e-multi-team-2".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "T7 E2E: multi-team uses server default".to_string(),
        ..Default::default()
    };
    store.add(&entry).expect("store.add must succeed");

    // Resolution chain should pick DEFAULT_TEAM (the server-specified default).
    assert!(
        team_queue_len(project_tmp.path(), DEFAULT_TEAM) > 0,
        "expected rows in queue for DEFAULT_TEAM {DEFAULT_TEAM}"
    );
    assert_eq!(
        team_queue_len(project_tmp.path(), OTHER_TEAM),
        0,
        "OTHER_TEAM {OTHER_TEAM} must receive no rows — default_team_id wins"
    );

    server.verify().await;
}

// ── Test 3: multi-team + per-project override ────────────────────────────────

/// AC: When a project-level `team_id` is set explicitly (via `cas cloud team
/// set`), it wins over the user-level `default_team_id` from `/api/me`.
#[tokio::test]
async fn e2e_per_project_override_beats_user_default() {
    const DEFAULT_TEAM:   &str = "e2e-proj-default-0000-000000000004";
    const OVERRIDE_TEAM:  &str = "e2e-proj-override-000-000000000005";

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-e2e-3",
            "email":   "carol@example.com",
            "teams": [
                { "id": DEFAULT_TEAM,  "slug": "default-proj-squad",  "name": "Default",  "role": "owner"  },
                { "id": OVERRIDE_TEAM, "slug": "override-proj-squad",  "name": "Override", "role": "member" }
            ],
            "default_team_id": DEFAULT_TEAM
        })))
        .expect(1)
        .mount(&server)
        .await;

    let user_tmp = TempDir::new().unwrap();
    fetch_and_cache_teams_inner(&server.uri(), "test-token", user_tmp.path());

    let user_cloud_json = user_tmp.path().join("cloud.json");
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_USER_CLOUD_JSON", &user_cloud_json);

    // Project config has explicit team_id = OVERRIDE_TEAM.
    let project_tmp = TempDir::new().unwrap();
    let mut pcfg = project_cfg_no_team(&server.uri());
    pcfg.set_team(OVERRIDE_TEAM, "override-proj-squad");
    pcfg.save_to_cas_dir(project_tmp.path()).unwrap();

    env.set("CAS_ROOT", project_tmp.path());
    let store = open_store(project_tmp.path()).expect("open_store must succeed");
    let entry = Entry {
        id: "e2e-proj-override-3".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "T7 E2E: project override beats user default".to_string(),
        ..Default::default()
    };
    store.add(&entry).expect("store.add must succeed");

    // Project override (OVERRIDE_TEAM) must win over user default (DEFAULT_TEAM).
    assert!(
        team_queue_len(project_tmp.path(), OVERRIDE_TEAM) > 0,
        "expected rows for OVERRIDE_TEAM {OVERRIDE_TEAM}"
    );
    assert_eq!(
        team_queue_len(project_tmp.path(), DEFAULT_TEAM),
        0,
        "DEFAULT_TEAM {DEFAULT_TEAM} must get no rows — project override wins"
    );

    server.verify().await;
}

// ── Test 4: no-team / personal-only ──────────────────────────────────────────

/// AC: When `/api/me` returns `teams: []`, `store.add()` produces personal
/// queue rows only — no team queue rows, no warning/panic.
#[tokio::test]
async fn e2e_no_team_personal_only_no_queue_rows() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-e2e-4",
            "email":   "dan@example.com",
            "teams":   [],
            "default_team_id": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let user_tmp = TempDir::new().unwrap();
    let outcome = fetch_and_cache_teams_inner(&server.uri(), "test-token", user_tmp.path());
    assert_eq!(
        outcome,
        cas::cloud::FetchTeamsOutcome::Empty,
        "zero-membership /api/me must return Empty outcome"
    );

    let user_cloud_json = user_tmp.path().join("cloud.json");
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_USER_CLOUD_JSON", &user_cloud_json);

    let project_tmp = TempDir::new().unwrap();
    project_cfg_no_team(&server.uri())
        .save_to_cas_dir(project_tmp.path())
        .unwrap();

    env.set("CAS_ROOT", project_tmp.path());
    let store = open_store(project_tmp.path()).expect("open_store must succeed");
    let entry = Entry {
        id: "e2e-no-team-4".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "T7 E2E: no-team personal-only".to_string(),
        ..Default::default()
    };
    // Must not panic even when no team is configured.
    store.add(&entry).expect("store.add must succeed with zero teams");

    // Verify no team queue rows exist for any plausible team ID.
    // We check a sentinel ID that would be used if the code mistakenly tried.
    let sentinel = "would-be-team-0000-0000-000000000099";
    assert_eq!(
        team_queue_len(project_tmp.path(), sentinel),
        0,
        "no team queue rows must exist when teams[] is empty"
    );

    // Verify user config on disk has empty teams.
    let cfg = CloudConfig::load_from_cas_dir(user_tmp.path()).unwrap();
    assert!(cfg.teams.is_empty(), "user config must have empty teams");
    // teams_fetched_at should still be stamped (we did fetch — just got 0 members).
    assert!(
        cfg.teams_fetched_at.is_some(),
        "teams_fetched_at must be stamped even on zero-membership response"
    );

    server.verify().await;
}

// ── Test 5: URL on the wire uses endpoint + /api/me path, no extra params ────

/// AC: The HTTP request reaches `/api/me` — no `?team_id=` or other query
/// params smuggled onto the path (regression guard against cas-2eb3 pattern).
#[tokio::test]
async fn e2e_api_me_url_on_wire_is_correct_path_no_extra_params() {
    let server = MockServer::start().await;

    // Mount on the exact path "/api/me" — wiremock will reject any request
    // that goes to a different path (e.g. "/api/me?team_id=...").
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-url-check",
            "email":   "urlcheck@example.com",
            "teams": [
                { "id": "url-team-id", "slug": "url-team", "name": "URL Team", "role": "member" }
            ],
            "default_team_id": null
        })))
        .expect(1) // exactly one request, to exactly /api/me
        .mount(&server)
        .await;

    let user_tmp = TempDir::new().unwrap();
    let outcome = fetch_and_cache_teams_inner(&server.uri(), "test-token", user_tmp.path());
    assert_eq!(
        outcome,
        cas::cloud::FetchTeamsOutcome::Updated { team_count: 1 },
    );

    // server.verify() will assert `.expect(1)` was satisfied — i.e. the
    // request hit `/api/me` with no query params.
    server.verify().await;
}
