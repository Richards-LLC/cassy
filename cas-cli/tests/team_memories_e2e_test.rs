//! End-to-end tests for the team-memories workflow (EPIC cas-cf44).
//!
//! Exercises the full pipeline that lets a teammate land on a project
//! and see shared memories with zero flags and zero UUID lookups:
//!
//! 1. `cas memory share --all` retroactively promotes pre-existing
//!    personal entries to the team queue (T5 cas-07d7).
//! 2. `cas cloud sync` drains the team queue into the team push
//!    endpoint (T4 cas-1f44).
//! 3. `cas cloud team-memories` pulls those memories into a fresh
//!    teammate's local store (the zero-flag journey).
//!
//! Each test exercises the real SQLite store, real `SyncQueue`, and
//! the real `CloudSyncer::push_team` / `pull_team` code paths. Only
//! the HTTP boundary is mocked via `wiremock`. This matches the
//! pattern established by `team_sync_test.rs` (cas-1f44) and extends
//! it to the retroactive backfill + pull-side cases.
//!
//! Carry-in (from T5 verification note): `share --since <duration>`
//! has parse_duration unit-test coverage but no integration coverage
//! of the time-window filter path. `share_since_filter_...` below
//! closes that gap.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{TEST_TEAM, make_cli_json, make_cloud_config};

use cas::cli::cloud::execute_team_push;
use cas::cli::memory::{ShareArgs, execute_share};
use cas::cloud::{CloudSyncer, CloudSyncerConfig, SyncQueue};
use cas::store::{open_rule_store, open_skill_store, open_store, open_task_store};
use cas::types::{Entry, EntryType, Scope, ShareScope};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn seed_team_cloud_config_on_disk(cas_dir: &Path, endpoint: String) {
    make_cloud_config(endpoint).save_to_cas_dir(cas_dir).unwrap();
}

fn mock_push_endpoint() -> Mock {
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/push")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "synced": {
                    "entries": 0,
                    "tasks": 0, "rules": 0, "skills": 0,
                    "sessions": 0, "verifications": 0, "events": 0,
                    "prompts": 0, "file_changes": 0, "commit_links": 0,
                    "agents": 0, "worktrees": 0,
                }
            })),
        )
        .expect(1..)
}

/// Test 1 (primary E2E): retroactive backfill + team push.
///
/// Simulates Daniel's 392-entry scenario: entries that were written
/// BEFORE a team was configured (so they only hit the personal queue)
/// can be retroactively promoted with `cas memory share --all` and
/// pushed to the team via `execute_team_push`. This is the key
/// regression-catching path — if any of T3 (dual-enqueue), T4 (team
/// push), or T5 (share CLI) breaks, the team queue stays empty and
/// the assertion below fails.
#[tokio::test]
async fn retroactive_share_all_then_team_push_surfaces_preexisting_entries() {
    let server = MockServer::start().await;
    mock_push_endpoint().mount(&server).await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_dir = tmp.path();

    // Stage 1: cold start — NO team configured, no cloud.json.
    // Seed two project-scoped learnings (T1 predicate says eligible
    // once a team is configured) plus one Preference entry (which
    // the T1 predicate MUST exclude, giving us end-to-end evidence
    // of the Preference carve-out for AC#4 "filter policy from T1
    // honored in assertions").
    {
        let store = open_store(cas_dir).unwrap();
        store
            .add(&Entry {
                id: "2026-03-01-1".to_string(),
                scope: Scope::Project,
                entry_type: EntryType::Learning,
                content: "pre-existing personal entry A".to_string(),
                ..Default::default()
            })
            .unwrap();
        store
            .add(&Entry {
                id: "2026-03-01-2".to_string(),
                scope: Scope::Project,
                entry_type: EntryType::Learning,
                content: "pre-existing personal entry B".to_string(),
                ..Default::default()
            })
            .unwrap();
        store
            .add(&Entry {
                id: "2026-03-01-pref".to_string(),
                scope: Scope::Project,
                entry_type: EntryType::Preference,
                content: "dark mode please".to_string(),
                ..Default::default()
            })
            .unwrap();
    }

    // With no team configured, the team queue must be empty — both
    // entries only went to the personal queue.
    {
        let q = SyncQueue::open(cas_dir).unwrap();
        q.init().unwrap();
        assert_eq!(
            q.pending_for_team(TEST_TEAM, 1000, 10).unwrap().len(),
            0,
            "team queue must start empty before team is configured"
        );
    }

    // Stage 2: configure the team on disk (simulates `cas cloud team set`).
    seed_team_cloud_config_on_disk(cas_dir, server.uri());

    // Stage 3: retroactive backfill.
    let share_args = ShareArgs {
        id: None,
        since: None,
        all: true,
        dry_run: false,
    };
    let cas_dir_owned = cas_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        execute_share(&share_args, &cas_dir_owned).expect("share --all")
    })
    .await
    .unwrap();

    // Both Learning entries must now carry share=Team on disk AND
    // be in the team queue. The Preference entry must stay share=None
    // (T1 filter policy — AC#4 end-to-end evidence). The store
    // assertions prove T5's mutation, the queue count proves T3's
    // dual-enqueue fired on exactly the eligible subset.
    {
        let store = open_store(cas_dir).unwrap();
        assert_eq!(
            store.get("2026-03-01-1").unwrap().share,
            Some(ShareScope::Team),
            "share --all must mark eligible learning entries share=Team"
        );
        assert_eq!(
            store.get("2026-03-01-2").unwrap().share,
            Some(ShareScope::Team),
        );
        assert_eq!(
            store.get("2026-03-01-pref").unwrap().share,
            None,
            "T1 Preference carve-out: share --all must NOT promote Preference entries"
        );

        let q = SyncQueue::open(cas_dir).unwrap();
        q.init().unwrap();
        let team_rows = q.pending_for_team(TEST_TEAM, 1000, 10).unwrap();
        assert_eq!(
            team_rows.len(),
            2,
            "exactly 2 Learning entries in team queue (Preference excluded by T1 filter)"
        );
    }

    // Stage 4: team push drains the queue.
    let cfg = make_cloud_config(server.uri());
    let cas_dir_owned = cas_dir.to_path_buf();
    let cli = make_cli_json();
    tokio::task::spawn_blocking(move || {
        execute_team_push(&cfg, &cas_dir_owned, &cli).expect("execute_team_push")
    })
    .await
    .unwrap();

    {
        let q = SyncQueue::open(cas_dir).unwrap();
        q.init().unwrap();
        assert_eq!(
            q.pending_for_team(TEST_TEAM, 1000, 10).unwrap().len(),
            0,
            "team queue must be drained after execute_team_push",
        );
    }
    // wiremock's `.expect(1..)` ensures at least one POST to the
    // team push endpoint fired; the MockServer Drop verifies it.
}

/// Test 2 (carry-in from T5 verification): `--since <duration>`
/// selects only entries within the cutoff window.
///
/// `parse_duration` is unit-tested, but no integration test
/// exercises the store.list() + created-timestamp filter. This
/// seeds entries with distinct created timestamps and verifies
/// only the recent one gets promoted.
#[tokio::test]
async fn share_since_filter_selects_only_recent_entries() {
    const SINCE_WINDOW: &str = "48h";

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_dir = tmp.path();

    // Match the real retroactive-backfill scenario: entries are
    // written BEFORE a team is configured, so the initial add path
    // does NOT dual-enqueue. This keeps the post-share team-queue
    // assertion honest — any rows there came from --since, not from
    // the seeding.

    // Seed three entries: one recent, one ~3 days old, one ~30 days
    // old. All Project/Learning so the T1 predicate is satisfied;
    // the cutoff is the only filter in play.
    let now = chrono::Utc::now();
    let mut recent = Entry {
        id: "recent".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "recent".to_string(),
        ..Default::default()
    };
    recent.created = now - chrono::Duration::hours(12);
    let mut medium = Entry {
        id: "medium".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "medium".to_string(),
        ..Default::default()
    };
    medium.created = now - chrono::Duration::days(3);
    let mut old = Entry {
        id: "old".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "old".to_string(),
        ..Default::default()
    };
    old.created = now - chrono::Duration::days(30);

    {
        let store = open_store(cas_dir).unwrap();
        store.add(&recent).unwrap();
        store.add(&medium).unwrap();
        store.add(&old).unwrap();

        // Pin the invariant this test rests on: store.add() must
        // preserve the caller-supplied `created` timestamp. If a
        // refactor ever overrides this to `Utc::now()`, the --since
        // cutoff check below becomes meaningless (all three entries
        // would be effectively "now" and all would pass the filter).
        assert_eq!(
            store.get("recent").unwrap().created,
            recent.created,
            "store.add must preserve caller-supplied created timestamp"
        );
    }

    // Configure team AFTER seeding so the initial adds go to the
    // personal queue only — the retroactive-backfill scenario.
    seed_team_cloud_config_on_disk(cas_dir, "http://127.0.0.1:0".to_string());

    // `--since 48h` should select only the 12-hours-ago entry.
    let args = ShareArgs {
        id: None,
        since: Some(SINCE_WINDOW.to_string()),
        all: false,
        dry_run: false,
    };
    let cas_dir_owned = cas_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        execute_share(&args, &cas_dir_owned).expect("share --since")
    })
    .await
    .unwrap();

    let store = open_store(cas_dir).unwrap();
    assert_eq!(
        store.get("recent").unwrap().share,
        Some(ShareScope::Team),
        "entry inside --since {SINCE_WINDOW} window must be promoted",
    );
    assert_eq!(
        store.get("medium").unwrap().share,
        None,
        "entry 3d old must be outside a {SINCE_WINDOW} window",
    );
    assert_eq!(
        store.get("old").unwrap().share,
        None,
        "entry 30d old must be outside a {SINCE_WINDOW} window",
    );

    // Team-queue side-effect: exactly one row should have been
    // dual-enqueued — the same one that got share=Team. Catches a
    // regression where the filter is applied to the mutation but
    // not to the queue enqueue step.
    let q = SyncQueue::open(cas_dir).unwrap();
    q.init().unwrap();
    assert_eq!(
        q.pending_for_team(TEST_TEAM, 100, 10).unwrap().len(),
        1,
        "only the filtered entry must be in the team queue",
    );
}

/// Test 3 (fresh-teammate pull): the wire contract that makes the
/// zero-flag journey work. A `GET /api/teams/{uuid}/sync/pull`
/// response containing a team-scoped entry must land in the local
/// SQLite store via `CloudSyncer::pull_team`. This is the receive
/// side of the E2E chain — if it regresses, teammate B sees
/// nothing regardless of how correctly A pushed.
#[tokio::test]
async fn fresh_teammate_pull_applies_team_memories_to_local_store() {
    let server = MockServer::start().await;

    // Build the mock payload by serializing a real `Entry`. This keeps
    // the wire shape in sync with `Entry::serialize` (which uses
    // `#[serde(rename = "type")]` on `entry_type` — a hand-written
    // JSON blob using the Rust field name would silently fall through
    // to the serde default and make entry_type assertions vacuous).
    // Using `EntryType::Context` (non-default) guarantees any
    // deserialization regression flips the assertion.
    let alice_entry = Entry {
        id: "alice-shared-001".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: "alice's shared learning".to_string(),
        ..Default::default()
    };
    let mut shared_entry = serde_json::to_value(&alice_entry).unwrap();
    // The real cloud echoes `project_id` on every pull-response row (cas-6479), so the
    // mock must too — otherwise the hardened `entity_matches_project` correctly skips this
    // Project-scoped entry for lacking a project_id. Mirrors the project the teammate pulls.
    shared_entry["project_id"] = serde_json::json!("fresh-teammate-test-project");

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
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let cas_dir = tmp.path();

    // Fresh teammate — empty stores, team configured.
    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    let entry_store = open_store(cas_dir).unwrap();
    let task_store = open_task_store(cas_dir).unwrap();
    let rule_store = open_rule_store(cas_dir).unwrap();
    let skill_store = open_skill_store(cas_dir).unwrap();

    // No existing entries.
    assert!(entry_store.get("alice-shared-001").is_err());

    let cfg = make_cloud_config(server.uri());
    let syncer_config = CloudSyncerConfig {
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let syncer = CloudSyncer::new(Arc::new(queue), cfg, syncer_config);

    // `pull_team` is sync + blocking ureq; run on the blocking pool
    // so the wiremock tokio runtime can serve the GET. cas-53d5 added
    // the explicit `project_id` parameter — for this fresh-teammate
    // wire-shape test, the value is arbitrary (no cross-scope assertion
    // exists here; that lives in `team_pull_watermark_scope_test.rs`).
    let result = tokio::task::spawn_blocking(move || {
        syncer.pull_team(
            TEST_TEAM,
            "fresh-teammate-test-project",
            &*entry_store,
            &*task_store,
            &*rule_store,
            &*skill_store,
        )
    })
    .await
    .unwrap();

    let sync_result = result.expect("pull_team returned Err");
    assert_eq!(
        sync_result.pulled_entries, 1,
        "exactly one entry must be applied to the fresh teammate's store"
    );
    assert_eq!(sync_result.pulled_tasks, 0);
    assert_eq!(sync_result.pulled_rules, 0);
    assert_eq!(sync_result.pulled_skills, 0);
    assert!(
        sync_result.errors.is_empty(),
        "unexpected pull errors: {:?}",
        sync_result.errors,
    );

    // Fresh teammate's local store now has alice's shared memory,
    // with zero flags, zero UUID lookups — AC demo met.
    let fresh_store = open_store(cas_dir).unwrap();
    let pulled = fresh_store
        .get("alice-shared-001")
        .expect("fresh teammate must see alice's shared memory");
    assert_eq!(pulled.id, "alice-shared-001");
    assert_eq!(pulled.content, "alice's shared learning");
    assert_eq!(pulled.scope, Scope::Project);
    // Non-default entry_type proves the `#[serde(rename = "type")]`
    // decoded correctly — a hand-written JSON using `entry_type` as
    // the key would silently fall through to `EntryType::Learning`.
    assert_eq!(pulled.entry_type, EntryType::Context);
    assert!(
        pulled.raw_content.is_none(),
        "raw_content should be absent from wire payload, not mapped from content",
    );
    // `wiremock`'s `.expect(1)` asserts the GET actually fired.
}
