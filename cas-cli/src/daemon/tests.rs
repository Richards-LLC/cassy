use chrono::{Duration, Utc};
use std::sync::Arc;

use crate::daemon::decay::apply_memory_decay;
use crate::daemon::{CodeIndexResult, DaemonConfig, DaemonStatus, WatchEvent, run_once};
use crate::store::Store;
use crate::store::mock::MockStore;
use crate::types::{Entry, EntryType, MemoryTier};

#[test]
fn test_daemon_config_default() {
    let config = DaemonConfig::default();
    assert_eq!(config.interval_minutes, 30);
    assert_eq!(config.model, "haiku");
    assert!(config.auto_prune);
}

#[test]
fn test_daemon_status_default() {
    let status = DaemonStatus::default();
    assert!(!status.running);
    assert!(status.last_run.is_none());
}

#[test]
fn memory_decay_status_round_trips_atomically() {
    let temp = tempfile::tempdir().unwrap();

    super::MemoryDecayStatus::write(temp.path(), 7, 3).unwrap();

    let status = super::MemoryDecayStatus::read(temp.path()).unwrap();
    assert_eq!(status.curated_entries_protected, 7);
    assert_eq!(status.promoted_on_access, 3);
    assert!(status.recorded_at <= Utc::now());
}

fn make_entry(id: &str, entry_type: EntryType, tier: MemoryTier) -> Entry {
    Entry {
        id: id.to_string(),
        content: format!("Content for {id}"),
        entry_type,
        memory_tier: tier,
        created: Utc::now(),
        importance: 0.5,
        stability: 0.5,
        ..Default::default()
    }
}

fn make_store(entries: Vec<Entry>) -> Arc<dyn Store> {
    Arc::new(MockStore::with_entries(entries)) as Arc<dyn Store>
}

#[test]
fn test_observation_without_feedback_moves_to_cold() {
    let mut observation = make_entry("obs-001", EntryType::Observation, MemoryTier::Working);
    observation.helpful_count = 0;
    observation.harmful_count = 0;

    let store = make_store(vec![observation]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 1);

    let updated = store.get("obs-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Cold);
}

#[test]
fn test_observation_with_positive_feedback_stays_working() {
    let mut observation = make_entry("obs-001", EntryType::Observation, MemoryTier::Working);
    observation.helpful_count = 1;
    observation.harmful_count = 0;

    let store = make_store(vec![observation]);
    let _count = apply_memory_decay(&store).unwrap();

    let updated = store.get("obs-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Working);
}

#[test]
fn test_low_importance_moves_to_cold() {
    let mut low_imp = make_entry("low-001", EntryType::Learning, MemoryTier::Working);
    low_imp.importance = 0.2;
    low_imp.helpful_count = 0;
    low_imp.harmful_count = 0;

    let store = make_store(vec![low_imp]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 1);

    let updated = store.get("low-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Cold);
}

#[test]
fn test_low_importance_with_feedback_stays_working() {
    let mut low_imp = make_entry("low-001", EntryType::Learning, MemoryTier::Working);
    low_imp.importance = 0.2;
    low_imp.helpful_count = 1;

    let store = make_store(vec![low_imp]);
    let _count = apply_memory_decay(&store).unwrap();

    let updated = store.get("low-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Working);
}

#[test]
fn test_negative_feedback_moves_to_archive() {
    let mut negative = make_entry("neg-001", EntryType::Learning, MemoryTier::Working);
    negative.helpful_count = 0;
    negative.harmful_count = 2;

    let store = make_store(vec![negative]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 1);

    let updated = store.get("neg-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Archive);
}

#[test]
fn test_negative_feedback_from_cold_to_archive() {
    let mut negative = make_entry("neg-001", EntryType::Learning, MemoryTier::Cold);
    negative.helpful_count = 1;
    negative.harmful_count = 3;

    let store = make_store(vec![negative]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 1);

    let updated = store.get("neg-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Archive);
}

#[test]
fn test_non_expired_in_context_entries_are_skipped_by_decay() {
    let mut pinned = make_entry("pin-001", EntryType::Observation, MemoryTier::InContext);
    pinned.helpful_count = 0;
    pinned.harmful_count = 5;
    pinned.valid_until = Some(Utc::now() + Duration::seconds(1));

    let store = make_store(vec![pinned]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 0);

    let updated = store.get("pin-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::InContext);
}

#[test]
fn test_expired_in_context_entries_demote_to_archive() {
    let mut working = make_entry("expired-working", EntryType::Learning, MemoryTier::Working);
    working.valid_until = Some(Utc::now() - Duration::seconds(1));

    let mut pinned = make_entry("expired-pinned", EntryType::Learning, MemoryTier::InContext);
    pinned.valid_until = Some(Utc::now() - Duration::seconds(1));

    let store = make_store(vec![working, pinned]);
    assert_eq!(apply_memory_decay(&store).unwrap(), 2);
    assert_eq!(
        store.get("expired-working").unwrap().memory_tier,
        MemoryTier::Archive
    );
    assert_eq!(
        store.get("expired-pinned").unwrap().memory_tier,
        MemoryTier::Archive,
        "expiry must override an old in-context pin"
    );
}

#[test]
fn test_low_stability_demotes_to_cold() {
    let mut low_stab = make_entry("stab-001", EntryType::Learning, MemoryTier::Working);
    low_stab.stability = 0.2;
    low_stab.importance = 0.5;

    let store = make_store(vec![low_stab]);
    let count = apply_memory_decay(&store).unwrap();

    assert!(count >= 1);

    let updated = store.get("stab-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Cold);
}

#[test]
fn test_curated_high_importance_survives_stability_decay() {
    let mut curated = make_entry("curated-importance", EntryType::Learning, MemoryTier::Working);
    curated.importance = 0.95;
    curated.stability = 0.2;

    let store = make_store(vec![curated]);

    apply_memory_decay(&store).unwrap();

    assert_eq!(
        store.get("curated-importance").unwrap().memory_tier,
        MemoryTier::Working,
        "curated entries must not fall below working through stability decay"
    );
}

#[test]
fn test_curated_helpful_entry_survives_stability_decay() {
    let mut curated = make_entry("curated-helpful", EntryType::Learning, MemoryTier::Working);
    curated.helpful_count = 1;
    curated.stability = 0.1;

    let store = make_store(vec![curated]);

    apply_memory_decay(&store).unwrap();

    assert_eq!(
        store.get("curated-helpful").unwrap().memory_tier,
        MemoryTier::Working,
        "helpful entries must not fall below working through stability decay"
    );
}

#[test]
fn test_expired_curated_entry_still_archives() {
    let mut curated = make_entry("expired-curated", EntryType::Learning, MemoryTier::Working);
    curated.importance = 0.95;
    curated.valid_until = Some(Utc::now() - Duration::seconds(1));

    let store = make_store(vec![curated]);

    apply_memory_decay(&store).unwrap();

    assert_eq!(
        store.get("expired-curated").unwrap().memory_tier,
        MemoryTier::Archive,
        "expiry must override curated stability protection"
    );
}

#[test]
fn test_negative_curated_entry_still_archives() {
    let mut curated = make_entry("negative-curated", EntryType::Learning, MemoryTier::Working);
    curated.importance = 0.95;
    curated.harmful_count = 1;

    let store = make_store(vec![curated]);

    apply_memory_decay(&store).unwrap();

    assert_eq!(
        store.get("negative-curated").unwrap().memory_tier,
        MemoryTier::Archive,
        "negative feedback must override curated stability protection"
    );
}

#[test]
fn test_very_low_stability_demotes_cold_to_archive() {
    let mut very_low_stab = make_entry("stab-001", EntryType::Learning, MemoryTier::Cold);
    very_low_stab.stability = 0.1;

    let store = make_store(vec![very_low_stab]);
    let count = apply_memory_decay(&store).unwrap();

    assert!(count >= 1);

    let updated = store.get("stab-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Archive);
}

#[test]
fn test_access_promotes_archive_directly_to_working() {
    let mut archived = make_entry("accessed-archive", EntryType::Learning, MemoryTier::Archive);

    archived.promote_tier();

    assert_eq!(archived.memory_tier, MemoryTier::Working);
}

#[test]
fn test_normal_entry_no_immediate_tier_change() {
    let normal = make_entry("norm-001", EntryType::Learning, MemoryTier::Working);

    let store = make_store(vec![normal]);
    let _count = apply_memory_decay(&store).unwrap();

    let updated = store.get("norm-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Working);
}

#[test]
fn test_multiple_entries_tiering() {
    let mut obs = make_entry("obs-001", EntryType::Observation, MemoryTier::Working);
    obs.helpful_count = 0;

    let mut negative = make_entry("neg-001", EntryType::Learning, MemoryTier::Working);
    negative.harmful_count = 1;

    let normal = make_entry("norm-001", EntryType::Learning, MemoryTier::Working);

    let store = make_store(vec![obs, negative, normal]);
    let count = apply_memory_decay(&store).unwrap();

    assert!(count >= 2);

    assert_eq!(store.get("obs-001").unwrap().memory_tier, MemoryTier::Cold);
    assert_eq!(
        store.get("neg-001").unwrap().memory_tier,
        MemoryTier::Archive
    );
    assert_eq!(
        store.get("norm-001").unwrap().memory_tier,
        MemoryTier::Working
    );
}

#[test]
fn test_already_archived_not_double_processed() {
    let mut archived = make_entry("arch-001", EntryType::Learning, MemoryTier::Archive);
    archived.harmful_count = 5;

    let store = make_store(vec![archived]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 0);

    let updated = store.get("arch-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Archive);
}

#[test]
fn test_decay_does_not_promote_archived_entry_from_historical_access() {
    let mut archived = make_entry(
        "historically-accessed-archive",
        EntryType::Learning,
        MemoryTier::Archive,
    );
    archived.last_accessed = Some(Utc::now() - Duration::days(10));

    let store = make_store(vec![archived]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 0, "decay must not rewrite an already archived entry");
    assert_eq!(
        store
            .get("historically-accessed-archive")
            .unwrap()
            .memory_tier,
        MemoryTier::Archive,
        "historical access does not prove access after archival"
    );
}

#[test]
fn test_boundary_importance_value() {
    let mut boundary = make_entry("bound-001", EntryType::Learning, MemoryTier::Working);
    boundary.importance = 0.3;
    boundary.helpful_count = 0;

    let store = make_store(vec![boundary]);
    let _count = apply_memory_decay(&store).unwrap();

    let updated = store.get("bound-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Working);
}

#[test]
fn test_boundary_stability_value() {
    let mut boundary = make_entry("bound-001", EntryType::Learning, MemoryTier::Working);
    boundary.stability = 0.3;

    let store = make_store(vec![boundary]);
    let _count = apply_memory_decay(&store).unwrap();

    let updated = store.get("bound-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Working);
}

#[test]
fn test_observation_in_cold_stays_cold() {
    let mut obs_cold = make_entry("obs-001", EntryType::Observation, MemoryTier::Cold);
    obs_cold.helpful_count = 0;
    obs_cold.stability = 0.5;

    let store = make_store(vec![obs_cold]);
    let _count = apply_memory_decay(&store).unwrap();

    let updated = store.get("obs-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::Cold);
}

#[test]
fn test_code_index_result_default() {
    let result = CodeIndexResult::default();
    assert_eq!(result.files_indexed, 0);
    assert_eq!(result.files_deleted, 0);
    assert_eq!(result.symbols_indexed, 0);
    assert!(result.errors.is_empty());
}

#[test]
fn test_code_index_result_tracks_deletions() {
    let result = CodeIndexResult {
        files_deleted: 5,
        files_indexed: 10,
        ..Default::default()
    };
    assert_eq!(result.files_deleted, 5);
    assert_eq!(result.files_indexed, 10);
}

#[test]
fn test_watch_event_variants() {
    use std::path::PathBuf;

    let modified = WatchEvent::Modified(PathBuf::from("test.rs"));
    let deleted = WatchEvent::Deleted(PathBuf::from("deleted.rs"));
    let error = WatchEvent::Error("test error".to_string());

    match modified {
        WatchEvent::Modified(path) => assert_eq!(path, PathBuf::from("test.rs")),
        _ => panic!("Expected Modified variant"),
    }

    match deleted {
        WatchEvent::Deleted(path) => assert_eq!(path, PathBuf::from("deleted.rs")),
        _ => panic!("Expected Deleted variant"),
    }

    match error {
        WatchEvent::Error(message) => assert_eq!(message, "test error"),
        _ => panic!("Expected Error variant"),
    }
}

#[test]
fn test_maintenance_cycle_runs_pruning_and_checkpoint() {
    use crate::daemon::maintenance::run_once;
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().to_path_buf();

    // Initialize the stores so tables exist
    let _store = crate::store::open_store(&cas_root).unwrap();
    let _event_store = crate::store::open_event_store(&cas_root).unwrap();
    let _agent_store = crate::store::open_agent_store(&cas_root).unwrap();

    let config = DaemonConfig {
        cas_root: cas_root.clone(),
        auto_prune: true,
        process_observations: false,
        consolidate_memories: false,
        apply_decay: false,
        index_bm25: false,
        update_entity_summaries: false,
        agent_purge_age_hours: 0,
        ..DaemonConfig::default()
    };

    let result = run_once(&config).expect("maintenance cycle should succeed");

    // With empty tables, pruning should complete without error and prune 0 rows
    assert_eq!(result.events_pruned, 0);
    assert_eq!(result.lease_history_pruned, 0);
    assert!(
        result.errors.is_empty(),
        "unexpected errors: {:?}",
        result.errors
    );
}

#[test]
fn daemon_index_cycle_repairs_the_legacy_tantivy_root_before_draining_pending_entries() {
    use crate::daemon::indexing::run_indexing_cycle;
    use crate::hybrid_search::{DocType, SearchIndex, SearchOptions};
    use tempfile::TempDir;

    let temp = TempDir::new().expect("tempdir");
    let cas_root = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("create .cas");
    let store = crate::store::open_store(&cas_root).expect("store");
    let entry = Entry::new(
        "daemon-repair-entry".to_string(),
        "daemonrepairquasar legacy root".to_string(),
    );
    store.add(&entry).expect("add entry");
    {
        let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
        legacy.index_entry(&entry).expect("legacy write");
    }
    store
        .mark_indexed(&entry.id)
        .expect("incorrect indexed flag");

    let result = run_indexing_cycle(&DaemonConfig {
        cas_root: cas_root.clone(),
        ..DaemonConfig::default()
    })
    .expect("daemon index cycle");
    assert_eq!(result.indexed, 1);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(!cas_root.join("index/meta.json").exists());

    let canonical = SearchIndex::open(&crate::hybrid_search::tantivy_index_dir(&cas_root))
        .expect("canonical index");
    let hits = canonical
        .search(
            &SearchOptions {
                query: "daemonrepairquasar".to_string(),
                doc_types: vec![DocType::Entry],
                ..Default::default()
            },
            &store.list().expect("entries"),
        )
        .expect("canonical search");
    assert_eq!(
        hits.first().map(|hit| hit.id.as_str()),
        Some(entry.id.as_str())
    );
}

#[test]
fn test_maintenance_archives_old_events_and_recordings() {
    use crate::store::{init_cas_dir, open_event_store, open_recording_store};
    use cas_types::{
        Event, EventEntityType, EventType, Recording, RecordingAgent, RecordingEvent,
        RecordingEventType,
    };
    use std::io::Read;
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let cas_root = init_cas_dir(temp.path()).unwrap();
    let event_store = open_event_store(&cas_root).unwrap();
    let recording_store = open_recording_store(&cas_root).unwrap();
    let old = Utc::now() - Duration::days(31);

    let mut event = Event::new(
        EventType::TaskStarted,
        EventEntityType::Task,
        "cas-62a6",
        "old event",
    );
    event.created_at = old;
    event_store.record(&event).unwrap();

    let mut recording = Recording::new("/tmp/cas-62a6.trace".to_string());
    recording.created_at = old;
    let recording_id = recording.id.clone();
    recording_store.add(&recording).unwrap();
    recording_store
        .add_agent(&RecordingAgent::new(
            recording_id.clone(),
            "old-worker".to_string(),
            "worker".to_string(),
            "/tmp/old-worker.trace".to_string(),
        ))
        .unwrap();
    recording_store
        .add_event(&RecordingEvent::new(
            recording_id.clone(),
            42,
            RecordingEventType::TaskStarted,
        ))
        .unwrap();
    recording_store
        .add_fts_content(&recording_id, "old transcript", "output", 42)
        .unwrap();

    let config = DaemonConfig {
        cas_root: cas_root.clone(),
        auto_prune: true,
        process_observations: false,
        consolidate_memories: false,
        apply_decay: false,
        index_bm25: false,
        update_entity_summaries: false,
        agent_purge_age_hours: 0,
        ..DaemonConfig::default()
    };

    let result = run_once(&config).expect("maintenance cycle should succeed");
    assert_eq!(result.events_pruned, 1);
    assert_eq!(result.recordings_pruned, 1);
    assert!(event_store.list_recent(10).unwrap().is_empty());
    assert!(recording_store.get(&recording_id).is_err());

    let archive_dir = cas_root.join("archive");
    let archive_names: Vec<String> = std::fs::read_dir(archive_dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(archive_names.iter().any(|name| name.starts_with("events-")));
    assert!(archive_names
        .iter()
        .any(|name| name.starts_with("recordings-")));

    let listed =
        crate::store::list_archived_traces(&cas_root, old - Duration::seconds(1), Utc::now())
            .unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|trace| trace.archive_path.exists()));
    let sample =
        crate::store::sample_archived_traces(&cas_root, old - Duration::seconds(1), Utc::now(), 1)
            .unwrap();
    assert_eq!(sample.len(), 1);

    let read_archive = |prefix: &str| {
        let name = archive_names
            .iter()
            .find(|name| name.starts_with(prefix))
            .unwrap();
        let file = std::fs::File::open(cas_root.join("archive").join(name)).unwrap();
        let mut decoded = String::new();
        zstd::stream::read::Decoder::new(file)
            .unwrap()
            .read_to_string(&mut decoded)
            .unwrap();
        serde_json::from_str::<serde_json::Value>(decoded.trim()).unwrap()
    };
    let event_archive = read_archive("events-");
    assert_eq!(event_archive["summary"], "old event");
    let recording_archive = read_archive("recordings-");
    assert_eq!(recording_archive["recording"]["id"], recording_id);
    assert_eq!(recording_archive["agents"].as_array().unwrap().len(), 1);
    assert_eq!(recording_archive["events"].as_array().unwrap().len(), 1);
    assert_eq!(recording_archive["fts"][0]["content"], "old transcript");
}

#[test]
fn test_maintenance_enforces_trace_archive_size_cap() {
    use crate::store::{init_cas_dir, open_event_store};
    use cas_types::{Event, EventEntityType, EventType};
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let cas_root = init_cas_dir(temp.path()).unwrap();
    let event_store = open_event_store(&cas_root).unwrap();
    let mut event = Event::new(
        EventType::TaskStarted,
        EventEntityType::Task,
        "cas-2b42",
        "size-capped event",
    );
    event.created_at = Utc::now() - Duration::days(31);
    event_store.record(&event).unwrap();

    let result = run_once(&DaemonConfig {
        cas_root: cas_root.clone(),
        auto_prune: true,
        archive_max_bytes: 1,
        process_observations: false,
        consolidate_memories: false,
        apply_decay: false,
        index_bm25: false,
        update_entity_summaries: false,
        agent_purge_age_hours: 0,
        ..DaemonConfig::default()
    })
    .expect("maintenance cycle should succeed");

    assert_eq!(result.events_pruned, 1);
    assert_eq!(result.trace_archives_evicted, 1);
    assert_eq!(
        crate::store::trace_archive_stats(&cas_root).unwrap().bytes,
        0
    );
    assert!(event_store.list_recent(10).unwrap().is_empty());
}

// ===== cas-499c: symbol index revival =====

/// `repository` used to be the file's parent-directory name, so `a/src/lib.rs` and
/// `b/src/lib.rs` collapsed onto repository = "src" and fought over `UNIQUE(repository, path)`.
#[test]
fn resolve_repository_uses_git_work_tree_root() {
    use crate::daemon::indexing::resolve_repository;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("my-project");
    let nested = repo.join("crates/cas-code/src/parser");
    std::fs::create_dir_all(&nested).expect("nested dirs");
    std::fs::create_dir_all(repo.join(".git")).expect(".git dir");
    let file = nested.join("mod.rs");
    std::fs::write(&file, "pub fn parse() {}").expect("write file");

    let (root, name) = resolve_repository(&file);
    assert_eq!(root.as_deref(), Some(repo.as_path()));
    assert_eq!(
        name, "my-project",
        "must be the work-tree root, not `parser`"
    );
}

/// A linked worktree / submodule has `.git` as a *file*, so the walk must test existence.
#[test]
fn resolve_repository_handles_git_file_worktrees() {
    use crate::daemon::indexing::resolve_repository;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("worktree-checkout");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::write(repo.join(".git"), "gitdir: /elsewhere/.git/worktrees/x").expect(".git file");
    let file = repo.join("src/lib.rs");
    std::fs::write(&file, "pub fn hello() {}").expect("write file");

    let (root, name) = resolve_repository(&file);
    assert_eq!(root.as_deref(), Some(repo.as_path()));
    assert_eq!(name, "worktree-checkout");
}

/// Outside any git tree there is nothing better than the old behaviour, so keep it.
#[test]
fn resolve_repository_falls_back_to_parent_dir_outside_git() {
    use crate::daemon::indexing::resolve_repository;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let dir = temp.path().join("loose-files");
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("thing.rs");
    std::fs::write(&file, "pub fn thing() {}").expect("write file");

    let (root, name) = resolve_repository(&file);
    assert!(root.is_none());
    assert_eq!(name, "loose-files");
}

/// The whole point of M2: indexing a file must leave behind searchable symbols AND the
/// `<cas_root>/index/code` BM25 index that `code_search` probes before answering.
#[test]
fn index_code_files_populates_symbols_and_code_search_index() {
    use crate::daemon::indexing::index_code_files;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("demo-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect(".git dir");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");

    let file = repo.join("src/lib.rs");
    std::fs::write(
        &file,
        "/// Adds two numbers.\npub fn quicksilver_add(a: i64, b: i64) -> i64 { a + b }\n",
    )
    .expect("write file");

    assert!(
        !crate::hybrid_search::code::code_search_available(&cas_root),
        "precondition: no code index yet"
    );

    let result = index_code_files(&[file], &cas_root).expect("indexing should succeed");
    assert!(
        result.errors.is_empty(),
        "unexpected errors: {:?}",
        result.errors
    );
    assert_eq!(result.files_indexed, 1);
    assert!(result.symbols_indexed > 0, "no symbols parsed");

    let store = crate::store::open_code_store(&cas_root).expect("code store");
    assert!(
        store.count_symbols().expect("count") > 0,
        "code_symbols stayed empty"
    );
    assert!(
        crate::hybrid_search::code::code_search_available(&cas_root),
        "`.cas/index/code` was never written, so code_search would still return the stub"
    );

    // And the index actually answers, rather than merely existing.
    let search = crate::hybrid_search::code::open_code_search(&cas_root).expect("open code search");
    let hits = search
        .search(&cas_search::CodeSearchOptions {
            query: "quicksilver_add".to_string(),
            limit: 5,
            ..Default::default()
        })
        .expect("search");
    assert!(
        !hits.is_empty(),
        "code search returned nothing for an indexed symbol"
    );
}

/// `commit_hash` was hardcoded `None` on every row; the git watermark now lands.
#[test]
fn index_code_files_records_head_commit_hash() {
    use crate::daemon::indexing::index_code_files;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("git-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
    };
    if !git(&["init", "-q"])
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("git unavailable; skipping commit-hash assertion");
        return;
    }
    let _ = git(&["config", "user.email", "t@example.com"]);
    let _ = git(&["config", "user.name", "T"]);

    let file = repo.join("src/lib.rs");
    std::fs::write(&file, "pub fn tracked() {}\n").expect("write file");
    let _ = git(&["add", "-A"]);
    let _ = git(&["commit", "-qm", "seed"]);

    let head = String::from_utf8(git(&["rev-parse", "HEAD"]).expect("rev-parse").stdout)
        .expect("utf8")
        .trim()
        .to_string();
    assert_eq!(head.len(), 40, "expected a full sha, got {head:?}");

    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");
    index_code_files(&[file], &cas_root).expect("indexing should succeed");

    let store = crate::store::open_code_store(&cas_root).expect("code store");
    let files = store.list_files("git-repo", None).expect("list files");
    assert_eq!(files.len(), 1, "expected one indexed file: {files:?}");
    assert_eq!(files[0].commit_hash.as_deref(), Some(head.as_str()));
}

#[test]
fn full_tree_reconciliation_retires_files_deleted_while_daemon_was_stopped() {
    use crate::cloud::embeddings::{EmbeddingMeta, KnowledgeVectorCache, code_symbol_key};
    use crate::daemon::indexing::reconcile_code_tree;
    use cas_search::Bm25Index;
    use cas_store::SqliteCodeVectorStore;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("reconcile-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");
    let old = repo.join("src/old.rs");
    std::fs::write(&old, "pub fn removed_symbol() {}\n").expect("old file");

    reconcile_code_tree(
        std::slice::from_ref(&old),
        std::slice::from_ref(&repo),
        &cas_root,
        false,
    )
    .expect("initial scan");
    let store = crate::store::open_code_store(&cas_root).expect("code store");
    let removed = store
        .search_symbols("%removed_symbol%", None, None, 10)
        .expect("removed symbol before delete")
        .pop()
        .expect("removed symbol was initially indexed");
    let vectors = SqliteCodeVectorStore::open(&cas_root).expect("vector state");
    let cache =
        KnowledgeVectorCache::open_code(&cas_root, EmbeddingMeta::new("test", "test-code", 2))
            .expect("code vector cache");
    cache
        .put(&code_symbol_key(&removed.id), &[1.0, 0.0])
        .expect("seed code vector");
    assert!(
        vectors
            .mark_vectorized(&removed.id, &removed.content_hash)
            .expect("mark vectorized")
    );
    let bm25 =
        Bm25Index::open(&crate::daemon::indexing::code_index_dir(&cas_root)).expect("code bm25");
    assert!(bm25.exists(&removed.id).expect("bm25 contains removed"));

    std::fs::remove_file(&old).expect("delete while stopped");
    let new = repo.join("src/new.rs");
    std::fs::write(&new, "pub fn surviving_symbol() {}\n").expect("new file");
    let result = reconcile_code_tree(
        std::slice::from_ref(&new),
        std::slice::from_ref(&repo),
        &cas_root,
        false,
    )
    .expect("restart reconciliation");

    assert_eq!(result.files_deleted, 1);
    let files = store.list_files("reconcile-repo", None).expect("files");
    assert_eq!(files.len(), 1);
    assert!(files[0].path.ends_with("src/new.rs"));
    assert!(
        store
            .search_symbols("%removed_symbol%", None, None, 10)
            .expect("symbols")
            .is_empty(),
        "deleted symbol survived SQLite reconciliation"
    );
    let reopened_bm25 = Bm25Index::open(&crate::daemon::indexing::code_index_dir(&cas_root))
        .expect("reopen code bm25 after retirement");
    assert!(
        !reopened_bm25.exists(&removed.id).expect("bm25 retirement"),
        "deleted symbol survived BM25 reconciliation"
    );
    assert_eq!(
        cache
            .get(&code_symbol_key(&removed.id))
            .expect("cached vector retirement"),
        None,
        "deleted symbol survived vector-cache reconciliation"
    );
    assert_eq!(vectors.stats().expect("vector stats").eligible, 1);
    let scan = vectors
        .index_state("reconcile-repo")
        .expect("scan receipt")
        .expect("recorded scan receipt");
    assert_eq!(scan.eligible_files, 1);
    assert_eq!(scan.indexed_files, 1);
    assert_eq!(scan.failed_files, 0);
    assert_eq!(scan.last_error, None);
}

#[test]
fn full_tree_reconciliation_retires_repository_when_eligible_set_becomes_empty() {
    use crate::cloud::embeddings::{EmbeddingMeta, KnowledgeVectorCache, code_symbol_key};
    use crate::daemon::indexing::{reconcile_code_tree, run_code_index_cycle};
    use crate::daemon::{CodeWatcher, WatcherConfig};
    use cas_search::Bm25Index;
    use cas_store::SqliteCodeVectorStore;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("empty-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect("git marker");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");
    let old = repo.join("src/old.rs");
    std::fs::write(&old, "pub fn retired_with_last_file() {}\n").expect("old file");

    reconcile_code_tree(
        std::slice::from_ref(&old),
        std::slice::from_ref(&repo),
        &cas_root,
        false,
    )
    .expect("initial scan");
    let store = crate::store::open_code_store(&cas_root).expect("code store");
    let removed = store
        .search_symbols("%retired_with_last_file%", None, None, 10)
        .expect("removed symbol before delete")
        .pop()
        .expect("removed symbol was initially indexed");
    let vectors = SqliteCodeVectorStore::open(&cas_root).expect("vector state");
    let cache =
        KnowledgeVectorCache::open_code(&cas_root, EmbeddingMeta::new("test", "test-code", 2))
            .expect("code vector cache");
    cache
        .put(&code_symbol_key(&removed.id), &[1.0, 0.0])
        .expect("seed code vector");
    assert!(
        vectors
            .mark_vectorized(&removed.id, &removed.content_hash)
            .expect("mark vectorized")
    );

    std::fs::remove_file(&old).expect("delete last eligible file");
    let watcher = CodeWatcher::new(WatcherConfig {
        watch_paths: vec![repo.clone()],
        extensions: vec!["rs".to_string()],
        debounce_ms: 20,
        ignore_patterns: Vec::new(),
    });
    watcher.seed_initial(Vec::new());
    let result = run_code_index_cycle(&watcher, &cas_root).expect("empty startup reconciliation");

    assert_eq!(result.files_deleted, 1);
    assert!(
        store
            .list_files("empty-repo", None)
            .expect("files")
            .is_empty()
    );
    assert!(
        store
            .search_symbols("%retired_with_last_file%", None, None, 10)
            .expect("symbols")
            .is_empty(),
        "last symbol survived SQLite reconciliation"
    );
    let bm25 = Bm25Index::open(&crate::daemon::indexing::code_index_dir(&cas_root))
        .expect("reopen code bm25 after retirement");
    assert!(!bm25.exists(&removed.id).expect("bm25 retirement"));
    assert_eq!(
        cache
            .get(&code_symbol_key(&removed.id))
            .expect("cached vector retirement"),
        None
    );
    assert_eq!(vectors.stats().expect("vector stats").eligible, 0);
    let scan = vectors
        .index_state("empty-repo")
        .expect("scan receipt")
        .expect("recorded scan receipt");
    assert_eq!(
        (scan.eligible_files, scan.indexed_files, scan.failed_files),
        (0, 0, 0)
    );
    assert_eq!(scan.last_error, None);
}

/// cas-25a9 P1-B: a legacy-repair failure must not disable background indexing.
///
/// Before the fix, `generate_bm25_index` returned as soon as
/// `repair_legacy_index` errored, so a legacy root that could never be retired
/// (malformed `.managed.json`, EPERM, a lock held by a pre-fix daemon)
/// permanently stopped ALL background memory indexing — and `doctor --fix` hit
/// the same error, leaving no self-heal short of hand-deleting
/// `.cas/index/meta.json`. Repair is best-effort per cycle; the cycle continues.
#[test]
fn a_failing_legacy_repair_still_indexes_pending_entries() {
    use crate::hybrid_search::SearchIndex;

    let temp = tempfile::tempdir().expect("tempdir");
    let cas_root = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("create .cas");
    let store = crate::store::open_store(&cas_root).expect("store");

    // One entry stranded in the legacy root, one ordinary pending entry that
    // has nothing to do with the legacy problem.
    let stranded = Entry::new(
        "legacy-stranded".to_string(),
        "legacyquasar stranded in the old root".to_string(),
    );
    store.add(&stranded).expect("add stranded");
    {
        let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
        legacy.index_entry(&stranded).expect("index legacy");
    }
    store.mark_indexed(&stranded.id).expect("mark indexed");

    let healthy = Entry::new(
        "ordinary-pending".to_string(),
        "ordinarypending entry awaiting the canonical index".to_string(),
    );
    store.add(&healthy).expect("add healthy");

    // Poison retirement: `.managed.json` no longer parses, so repair fails
    // every single cycle, forever.
    std::fs::write(cas_root.join("index/.managed.json"), b"{ not json ]")
        .expect("poison managed list");

    let config = DaemonConfig {
        cas_root: cas_root.clone(),
        index_bm25: true,
        ..DaemonConfig::default()
    };
    let store: Arc<dyn Store> = store;
    let result =
        crate::daemon::indexing::generate_bm25_index(&store, &config).expect("cycle must not fail");

    assert!(
        result
            .errors
            .iter()
            .any(|(id, _)| id == "legacy-index-repair"),
        "the repair failure must be reported, not swallowed: {:?}",
        result.errors
    );
    assert!(
        result.indexed >= 1,
        "the cycle must still index pending entries despite the repair failure; \
         indexed {} errors {:?}",
        result.indexed,
        result.errors
    );
    assert!(
        store
            .list_pending_index(10)
            .expect("pending")
            .iter()
            .all(|entry| entry.id != healthy.id),
        "the unrelated pending entry must have reached the canonical index"
    );
}

/// cas-25a9 AC1 at the DAEMON call site: a held legacy lock must not wedge the
/// maintenance cycle.
///
/// This is the site that matters most — `run_maintenance` awaits the indexing
/// cycle inside the daemon's `select!`, so a block here stalls agent reaping,
/// lease reclaim and worktree cleanup for every session on the box. The
/// companion `doctor_fix_against_a_held_legacy_lock_warns_within_a_bounded_time`
/// covers the other call site.
#[test]
fn a_held_legacy_lock_does_not_wedge_the_daemon_cycle() {
    use crate::hybrid_search::SearchIndex;
    use std::time::{Duration, Instant};
    use tantivy::directory::Directory;

    let temp = tempfile::tempdir().expect("tempdir");
    let cas_root = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("create .cas");
    let store = crate::store::open_store(&cas_root).expect("store");

    let stranded = Entry::new(
        "daemon-locked-stranded".to_string(),
        "daemonlocked stranded in the legacy root".to_string(),
    );
    store.add(&stranded).expect("add stranded");
    {
        let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
        legacy.index_entry(&stranded).expect("index legacy");
    }
    store.mark_indexed(&stranded.id).expect("mark indexed");

    // An unrelated entry that the cycle must still index while the legacy root
    // is unavailable.
    let healthy = Entry::new(
        "daemon-locked-healthy".to_string(),
        "daemonhealthy entry awaiting the canonical index".to_string(),
    );
    store.add(&healthy).expect("add healthy");

    // A pre-fix `cas serve` is holding the legacy root.
    let holder = tantivy::Index::open_in_dir(cas_root.join("index")).expect("open legacy root");
    let held = holder
        .directory()
        .acquire_lock(&tantivy::directory::META_LOCK)
        .expect("hold the meta lock");

    let config = DaemonConfig {
        cas_root: cas_root.clone(),
        index_bm25: true,
        ..DaemonConfig::default()
    };
    let store_arc: Arc<dyn Store> = store.clone();

    let started = Instant::now();
    let result = crate::daemon::indexing::generate_bm25_index(&store_arc, &config)
        .expect("the cycle must not fail");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "a held legacy lock must not wedge the maintenance cycle; took {elapsed:?}"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|(id, message)| id == "legacy-index-repair" && message.contains("skipped")),
        "the skipped repair must be recorded: {:?}",
        result.errors
    );
    assert!(
        result.indexed >= 1,
        "the cycle must keep indexing while the legacy root is busy; indexed {} errors {:?}",
        result.indexed,
        result.errors
    );
    assert!(
        store
            .list_pending_index(10)
            .expect("pending")
            .iter()
            .all(|entry| entry.id != healthy.id),
        "the unrelated entry must have reached the canonical index"
    );

    // Release before anything opens a tantivy reader: `IndexReader` acquires
    // META_LOCK itself, so touching the root while still holding it would
    // deadlock this test against itself.
    drop(held);
}

// ---------------------------------------------------------------------------
// cas-bd9df (GH #698): BOM-marked source files.
//
// The reporter's repo carried UTF-16 LE files with CRLF. `read_to_string`
// failed on them, they were counted as index failures, and `cas index code` —
// the remediation doctor itself printed — re-read the same bytes and failed
// identically, so the warning could never clear.
// ---------------------------------------------------------------------------

fn utf16_le_bytes(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[test]
fn a_utf16_source_file_indexes_instead_of_failing_forever() {
    use crate::daemon::indexing::index_code_files;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("bom-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect(".git dir");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");

    // The reporter's exact shape: UTF-16 LE with CRLF line endings.
    let file = repo.join("src/admin.ts");
    std::fs::write(
        &file,
        utf16_le_bytes(
            "export function quicksilverAdmin(a: number): number {\r\n  return a + 1;\r\n}\r\n",
        ),
    )
    .expect("write utf-16 file");

    let result = index_code_files(&[file], &cas_root).expect("indexing should succeed");
    assert!(
        result.errors.is_empty(),
        "a UTF-16 file must not be an error: {:?}",
        result.errors
    );
    assert!(
        result.skipped.is_empty(),
        "a decodable UTF-16 file must be indexed, not skipped: {:?}",
        result.skipped
    );
    assert_eq!(result.files_indexed, 1);
    assert!(
        result.symbols_indexed > 0,
        "the decoded UTF-16 source must yield real symbols"
    );
}

#[test]
fn a_utf8_bom_file_indexes_without_the_bom_reaching_the_parser() {
    use crate::daemon::indexing::index_code_files;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("bom8-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect(".git dir");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");

    // The quiet half of GH #698: read_to_string ACCEPTED this (a BOM is valid
    // UTF-8), so the BOM reached the parser glued to the first token.
    let file = repo.join("src/lib.rs");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"pub fn quicksilver_bom(a: i64) -> i64 { a }\n");
    std::fs::write(&file, bytes).expect("write utf-8-bom file");

    let result = index_code_files(&[file], &cas_root).expect("indexing should succeed");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.files_indexed, 1);

    let store = crate::store::open_code_store(&cas_root).expect("code store");
    let symbols = store
        .search_symbols("quicksilver_bom", None, None, 10)
        .unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s.name == "quicksilver_bom"),
        "the first symbol must be findable by its real name, not with a BOM \
         attached: {:?}",
        symbols.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn a_binary_file_is_skipped_with_a_reason_and_is_not_an_error() {
    use crate::daemon::indexing::index_code_files;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("bin-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect(".git dir");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");

    // A .rs path holding PNG bytes: eligible by extension, undecodable in fact.
    let file = repo.join("src/not-really.rs");
    std::fs::write(&file, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0xFF]).expect("write");

    let result = index_code_files(&[file.clone()], &cas_root).expect("indexing should succeed");
    assert!(
        result.errors.is_empty(),
        "an undecodable file is a skip, not a failure — counting it as a failure \
         is what made the warning permanent: {:?}",
        result.errors
    );
    assert_eq!(result.files_indexed, 0);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].0, file);
    assert!(
        result.skipped[0].1.contains("not valid UTF-8"),
        "the skip must name its reason: {}",
        result.skipped[0].1
    );
}

#[test]
fn skipped_files_leave_the_eligible_denominator_so_coverage_can_reach_100_percent() {
    use crate::daemon::indexing::reconcile_code_tree;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let repo = temp.path().join("cov-repo");
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::create_dir_all(repo.join(".git")).expect(".git dir");
    let cas_root = repo.join(".cas");
    std::fs::create_dir_all(&cas_root).expect("cas root");

    let good = repo.join("src/good.rs");
    std::fs::write(&good, "pub fn quicksilver_good() -> i64 { 1 }\n").expect("write good");
    let binary = repo.join("src/binary.rs");
    std::fs::write(&binary, [0x89, b'P', b'N', b'G', 0xFF, 0x00]).expect("write binary");

    let result = reconcile_code_tree(
        &[good, binary.clone()],
        std::slice::from_ref(&repo),
        &cas_root,
        true,
    )
    .expect("reconcile should succeed");
    assert_eq!(result.skipped.len(), 1);

    let state = cas_store::SqliteCodeVectorStore::open(&cas_root).expect("state store");
    let repositories = ["cov-repo".to_string()];
    let scan = repositories
        .iter()
        .find_map(|repository| state.index_state(repository).ok().flatten())
        .expect("a scan receipt must have been recorded");

    // The whole point of GH #698: the undecodable file leaves the denominator
    // rather than sitting in it as a failure no rerun can clear.
    assert_eq!(
        scan.failed_files, 0,
        "an undecodable file must not be counted as a failure"
    );
    assert_eq!(scan.eligible_files, scan.indexed_files, "coverage must reach 100% of eligible");
    assert_eq!(scan.skipped_files, 1);
    let detail = scan.skipped_detail.expect("the skip must be named");
    assert!(detail.contains("binary.rs"), "{detail}");
    assert!(detail.contains("not valid UTF-8"), "{detail}");
}
