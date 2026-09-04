//! Tests for the per-(team_id, project_canonical_id) watermark scope on team
//! pulls (task cas-53d5, EPIC cas-ffc4).
//!
//! Before this fix, `CloudSyncer::pull_team` (cas-cli/src/cloud/syncer/pull.rs)
//! stored its pull watermark under `last_team_pull_at_{team_id}` only. A user
//! working on team T across two projects (P1 and P2) would:
//!
//!  1. Pull T+P1 (full backfill; watermark set to e.g. 2026-05-13T14:30Z).
//!  2. Switch to P2, run `cas cloud sync` again.
//!  3. The team pull sends `since=2026-05-13T14:30Z` against the P2 scope and
//!     gets back only P2 rows updated AFTER that watermark — the historical
//!     P2 backfill is silently skipped. The user sees "0 of every entity"
//!     even though P2 rows exist on the cloud — the same surface failure
//!     mode cas-6ec7 just fixed for the no-call-at-all case.
//!
//! The fix re-keys the watermark to
//! `last_team_pull_at_{team_id}_{project_canonical_id}`. First pull into a new
//! (team, project) scope sends no `since=`; subsequent pulls into the same
//! scope send the recorded `since`.
//!
//! Test coverage:
//! - Cross-project full backfill (`cross_project_second_pull_sends_no_since`):
//!   two `pull_team` calls under the SAME team but DIFFERENT projects — second
//!   call must send NO `since=` query param (full backfill).
//! - Same-scope incremental (`same_scope_second_pull_sends_recorded_since`):
//!   two `pull_team` calls under same team AND same project — second call
//!   sends `since=<first call's pulled_at>`.
//! - `--full` scope isolation
//!   (`full_flag_clears_only_current_scope_watermark`): seed two scopes' worth
//!   of watermarks, clear one, assert the other survives.
//! - Migration cleanup (`old_global_per_team_key_is_deleted_on_first_new_write`):
//!   confirms `last_team_pull_at_{team_id}` (legacy key) is best-effort
//!   removed when the new-format key lands for the same team.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{TEST_TEAM, make_cloud_config};

use cas::cloud::{CloudSyncer, CloudSyncerConfig, SyncQueue};
use cas::store::{open_rule_store, open_skill_store, open_store, open_task_store};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Initialize the queue + the 4 stores `pull_team` writes into.
fn init_stores(cas_root: &Path) {
    SyncQueue::open(cas_root).unwrap().init().unwrap();
    let _ = open_store(cas_root).unwrap();
    let _ = open_task_store(cas_root).unwrap();
    let _ = open_rule_store(cas_root).unwrap();
    let _ = open_skill_store(cas_root).unwrap();
}

/// Build a CloudSyncer pointed at `server` with a fresh in-tempdir queue.
fn syncer_for(server_uri: String, cas_root: &Path) -> CloudSyncer {
    let queue = SyncQueue::open(cas_root).unwrap();
    queue.init().unwrap();
    CloudSyncer::new(
        Arc::new(queue),
        make_cloud_config(server_uri),
        CloudSyncerConfig {
            timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
}

/// AC core: cross-project full backfill. Two `pull_team` calls — same team,
/// different `project_canonical_id` values. The mock asserts:
/// - First call: NO `since=` (first sync into scope T+P1).
/// - Second call: NO `since=` (first sync into scope T+P2 — even though
///   T+P1 wrote a watermark, T+P2 is a fresh scope and must get a full
///   backfill).
///
/// Before this fix, the second call would send `since=<P1's pulled_at>`
/// because the watermark was keyed by `team_id` alone — exactly the bug
/// from hypothesis #2 in the bug doc.
#[tokio::test]
async fn cross_project_second_pull_sends_no_since() {
    let server = MockServer::start().await;

    // FIRST call: scope T+P1. No `since` expected.
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .and(query_param("project_id", "project-alpha"))
        .and(query_param_is_missing("since"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": "2026-05-13T14:30:00Z",
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // SECOND call: scope T+P2. ALSO no `since` (regression-guard for the
    // pre-fix behavior where the global-per-team watermark leaked across
    // projects).
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .and(query_param("project_id", "project-beta"))
        .and(query_param_is_missing("since"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": "2026-05-13T15:00:00Z",
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
    let cas_root = tmp.path();
    init_stores(cas_root);
    let syncer = syncer_for(server.uri(), cas_root);

    // Sequential calls on the blocking pool — ureq is sync and would block
    // the wiremock tokio runtime otherwise.
    let store1 = open_store(cas_root).unwrap();
    let task_store1 = open_task_store(cas_root).unwrap();
    let rule_store1 = open_rule_store(cas_root).unwrap();
    let skill_store1 = open_skill_store(cas_root).unwrap();
    let server_uri = server.uri();
    let cas_root_owned = cas_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let syncer = syncer_for(server_uri, &cas_root_owned);
        syncer
            .pull_team(
                TEST_TEAM,
                "project-alpha",
                &*store1,
                &*task_store1,
                &*rule_store1,
                &*skill_store1,
            )
            .expect("first pull_team call must succeed");
    })
    .await
    .unwrap();

    let store2 = open_store(cas_root).unwrap();
    let task_store2 = open_task_store(cas_root).unwrap();
    let rule_store2 = open_rule_store(cas_root).unwrap();
    let skill_store2 = open_skill_store(cas_root).unwrap();
    let server_uri = server.uri();
    let cas_root_owned = cas_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let _ = syncer; // ensure first syncer's queue dropped before re-open
        let syncer = syncer_for(server_uri, &cas_root_owned);
        syncer
            .pull_team(
                TEST_TEAM,
                "project-beta",
                &*store2,
                &*task_store2,
                &*rule_store2,
                &*skill_store2,
            )
            .expect("second pull_team (different project) must succeed");
    })
    .await
    .unwrap();

    // Wiremock's `.expect(1)` on each mock asserts both calls hit with the
    // correct `project_id` AND missing `since`. The MockServer's Drop
    // verifies the expectations when it goes out of scope at function end.
}

/// AC: two independently rooted syncers in one process must send the
/// identity pinned by their own queue root. This is the `cas update` refresh
/// shape: the caller iterates projects without changing process cwd between
/// roots, so a process-wide identity cache would send both requests to the
/// first project's bucket.
#[tokio::test]
async fn two_queue_roots_send_their_own_project_scopes() {
    let server = MockServer::start().await;

    for project_id in ["project-alpha", "project-beta"] {
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
            .and(query_param("project_id", project_id))
            .and(query_param_is_missing("since"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [], "tasks": [], "rules": [], "skills": [],
                "pulled_at": "2026-05-13T14:30:00Z",
                "team_id": TEST_TEAM,
                "status": "ok",
            })))
            .expect(1)
            .mount(&server)
            .await;
    }

    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    std::fs::write(
        first.path().join("config.toml"),
        "[project]\ncanonical_id = \"project-alpha\"\n",
    )
    .unwrap();
    std::fs::write(
        second.path().join("config.toml"),
        "[project]\ncanonical_id = \"project-beta\"\n",
    )
    .unwrap();
    init_stores(first.path());
    init_stores(second.path());

    let server_uri = server.uri();
    let first_root = first.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let syncer = syncer_for(server_uri, &first_root);
        let store = open_store(&first_root).unwrap();
        let task_store = open_task_store(&first_root).unwrap();
        let rule_store = open_rule_store(&first_root).unwrap();
        let skill_store = open_skill_store(&first_root).unwrap();
        syncer
            .pull_team(
                TEST_TEAM,
                "project-alpha",
                &*store,
                &*task_store,
                &*rule_store,
                &*skill_store,
            )
            .unwrap();
    })
    .await
    .unwrap();

    let server_uri = server.uri();
    let second_root = second.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let syncer = syncer_for(server_uri, &second_root);
        let store = open_store(&second_root).unwrap();
        let task_store = open_task_store(&second_root).unwrap();
        let rule_store = open_rule_store(&second_root).unwrap();
        let skill_store = open_skill_store(&second_root).unwrap();
        syncer
            .pull_team(
                TEST_TEAM,
                "project-beta",
                &*store,
                &*task_store,
                &*rule_store,
                &*skill_store,
            )
            .unwrap();
    })
    .await
    .unwrap();
}

/// AC: same-scope incremental — second call into the same (team, project)
/// scope must send `since=<first call's pulled_at>`. This is the steady-state
/// behavior; without it the team pull is permanently full-backfilling every
/// sync.
#[tokio::test]
async fn same_scope_second_pull_sends_recorded_since() {
    let server = MockServer::start().await;
    let first_pulled_at = "2026-05-13T14:30:00Z";

    // FIRST call: no `since`. Mock returns a known `pulled_at`.
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .and(query_param("project_id", "project-alpha"))
        .and(query_param_is_missing("since"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": first_pulled_at,
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // SECOND call: same scope. Must send `since=<first_pulled_at>`.
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .and(query_param("project_id", "project-alpha"))
        .and(query_param("since", first_pulled_at))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": "2026-05-13T16:00:00Z",
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
    let cas_root = tmp.path();
    init_stores(cas_root);

    let server_uri = server.uri();
    let cas_root_owned = cas_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let syncer = syncer_for(server_uri, &cas_root_owned);
        let store = open_store(&cas_root_owned).unwrap();
        let task_store = open_task_store(&cas_root_owned).unwrap();
        let rule_store = open_rule_store(&cas_root_owned).unwrap();
        let skill_store = open_skill_store(&cas_root_owned).unwrap();
        syncer
            .pull_team(
                TEST_TEAM,
                "project-alpha",
                &*store,
                &*task_store,
                &*rule_store,
                &*skill_store,
            )
            .expect("first pull_team call must succeed");
    })
    .await
    .unwrap();

    let server_uri = server.uri();
    let cas_root_owned = cas_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let syncer = syncer_for(server_uri, &cas_root_owned);
        let store = open_store(&cas_root_owned).unwrap();
        let task_store = open_task_store(&cas_root_owned).unwrap();
        let rule_store = open_rule_store(&cas_root_owned).unwrap();
        let skill_store = open_skill_store(&cas_root_owned).unwrap();
        syncer
            .pull_team(
                TEST_TEAM,
                "project-alpha",
                &*store,
                &*task_store,
                &*rule_store,
                &*skill_store,
            )
            .expect("second pull_team call (same scope) must succeed");
    })
    .await
    .unwrap();
}

/// AC: `--full` scope isolation. Seed two scope-specific watermarks for the
/// SAME team but DIFFERENT projects. Clear ONE scope's watermark (as `--full`
/// would). Assert the other scope's watermark is untouched.
///
/// This is the queue-level invariant the new `--full` handling depends on —
/// the metadata-clear must be surgical to the current scope and must NOT
/// nuke watermarks for other projects the user has worked on with this team.
#[tokio::test]
async fn full_flag_clears_only_current_scope_watermark() {
    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();

    let key_p1 = format!("last_team_pull_at_{TEST_TEAM}_project-alpha");
    let key_p2 = format!("last_team_pull_at_{TEST_TEAM}_project-beta");
    queue.set_metadata(&key_p1, "2026-05-13T14:30:00Z").unwrap();
    queue.set_metadata(&key_p2, "2026-05-13T15:00:00Z").unwrap();

    // Simulate `--full` for the (T, P1) scope: clear only P1's key.
    queue.delete_metadata(&key_p1).unwrap();

    assert_eq!(
        queue.get_metadata(&key_p1).unwrap(),
        None,
        "P1 watermark must be cleared by --full",
    );
    assert_eq!(
        queue.get_metadata(&key_p2).unwrap().as_deref(),
        Some("2026-05-13T15:00:00Z"),
        "P2 watermark must NOT be cleared by P1's --full (scope isolation)",
    );
}

/// AC migration: the legacy `last_team_pull_at_{team_id}` key (pre-cas-53d5)
/// becomes dead metadata after the re-key. The fix includes a one-shot
/// best-effort cleanup that deletes it when the new-format key is first
/// written for the same team_id.
///
/// This test seeds the legacy key, runs one successful `pull_team` (which
/// writes the new-format key), then asserts the legacy key is gone.
#[tokio::test]
async fn old_global_per_team_key_is_deleted_on_first_new_write() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "pulled_at": "2026-05-13T14:30:00Z",
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_root = tmp.path();
    init_stores(cas_root);

    // Seed the legacy key with a stale watermark.
    let legacy_key = format!("last_team_pull_at_{TEST_TEAM}");
    {
        let q = SyncQueue::open(cas_root).unwrap();
        q.set_metadata(&legacy_key, "2020-01-01T00:00:00Z").unwrap();
        assert_eq!(
            q.get_metadata(&legacy_key).unwrap().as_deref(),
            Some("2020-01-01T00:00:00Z"),
            "precondition: legacy key seeded",
        );
    }

    // Run pull_team; it should write `last_team_pull_at_{team_id}_{project_id}`
    // AND delete the legacy `last_team_pull_at_{team_id}` (best-effort).
    let server_uri = server.uri();
    let cas_root_owned = cas_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let syncer = syncer_for(server_uri, &cas_root_owned);
        let store = open_store(&cas_root_owned).unwrap();
        let task_store = open_task_store(&cas_root_owned).unwrap();
        let rule_store = open_rule_store(&cas_root_owned).unwrap();
        let skill_store = open_skill_store(&cas_root_owned).unwrap();
        syncer
            .pull_team(
                TEST_TEAM,
                "project-alpha",
                &*store,
                &*task_store,
                &*rule_store,
                &*skill_store,
            )
            .expect("pull_team must succeed");
    })
    .await
    .unwrap();

    {
        let q = SyncQueue::open(cas_root).unwrap();
        assert_eq!(
            q.get_metadata(&legacy_key).unwrap(),
            None,
            "legacy `last_team_pull_at_{{team_id}}` key must be cleaned up after first new-format write",
        );

        let new_key = format!("last_team_pull_at_{TEST_TEAM}_project-alpha");
        assert_eq!(
            q.get_metadata(&new_key).unwrap().as_deref(),
            Some("2026-05-13T14:30:00Z"),
            "new-format `last_team_pull_at_{{team_id}}_{{project_id}}` key must hold the watermark",
        );
    }
}
