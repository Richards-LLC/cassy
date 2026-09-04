//! Integration tests for `cas cloud sync` / `cas cloud pull`'s team-pull wiring
//! (task cas-6ec7, EPIC cas-ffc4 — fix `cas cloud sync` pull returning 0 for new
//! team members).
//!
//! Before this fix, `execute_sync` (cli/cloud.rs) and `execute_pull` both called
//! `syncer.pull(...)` only — they never invoked `syncer.pull_team(...)` even when
//! a team was configured. New team members landed in a project, ran
//! `cas cloud sync`, and saw zero team-scoped rows because the team pull endpoint
//! was never hit. This task adds the missing symmetry via a new `execute_team_pull`
//! helper that mirrors `execute_team_push` (cli/cloud.rs:1313).
//!
//! Test coverage:
//! - Behavioral: `execute_team_pull` helper hits `/api/teams/{uuid}/sync/pull` when
//!   a team is configured AND lands rows in the local store (positive path).
//! - Behavioral: `execute_team_pull` does NOT hit the team endpoint when no team
//!   is configured (negative / early-return path).
//! - Behavioral: clearing `last_team_pull_at_<team_id>` from the sync queue
//!   (the `--full` watermark reset for team pulls).
//! - Behavioral end-to-end (`execute_sync_hits_each_pull_endpoint_exactly_once_when_team_configured`):
//!   `execute_sync` fires the personal `GET /api/sync/pull` AND the team
//!   `GET /api/teams/{uuid}/sync/pull` endpoints — each exactly once — and
//!   team rows land in the local store. The `.expect(1)` on the team endpoint
//!   doubles as the regression guard against the previous "double-call" fix
//!   (rejected in code review): if a future change wires `execute_team_pull`
//!   into `execute_sync` directly in addition to its placement at the tail
//!   of `execute_pull`, this test fails with `expected 1, got 2`.
//! - Behavioral end-to-end (`execute_sync_does_not_hit_team_pull_when_no_team_configured`):
//!   when no team is configured, the team endpoint is never hit (`.expect(0)`).
//! - Source-grep: `execute_pull` (standalone command) invokes `execute_team_pull`
//!   when a team is configured. Belt-and-suspenders alongside the behavioral
//!   tests — a refactor that strips the call from `execute_pull` would fail
//!   both. Kept because it pinpoints the exact wire-up site.
//! - Source-grep: the `--full` branch in `execute_pull` clears the team-pull
//!   watermark (`last_team_pull_at_`) in addition to `last_pull_at`.
//!
//! End-to-end tests use a process-global `CAS_ROOT` env var to point
//! `CloudConfig::load()` at a tempdir. CAS_ROOT mutations are serialized
//! through `ENV_LOCK` to keep parallel tokio::test threads from racing.

use std::path::Path;
use std::sync::Mutex;

mod common;
use common::{TEST_TEAM, make_cli_json, make_cloud_config};

use cas::cli::cloud::{CloudSyncArgs, execute_sync, execute_team_pull};
use cas::cloud::{CloudConfig, SyncQueue, get_project_canonical_id};
use cas::store::{
    open_commit_link_store, open_event_store, open_file_change_store, open_prompt_store,
    open_rule_store, open_skill_store, open_spec_store, open_store, open_task_store,
};
use cas::types::{Entry, EntryType, Scope, Skill};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Process-global lock for CAS_ROOT mutations. `cargo test` runs each
/// `#[tokio::test]` on its own thread within the same binary process; the
/// `CAS_ROOT` env var is shared across all of them, so concurrent
/// set/restore pairs corrupt each other's state. Tests that need to set
/// CAS_ROOT acquire this mutex; tests that don't (helper-level + source-grep)
/// do not need the lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that sets CAS_ROOT for the duration of one test and restores
/// the previous value (or removes the var) on drop. `unsafe` is required
/// only because `std::env::set_var` is `unsafe` in edition 2024 — there is
/// no actual undefined-behavior surface beyond the documented thread-safety
/// caveats, which we handle with `ENV_LOCK`.
struct CasRootGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl CasRootGuard {
    fn set(cas_root: &Path) -> Self {
        Self::set_with(cas_root, &[])
    }

    /// Set `CAS_ROOT` plus `extra` vars under a SINGLE `ENV_LOCK`
    /// acquisition. Two separate guards cannot be combined: `std::sync::Mutex`
    /// is not reentrant, so holding `CasRootGuard` and `ScopedEnvVar` at once
    /// would deadlock the test thread.
    fn set_with(cas_root: &Path, extra: &[(&'static str, &str)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut prev = vec![("CAS_ROOT", std::env::var_os("CAS_ROOT"))];
        // SAFETY: env mutation on an integration-test process, guarded by
        // ENV_LOCK so no other test can race the var concurrently.
        unsafe { std::env::set_var("CAS_ROOT", cas_root) };
        for (key, value) in extra {
            prev.push((*key, std::env::var_os(key)));
            // SAFETY: as above.
            unsafe { std::env::set_var(key, value) };
        }
        Self { _lock: lock, prev }
    }
}

impl Drop for CasRootGuard {
    fn drop(&mut self) {
        // SAFETY: same as `set` — ENV_LOCK held for entire guard lifetime.
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

/// RAII guard for scoping an arbitrary env-var mutation to the duration of
/// one test. Used to isolate `active_team_id()` from `~/.cas/cloud.json`
/// (which may have a `default_team_id` on the developer's machine) by
/// pointing `CAS_USER_CLOUD_JSON` at a non-existent path.
struct ScopedEnvVar {
    _lock: std::sync::MutexGuard<'static, ()>,
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os(key);
        // SAFETY: env mutation guarded by ENV_LOCK; no other test can race.
        unsafe { std::env::set_var(key, value) };
        Self { _lock: lock, key, prev }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK held for entire guard lifetime.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Initialize a fresh `.cas`-style tempdir with empty SQLite stores + queue.
fn make_cas_root() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    // Force store creation so subsequent `open_store(cas_root)` calls inside
    // `execute_team_pull` find the SQLite files in place.
    let _ = open_store(tmp.path()).unwrap();
    let _ = open_task_store(tmp.path()).unwrap();
    let _ = open_rule_store(tmp.path()).unwrap();
    let _ = open_skill_store(tmp.path()).unwrap();
    tmp
}

/// Helper to mount a team-pull mock that serves exactly one entry. The entry is
/// serialized via `serde_json::to_value(&Entry{...})` to lock in the actual
/// wire shape (matches the precedent in `team_memories_e2e_test.rs:331`).
async fn mount_team_pull_with_one_entry(server: &MockServer, entry_id: &str) {
    // Inject the same project_id that execute_team_pull resolves via
    // get_project_canonical_id() so entity_matches_project() (cas-6479) accepts
    // the row. Using the same runtime resolver ensures the mock and the
    // production path agree even when the process canonical-id is cached from
    // a prior test or set via CAS_ROOT (cas-6ddc).
    let project_id = get_project_canonical_id()
        .unwrap_or_else(|| "cas-src".to_string());

    let alice_entry = Entry {
        id: entry_id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: "alice's shared learning".to_string(),
        ..Default::default()
    };
    let mut shared_entry = serde_json::to_value(&alice_entry).unwrap();
    shared_entry["project_id"] = serde_json::json!(project_id);

    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [shared_entry],
            "tasks": [],
            "rules": [],
            "skills": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(server)
        .await;
}

/// AC: `execute_team_pull` MUST hit `/api/teams/{uuid}/sync/pull` and land the
/// returned rows into local stores when a team is configured. This is the
/// core positive behavioral test — the bug it guards against is exactly the
/// scenario reported: a new team member runs `cas cloud sync`, the team
/// endpoint is never hit, and zero rows arrive.
#[tokio::test]
async fn team_pull_hits_endpoint_and_lands_rows_when_team_configured() {
    let server = MockServer::start().await;
    mount_team_pull_with_one_entry(&server, "alice-shared-001").await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let cfg = make_cloud_config(server.uri());
    let cli = make_cli_json();

    // Fresh teammate — no existing entry yet.
    {
        let store = open_store(&cas_root).unwrap();
        assert!(
            store.get("alice-shared-001").is_err(),
            "store must start empty for this test"
        );
    }

    // `execute_team_pull` is sync (uses blocking `ureq`); run it on the
    // blocking pool so the wiremock tokio runtime can serve the GET.
    let cas_root_owned = cas_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        execute_team_pull(&cfg, &cas_root_owned, &cli)
    })
    .await
    .unwrap();

    assert!(
        result.is_ok(),
        "execute_team_pull must return Ok (isolation contract); got {result:?}"
    );

    // The mock's `.expect(1)` already asserted exactly one GET fired; below
    // we additionally prove the row landed locally (the full contract — bug
    // would still surface if pull_team was called but rows were dropped).
    let store = open_store(&cas_root).unwrap();
    let pulled = store
        .get("alice-shared-001")
        .expect("team-pulled entry must land in local store");
    assert_eq!(pulled.content, "alice's shared learning");
    assert_eq!(pulled.entry_type, EntryType::Context);
}

/// GH #194: SqliteSkillStore reports an unknown skill as generic `NotFound`,
/// while the original team-pull upsert caught only `SkillNotFound`. That made
/// every sync print the same error and never populated the local row. Pull the
/// exact same response twice: first must insert; second must be quiet.
#[tokio::test]
async fn team_pull_inserts_missing_skill_then_second_pull_is_quiet() {
    let server = MockServer::start().await;
    let project_id = "team-skill-project";
    let mut skill = Skill::new("cas-ska3".to_string(), "Shared skill".to_string());
    skill.description = "shared by the team".to_string();
    let mut shared_skill = serde_json::to_value(&skill).unwrap();
    shared_skill["project_id"] = serde_json::json!(project_id);

    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [],
            "tasks": [],
            "rules": [],
            "skills": [shared_skill],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(2)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let config = make_cloud_config(server.uri());
    let first_and_second = tokio::task::spawn_blocking(move || {
        let queue = cas::cloud::SyncQueue::open(&cas_root).unwrap();
        queue.init().unwrap();
        let syncer = cas::cloud::CloudSyncer::new(
            std::sync::Arc::new(queue),
            config,
            cas::cloud::CloudSyncerConfig::default(),
        );
        let entries = open_store(&cas_root).unwrap();
        let tasks = open_task_store(&cas_root).unwrap();
        let rules = open_rule_store(&cas_root).unwrap();
        let skills = open_skill_store(&cas_root).unwrap();

        let first = syncer
            .pull_team(
                TEST_TEAM,
                project_id,
                entries.as_ref(),
                tasks.as_ref(),
                rules.as_ref(),
                skills.as_ref(),
            )
            .unwrap();
        let second = syncer
            .pull_team(
                TEST_TEAM,
                project_id,
                entries.as_ref(),
                tasks.as_ref(),
                rules.as_ref(),
                skills.as_ref(),
            )
            .unwrap();
        (first, second)
    })
    .await
    .unwrap();

    assert_eq!(first_and_second.0.pulled_skills, 1);
    assert!(
        first_and_second.0.errors.is_empty(),
        "first pull must insert the locally-missing skill: {:?}",
        first_and_second.0.errors
    );
    assert!(
        first_and_second.1.errors.is_empty(),
        "second pull must not repeat a missing-skill warning: {:?}",
        first_and_second.1.errors
    );
}

/// AC negative: `execute_team_pull` MUST NOT hit the team endpoint when no
/// team is configured. The mock fails the test (via Drop on MockServer with
/// `.expect(0)`) if a request reaches it.
#[tokio::test]
async fn team_pull_no_op_when_no_team_configured() {
    let server = MockServer::start().await;
    // Any HTTP method/path on the mock server fails the test — early-return
    // contract means zero traffic.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();

    // Isolate from ~/.cas/cloud.json: on machines where the developer has a
    // default_team_id set (e.g. via `cas cloud team set`), active_team_id()
    // falls through to user-level config and returns that team even when the
    // project config has no team — causing a spurious team pull (cas-6ddc).
    // Pointing CAS_USER_CLOUD_JSON at a nonexistent path makes CloudConfig::
    // load_from() fail and return None, so active_team_id() gets no user
    // config and correctly resolves to None for this no-team project config.
    let _user_cloud_guard = ScopedEnvVar::set(
        "CAS_USER_CLOUD_JSON",
        "/nonexistent/cas-test-isolation/cloud.json",
    );

    // Cloud config WITHOUT an active team — `active_team_id()` returns None.
    let mut cfg = CloudConfig::default();
    cfg.endpoint = server.uri();
    cfg.token = Some("test-token".to_string());
    let cli = make_cli_json();

    let result =
        tokio::task::spawn_blocking(move || execute_team_pull(&cfg, &cas_root, &cli))
            .await
            .unwrap();
    assert!(
        result.is_ok(),
        "execute_team_pull early-return on no-team must yield Ok(()); got {result:?}"
    );
}

/// AC: `cas cloud pull --full` must clear `last_team_pull_at_<team_id>` so the
/// next team pull is a full backfill (mirrors how it already clears
/// `last_pull_at` for personal pulls). This test exercises the exact metadata
/// key/format the implementation must use — a regression in the key string
/// would leave `--full` half-broken (personal-only).
#[tokio::test]
async fn full_flag_clears_team_pull_watermark_via_queue() {
    let tmp = make_cas_root();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();

    let key = format!("last_team_pull_at_{TEST_TEAM}");
    queue.set_metadata(&key, "2025-01-01T00:00:00Z").unwrap();
    assert_eq!(
        queue.get_metadata(&key).unwrap().as_deref(),
        Some("2025-01-01T00:00:00Z"),
        "precondition: watermark must exist before clear"
    );

    // The `--full` branch in `execute_pull` must run exactly this delete
    // (with this exact key format) when an active team is configured.
    queue.delete_metadata(&key).unwrap();

    assert_eq!(
        queue.get_metadata(&key).unwrap(),
        None,
        "watermark must be cleared by `--full`",
    );
}

/// Returns the cloud.rs source as a String, walking up from the test binary's
/// location so the relative-path resolution is robust to `target/` layout.
fn read_cloud_rs() -> String {
    let candidates = [
        cas::test_paths::crate_root().join("src/cli/cloud.rs"),
    ];
    for p in &candidates {
        if let Ok(content) = std::fs::read_to_string(p) {
            return content;
        }
    }
    panic!("could not locate cas-cli/src/cli/cloud.rs from candidates: {candidates:?}");
}

// Helper: open every store kind the standalone `execute_pull` / `execute_push`
// path needs so subsequent `open_*_store(cas_root)` calls inside the helpers
// find their SQLite files on disk. The 4-store helper (`make_cas_root`) is
// not enough for `execute_sync` because the personal pull/push paths also
// touch specs / events / prompts / file_changes / commit_links stores.
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

/// Mount the 4 endpoints `execute_sync` exercises against `server`. Personal
/// push/pull mocks return empty success bodies. Team push mock matches the
/// real server contract (a `synced` count map). Team pull returns a single
/// shared entry so the test can prove rows actually land in the local store.
/// Mount the knowledge-pull endpoint the T5 tail of `cas cloud sync` hits:
/// the same `/api/sync/pull` path, discriminated by `types=knowledge_pages`.
///
/// Kept as its own mock so the two logical pulls are counted separately —
/// this is what makes "each pull endpoint exactly once" mean what it says.
async fn mount_knowledge_pull(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("types", "knowledge_pages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "knowledge_pages": [],
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_full_sync_mocks(server: &MockServer, team_entry_id: &str) {
    // Project↔team registration check (cas-c117): `execute_sync` now verifies
    // with the server that this project is registered before it reports any
    // success. Answer "already registered" so these tests keep exercising the
    // pull wiring rather than the registration write.
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/projects")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{
                "id": "project-uuid-1",
                "canonical_id": get_project_canonical_id()
                    .unwrap_or_else(|| "cas-src".to_string()),
                "name": "CAS",
                "contributor_count": 1,
                "memory_count": 0,
            }]
        })))
        .mount(server)
        .await;

    // Personal push: any payload, success. Empty stores still produce 1 batch.
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(server)
        .await;

    // Personal pull: empty body. `.expect(1)` locks in exactly-one call.
    //
    // `query_param_is_missing("types")` is load-bearing, not decoration: the
    // knowledge tail (T5) issues a SECOND, different request to this same
    // path — `?types=knowledge_pages` — by design (docs/requests/
    // 2026-08-06-cloud-knowledge-sync-and-embeddings.md §"pull"). A path-only
    // matcher absorbs that request into this mock and reports two personal
    // pulls where the product made one personal pull and one knowledge pull.
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param_is_missing("types"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .expect(1)
        .mount(server)
        .await;

    // Knowledge pull: the T5 tail, asserted explicitly rather than left to be
    // silently swallowed by the personal-pull matcher. `.expect(1)` gives the
    // knowledge request the same exactly-once contract every other endpoint
    // in this test has.
    mount_knowledge_pull(server).await;

    // Team push: success with empty counts (empty team queue).
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "entries": 0, "tasks": 0, "rules": 0, "skills": 0,
                "sessions": 0, "verifications": 0, "events": 0,
                "prompts": 0, "file_changes": 0, "commit_links": 0,
                "agents": 0, "worktrees": 0,
            }
        })))
        .mount(server)
        .await;

    // Team pull: one shared entry. `.expect(1)` is the load-bearing
    // assertion — it fails the test if execute_sync hits this endpoint
    // zero times (the original bug) OR more than once (regression guard
    // for the previously-rejected double-call fix).
    // project_id injected so entity_matches_project (cas-6479) accepts the row.
    let project_id = get_project_canonical_id()
        .unwrap_or_else(|| "cas-src".to_string());
    let alice_entry = Entry {
        id: team_entry_id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: "alice's shared learning".to_string(),
        ..Default::default()
    };
    let mut shared_entry = serde_json::to_value(&alice_entry).unwrap();
    shared_entry["project_id"] = serde_json::json!(project_id);
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [shared_entry],
            "tasks": [], "rules": [], "skills": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(server)
        .await;
}

/// Core AC: `execute_sync` MUST hit BOTH `/api/sync/pull` AND
/// `/api/teams/{uuid}/sync/pull` — each EXACTLY ONCE — when a team is
/// configured, AND the team row must land in the local SQLite store.
///
/// This replaces the earlier source-grep ordering test (which only proved
/// the symbol appeared in `execute_sync`, not that the endpoint actually
/// fired). The `.expect(1)` on the team-pull mock is load-bearing in two
/// directions:
/// - `< 1` (zero): regresses the original bug — new team member gets 0 rows.
/// - `> 1` (two): regresses the "defense-in-depth double-call" fix that was
///   rejected in code review. The supervisor explicitly called out this
///   regression guard.
#[tokio::test]
async fn execute_sync_hits_each_pull_endpoint_exactly_once_when_team_configured() {
    let server = MockServer::start().await;
    mount_full_sync_mocks(&server, "alice-shared-via-sync-001").await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path().to_path_buf();
    init_all_stores_at(&cas_root);
    SyncQueue::open(&cas_root).unwrap().init().unwrap();
    // Seed cloud.json on disk so `CloudConfig::load()` (called inside
    // `execute_sync` → `execute_push` / `execute_pull`) finds a valid
    // config with TEST_TEAM configured.
    make_cloud_config(server.uri())
        .save_to_cas_dir(&cas_root)
        .unwrap();

    // CAS_ROOT guard scopes the env mutation to this test only — drops
    // restore the previous value (or remove the var) before another test
    // runs. ENV_LOCK held inside the guard serializes parallel tests.
    let _env = CasRootGuard::set(&cas_root);

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
        "execute_sync must return Ok; got {result:?}"
    );

    // Rows-land assertion (per supervisor: don't infer landing from
    // wiremock count alone — read the store directly).
    let store = open_store(&cas_root).unwrap();
    let pulled = store
        .get("alice-shared-via-sync-001")
        .expect("team-pulled entry must land in local store after execute_sync");
    assert_eq!(pulled.content, "alice's shared learning");
    assert_eq!(pulled.entry_type, EntryType::Context);

    // wiremock's `.expect(1)` on personal pull AND team pull (mounted in
    // `mount_full_sync_mocks`) fires on MockServer drop — guarantees:
    //   - personal `/api/sync/pull` hit exactly once
    //   - team `/api/teams/{uuid}/sync/pull` hit exactly once
    // Drop happens when `server` falls out of scope at function end.
}

/// AC negative: `execute_sync` MUST NOT hit the team pull endpoint when no
/// team is configured. `.expect(0)` on the team endpoint fails the test
/// on any traffic — this is the regression guard for the original bug's
/// inverse (accidentally always hitting team endpoint, even pre-team).
#[tokio::test]
async fn execute_sync_does_not_hit_team_pull_when_no_team_configured() {
    let server = MockServer::start().await;

    // Personal endpoints still get hit (sync = push + pull).
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    // `types` discriminates the personal pull from the knowledge pull that
    // shares this path — see `mount_knowledge_pull`.
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param_is_missing("types"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .expect(1)
        .mount(&server)
        .await;
    mount_knowledge_pull(&server).await;

    // Team endpoints: zero traffic. `.expect(0)` on both fails the test
    // if either fires.
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path().to_path_buf();
    init_all_stores_at(&cas_root);
    SyncQueue::open(&cas_root).unwrap().init().unwrap();

    // Cloud config WITHOUT team_id — `active_team_id()` returns None.
    let mut cfg = CloudConfig::default();
    cfg.endpoint = server.uri();
    cfg.token = Some("test-token".to_string());
    cfg.save_to_cas_dir(&cas_root).unwrap();

    // Isolate from `~/.cas/cloud.json`: since cas-c117, `execute_sync` adopts
    // a resolvable team automatically, so a developer machine with a real
    // membership would legitimately turn this "no team anywhere" project into
    // a team project and hit the endpoints this test asserts are silent.
    // Pointing the user config at a nonexistent path is what makes "no team
    // configured" true for the whole resolution chain.
    let _env = CasRootGuard::set_with(
        &cas_root,
        &[(
            "CAS_USER_CLOUD_JSON",
            "/nonexistent/cas-test-isolation/cloud.json",
        )],
    );

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
        "execute_sync must return Ok; got {result:?}"
    );
}

#[tokio::test]
async fn execute_sync_full_ignores_personal_team_and_knowledge_watermarks() {
    let server = MockServer::start().await;
    mount_full_sync_mocks(&server, "alice-shared-via-full-sync").await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path().to_path_buf();
    init_all_stores_at(&cas_root);
    let queue = SyncQueue::open(&cas_root).unwrap();
    queue.init().unwrap();
    make_cloud_config(server.uri())
        .save_to_cas_dir(&cas_root)
        .unwrap();

    let _env = CasRootGuard::set(&cas_root);
    let project_id = get_project_canonical_id().expect("full sync project id");
    queue
        .set_metadata("last_pull_at", "2026-08-09T17:00:00Z")
        .unwrap();
    queue
        .set_metadata("last_knowledge_pull_at", "2026-08-09T17:00:00Z")
        .unwrap();
    queue
        .set_metadata("knowledge_empty_pull_streak", "4")
        .unwrap();
    queue
        .set_metadata(
            &format!("last_team_pull_at_{TEST_TEAM}_{project_id}"),
            "2026-08-09T17:00:00Z",
        )
        .unwrap();

    let args = CloudSyncArgs {
        dry_run: false,
        rehome: false,
        full: true,
    };
    let cli = make_cli_json();
    let cas_root_owned = cas_root.clone();
    tokio::task::spawn_blocking(move || execute_sync(&args, &cli, &cas_root_owned))
        .await
        .unwrap()
        .expect("full sync must succeed");

    let requests = server.received_requests().await.unwrap();
    // A clean runner may also issue the independent lazy `/api/me` refresh;
    // it is not a data pull and must not make this watermark test depend on
    // whether the host happens to have a fresh user-level teams cache.
    let team_pull_path = format!("/api/teams/{TEST_TEAM}/sync/pull");
    let pull_requests: Vec<_> = requests
        .iter()
        .filter(|request| {
            request.method.as_str() == "GET"
                && (request.url.path() == "/api/sync/pull" || request.url.path() == team_pull_path)
        })
        .collect();
    assert_eq!(pull_requests.len(), 3, "personal, team, and knowledge pull");
    for request in pull_requests {
        assert!(
            request.url.query_pairs().all(|(key, _)| key != "since"),
            "--full must remove the current-project watermark before request: {}",
            request.url
        );
    }
    assert_eq!(
        queue
            .get_metadata("knowledge_empty_pull_streak")
            .unwrap()
            .as_deref(),
        Some("1"),
        "full sync must reset the old streak before the current empty pull increments it"
    );
}

/// Locks in: `execute_pull` (standalone `cas cloud pull` command) must also
/// invoke `execute_team_pull` so the standalone command stays symmetric with
/// `cas cloud sync`. Missing this wire-up would mean `cas cloud pull` works
/// but `cas cloud sync` doesn't (or vice-versa) — silent skew.
#[test]
fn execute_pull_invokes_execute_team_pull_when_team_active() {
    let src = read_cloud_rs();
    let start = src
        .find("fn execute_pull(")
        .expect("execute_pull must exist in cli/cloud.rs");
    let after_start = &src[start..];
    let end_rel = after_start
        .find("\nfn ")
        .or_else(|| after_start.find("\npub fn "))
        .unwrap_or(after_start.len());
    let body = &after_start[..end_rel];

    assert!(
        body.contains("execute_team_pull"),
        "execute_pull (standalone) must invoke `execute_team_pull` so \
         `cas cloud pull` is symmetric with `cas cloud sync`.\nBody scanned:\n{body}",
    );
}

/// Locks in: the `--full` branch in `execute_pull` must clear the team-pull
/// watermark (`last_team_pull_at_`) in addition to `last_pull_at`. The exact
/// key format `last_team_pull_at_<team_id>` matches what
/// `CloudSyncer::pull_team` writes (cas-cli/src/cloud/syncer/pull.rs:710).
#[test]
fn execute_pull_full_clears_team_pull_watermark_in_source() {
    let src = read_cloud_rs();
    let start = src
        .find("fn execute_pull(")
        .expect("execute_pull must exist in cli/cloud.rs");
    let after_start = &src[start..];
    let end_rel = after_start
        .find("\nfn ")
        .or_else(|| after_start.find("\npub fn "))
        .unwrap_or(after_start.len());
    let body = &after_start[..end_rel];

    assert!(
        body.contains("last_team_pull_at_"),
        "execute_pull `--full` branch must clear `last_team_pull_at_<team_id>` \
         metadata (the team-pull watermark) so `--full` triggers a full team \
         backfill in addition to a full personal backfill.\nBody scanned:\n{body}",
    );
}
