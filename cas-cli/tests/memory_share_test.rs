//! Integration tests for the `cas memory share` retroactive backfill path.
//!
//! Exercises `cas::cli::memory::execute_share` and `execute_unshare` —
//! the public-but-doc-hidden CLI helpers wired into the top-level
//! `Memory` subcommand by cas-07d7 (T5). The command's whole point is
//! the side-effect on the team push queue: a unit test that stopped at
//! `entry.share == Some(Team)` would miss the backfill regression the
//! command exists to prevent.
//!
//! Lives in `cas-cli/tests/` rather than co-located with
//! `cas-cli/src/cli/memory.rs` per the cas-1f44 verifier feedback —
//! integration behavior belongs in the integration-test tree.

use std::path::Path;

#[path = "common/mod.rs"]
mod common;
use common::TEST_TEAM;

use cas::cli::memory::{ShareArgs, UnshareArgs, execute_share, execute_unshare};
use cas::cloud::SyncQueue;
use cas::store::open_store;
use cas::types::{Entry, EntryType, Scope, ShareScope};
use tempfile::TempDir;

/// Writes a minimal team-configured CloudConfig to disk so `open_store`
/// wraps the SqliteStore in a SyncingEntryStore with dual-enqueue active.
/// Uses an unconnectable `localhost:0` endpoint on purpose — these tests
/// only verify local state (store + queue) and must not reach out
/// over HTTP.
fn seed_team_cloud_config(cas_dir: &Path) {
    common::make_cloud_config("http://localhost:0")
        .save_to_cas_dir(cas_dir)
        .unwrap();
}

fn seed_entry(cas_dir: &Path, id: &str, entry_type: EntryType, scope: Scope) {
    let store = open_store(cas_dir).unwrap();
    let entry = Entry {
        id: id.to_string(),
        scope,
        entry_type,
        content: format!("seed {id}"),
        ..Default::default()
    };
    store.add(&entry).unwrap();
}

fn drain_queue(cas_dir: &Path) {
    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    queue.clear().unwrap();
}

fn team_queue_len(cas_dir: &Path) -> usize {
    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    queue
        .pending_for_team(TEST_TEAM, 1000, 10)
        .map(|rows| rows.len())
        .unwrap_or(0)
}

#[test]
fn share_by_id_sets_team_share_and_enqueues_team_row() {
    let temp = TempDir::new().unwrap();
    seed_team_cloud_config(temp.path());
    seed_entry(temp.path(), "2026-03-01-1", EntryType::Learning, Scope::Project);
    drain_queue(temp.path());

    let args = ShareArgs {
        id: Some("2026-03-01-1".to_string()),
        since: None,
        all: false,
        dry_run: false,
    };
    execute_share(&args, temp.path()).expect("share by id");

    // Reload through a fresh store handle so the assertion hits disk.
    let store = open_store(temp.path()).unwrap();
    assert_eq!(
        store.get("2026-03-01-1").unwrap().share,
        Some(ShareScope::Team),
        "share by id must persist share=Team to SQLite"
    );
    assert_eq!(
        team_queue_len(temp.path()),
        1,
        "share=Team update must enqueue a team-queue row"
    );
}

#[test]
fn unshare_marks_private_and_blocks_future_enqueue() {
    let temp = TempDir::new().unwrap();
    seed_team_cloud_config(temp.path());
    seed_entry(temp.path(), "2026-03-02-1", EntryType::Learning, Scope::Project);
    drain_queue(temp.path());

    execute_unshare(
        &UnshareArgs {
            id: "2026-03-02-1".to_string(),
        },
        temp.path(),
    )
    .expect("unshare by id");

    let store = open_store(temp.path()).unwrap();
    assert_eq!(
        store.get("2026-03-02-1").unwrap().share,
        Some(ShareScope::Private),
    );

    // Drain the enqueue that unshare itself produced (the personal path
    // enqueues unconditionally for every write).
    drain_queue(temp.path());

    // A subsequent write on a Private entry must stay off the team queue.
    let mut e = store.get("2026-03-02-1").unwrap();
    e.content = "touch".to_string();
    store.update(&e).unwrap();

    assert_eq!(
        team_queue_len(temp.path()),
        0,
        "share=Private must suppress team-queue enqueue on subsequent writes"
    );
}

#[test]
fn share_all_skips_preference_entries() {
    let temp = TempDir::new().unwrap();
    seed_team_cloud_config(temp.path());
    seed_entry(
        temp.path(),
        "2026-03-03-1",
        EntryType::Preference,
        Scope::Project,
    );
    seed_entry(
        temp.path(),
        "2026-03-03-2",
        EntryType::Learning,
        Scope::Project,
    );
    drain_queue(temp.path());

    let args = ShareArgs {
        id: None,
        since: None,
        all: true,
        dry_run: false,
    };
    execute_share(&args, temp.path()).expect("share --all");

    let store = open_store(temp.path()).unwrap();
    // Preference entries stay personal.
    assert_eq!(
        store.get("2026-03-03-1").unwrap().share,
        None,
        "Preference entry must not be promoted by --all"
    );
    // Learning entry gets marked.
    assert_eq!(
        store.get("2026-03-03-2").unwrap().share,
        Some(ShareScope::Team),
    );
    // Only one team row — for the Learning entry.
    assert_eq!(team_queue_len(temp.path()), 1);
}

#[test]
fn share_by_id_refuses_preference_entry() {
    let temp = TempDir::new().unwrap();
    seed_team_cloud_config(temp.path());
    seed_entry(
        temp.path(),
        "2026-03-04-1",
        EntryType::Preference,
        Scope::Project,
    );

    let args = ShareArgs {
        id: Some("2026-03-04-1".to_string()),
        since: None,
        all: false,
        dry_run: false,
    };
    let err = execute_share(&args, temp.path()).expect_err("must refuse Preference");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not eligible"),
        "error must explain ineligibility, got: {msg}"
    );
}

#[test]
fn share_dry_run_does_not_mutate() {
    let temp = TempDir::new().unwrap();
    seed_team_cloud_config(temp.path());
    seed_entry(temp.path(), "2026-03-05-1", EntryType::Learning, Scope::Project);
    drain_queue(temp.path());

    let args = ShareArgs {
        id: None,
        since: None,
        all: true,
        dry_run: true,
    };
    execute_share(&args, temp.path()).expect("dry run");

    let store = open_store(temp.path()).unwrap();
    assert_eq!(
        store.get("2026-03-05-1").unwrap().share,
        None,
        "--dry-run must not mutate share"
    );
    assert_eq!(team_queue_len(temp.path()), 0);
}
