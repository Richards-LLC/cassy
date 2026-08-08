use chrono::Utc;
use std::sync::Arc;

use crate::daemon::decay::apply_memory_decay;
use crate::daemon::{CodeIndexResult, DaemonConfig, DaemonStatus, WatchEvent};
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
fn test_in_context_entries_are_skipped() {
    let mut pinned = make_entry("pin-001", EntryType::Observation, MemoryTier::InContext);
    pinned.helpful_count = 0;
    pinned.harmful_count = 5;

    let store = make_store(vec![pinned]);
    let count = apply_memory_decay(&store).unwrap();

    assert_eq!(count, 0);

    let updated = store.get("pin-001").unwrap();
    assert_eq!(updated.memory_tier, MemoryTier::InContext);
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
    assert!(result.errors.is_empty(), "unexpected errors: {:?}", result.errors);
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
    assert_eq!(name, "my-project", "must be the work-tree root, not `parser`");
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
    assert!(result.errors.is_empty(), "unexpected errors: {:?}", result.errors);
    assert_eq!(result.files_indexed, 1);
    assert!(result.symbols_indexed > 0, "no symbols parsed");

    let store = crate::store::open_code_store(&cas_root).expect("code store");
    assert!(store.count_symbols().expect("count") > 0, "code_symbols stayed empty");
    assert!(
        crate::hybrid_search::code::code_search_available(&cas_root),
        "`.cas/index/code` was never written, so code_search would still return the stub"
    );

    // And the index actually answers, rather than merely existing.
    let search =
        crate::hybrid_search::code::open_code_search(&cas_root).expect("open code search");
    let hits = search
        .search(&cas_search::CodeSearchOptions {
            query: "quicksilver_add".to_string(),
            limit: 5,
            ..Default::default()
        })
        .expect("search");
    assert!(!hits.is_empty(), "code search returned nothing for an indexed symbol");
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
    if !git(&["init", "-q"]).map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("git unavailable; skipping commit-hash assertion");
        return;
    }
    let _ = git(&["config", "user.email", "t@example.com"]);
    let _ = git(&["config", "user.name", "T"]);

    let file = repo.join("src/lib.rs");
    std::fs::write(&file, "pub fn tracked() {}\n").expect("write file");
    let _ = git(&["add", "-A"]);
    let _ = git(&["commit", "-qm", "seed"]);

    let head = String::from_utf8(
        git(&["rev-parse", "HEAD"]).expect("rev-parse").stdout,
    )
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
