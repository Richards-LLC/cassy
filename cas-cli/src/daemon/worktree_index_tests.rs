//! cas-e4aa: production reconciliation against disposable worktrees and a held writer.
use std::path::{Path, PathBuf};

use super::indexing::{collect_source_files, reconcile_code_tree};

pub(crate) struct Worktrees {
    _temp: tempfile::TempDir,
    pub main: PathBuf,
    pub sibling: PathBuf,
    pub cas_root: PathBuf,
}

impl Worktrees {
    pub fn new() -> Self {
        let temp = tempfile::TempDir::new().expect("temp repo");
        // Canonicalize once: macOS /var is a symlink to /private/var.
        let root = temp.path().canonicalize().unwrap();
        let main = root.join("main/repo");
        let sibling = root.join("linked/repo");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["config", "user.name", "Fixture"]);
        git(&main, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(main.join("old.rs"), "pub fn old_only() {}\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "old"]);
        let old = git(&main, &["rev-parse", "HEAD"]);
        std::fs::remove_file(main.join("old.rs")).unwrap();
        std::fs::write(main.join("new.rs"), "pub fn new_only() {}\n").unwrap();
        std::fs::write(main.join("extra.rs"), "pub fn extra() {}\n").unwrap();
        git(&main, &["add", "-A"]);
        git(&main, &["commit", "-qm", "new"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "--detach",
                sibling.to_str().unwrap(),
                &old,
            ],
        );
        let cas_root = main.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        std::fs::write(cas_root.join("config.toml"), "[code]\nenabled = true\n").unwrap();
        Self {
            _temp: temp,
            main,
            sibling,
            cas_root,
        }
    }

    pub fn scan(&self, root: &Path) -> super::CodeIndexResult {
        let cfg = crate::config::Config::load(&self.cas_root).unwrap().code();
        let roots = vec![root.to_path_buf()];
        let files = collect_source_files(&roots, &cfg.extensions, &cfg.exclude_patterns);
        reconcile_code_tree(&files, &roots, &self.cas_root, false).unwrap()
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn different_commit_worktree_scans_preserve_each_others_files_cas_e4aa() {
    let fixture = Worktrees::new();
    for root in [
        &fixture.main,
        &fixture.sibling,
        &fixture.main,
        &fixture.sibling,
    ] {
        let result = fixture.scan(root);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.files_deleted, 0, "retired a sibling's existing file");
    }
    let store = crate::store::open_code_store(&fixture.cas_root).unwrap();
    let files = store.list_files("repo", None).unwrap();
    assert_eq!(files.len(), 3);
    assert!(
        store
            .get_file_by_path("repo", &fixture.sibling.join("old.rs").to_string_lossy())
            .unwrap()
            .is_some()
    );
    // A genuine local deletion still retires; the sibling's source survives.
    std::fs::remove_file(fixture.main.join("extra.rs")).unwrap();
    let result = fixture.scan(&fixture.main);
    assert_eq!(result.files_deleted, 1);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(store.list_files("repo", None).unwrap().len(), 2);
}

#[test]
fn sibling_presence_under_a_held_tantivy_writer_has_no_retirement_failures_cas_e4aa() {
    let fixture = Worktrees::new();
    assert!(fixture.scan(&fixture.main).errors.is_empty());
    assert!(fixture.scan(&fixture.sibling).errors.is_empty());
    // Bm25Index caches the writer until drop, exactly like a foreign daemon.
    let holder =
        cas_search::Bm25Index::open(&super::indexing::code_index_dir(&fixture.cas_root)).unwrap();
    holder.delete_batch(["lock-probe"]).unwrap();
    let result = fixture.scan(&fixture.main);
    assert_eq!(result.files_deleted, 0);
    assert!(
        result.errors.is_empty(),
        "false retirement under writer lock: {:?}",
        result.errors
    );
    assert_eq!(
        crate::store::open_code_store(&fixture.cas_root)
            .unwrap()
            .list_files("repo", None)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn held_tantivy_writer_defers_deleted_files_then_retries_cas_e4aa() {
    let fixture = Worktrees::new();
    assert!(fixture.scan(&fixture.main).errors.is_empty());
    std::fs::remove_file(fixture.main.join("new.rs")).unwrap();
    std::fs::remove_file(fixture.main.join("extra.rs")).unwrap();
    let holder =
        cas_search::Bm25Index::open(&super::indexing::code_index_dir(&fixture.cas_root)).unwrap();
    holder.delete_batch(["lock-probe"]).unwrap();
    let result = fixture.scan(&fixture.main);
    assert_eq!(result.files_deleted, 0);
    assert_eq!(result.files_deferred, 2);
    assert!(
        result.errors.is_empty(),
        "contention inflated file failures: {:?}",
        result.errors
    );
    assert_eq!(
        crate::store::open_code_store(&fixture.cas_root)
            .unwrap()
            .list_files("repo", None)
            .unwrap()
            .len(),
        2
    );
    drop(holder);
    let retry = fixture.scan(&fixture.main);
    assert_eq!(retry.files_deleted, 2);
    assert!(retry.errors.is_empty(), "{:?}", retry.errors);
    assert_eq!(
        crate::store::open_code_store(&fixture.cas_root)
            .unwrap()
            .list_files("repo", None)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn checkout_selection_and_scan_receipts_are_worktree_scoped_cas_e4aa() {
    use super::indexing::{code_project_root_from, code_scan_key};
    let fixture = Worktrees::new();
    assert_eq!(
        code_project_root_from(&fixture.cas_root, &fixture.sibling),
        fixture.sibling
    );
    // An unrelated checkout cannot override an explicit store.
    let unrelated = fixture.main.parent().unwrap().join("other");
    std::fs::create_dir_all(&unrelated).unwrap();
    git(&unrelated, &["init", "-q"]);
    assert_eq!(
        code_project_root_from(&fixture.cas_root, &unrelated),
        fixture.main
    );
    let scans = cas_store::SqliteCodeVectorStore::open(&fixture.cas_root).unwrap();
    scans
        .record_scan(
            "repo",
            999,
            1,
            998,
            0,
            None,
            None,
            Some("legacy retirement error"),
        )
        .unwrap();
    fixture.scan(&fixture.main);
    fixture.scan(&fixture.sibling);
    let main = scans
        .index_state(&code_scan_key(&fixture.main))
        .unwrap()
        .unwrap();
    let sibling = scans
        .index_state(&code_scan_key(&fixture.sibling))
        .unwrap()
        .unwrap();
    assert_eq!(
        (main.eligible_files, main.indexed_files, main.failed_files),
        (2, 2, 0)
    );
    assert_eq!(
        (
            sibling.eligible_files,
            sibling.indexed_files,
            sibling.failed_files
        ),
        (1, 1, 0)
    );
    assert_ne!(main.last_head, sibling.last_head);
    assert_eq!(
        scans.index_state("repo").unwrap().unwrap().eligible_files,
        999
    );
}

#[test]
fn retirement_ownership_checks_path_boundaries_cas_e4aa() {
    use super::indexing::code_file_in_root;
    let fixture = Worktrees::new();
    let normalized =
        |path: &Path| cas_store::SqliteCodeStore::normalize_path(&path.to_string_lossy());
    assert!(code_file_in_root(
        &normalized(&fixture.main.join("deleted.rs")),
        &fixture.main
    ));
    assert!(!code_file_in_root(
        &normalized(&fixture.sibling.join("old.rs")),
        &fixture.main
    ));
    assert!(!code_file_in_root(
        &normalized(&fixture.main.with_file_name("repo-extra").join("source.rs")),
        &fixture.main
    ));
    assert!(
        !code_file_in_root("src/old.rs", &fixture.main),
        "relative historical row has no ownership"
    );
    let nested = fixture.main.join("nested/repo");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    git(
        &fixture.main,
        &[
            "worktree",
            "add",
            "--detach",
            nested.to_str().unwrap(),
            "HEAD",
        ],
    );
    assert!(!code_file_in_root(
        &normalized(&nested.join("new.rs")),
        &fixture.main
    ));
    let cfg = crate::config::Config::load(&fixture.cas_root)
        .unwrap()
        .code();
    let paths = collect_source_files(
        &[fixture.main.clone()],
        &cfg.extensions,
        &cfg.exclude_patterns,
    );
    assert_eq!(
        paths.len(),
        2,
        "main collector walked a nested linked checkout"
    );
    assert!(paths.iter().all(|path| !path.starts_with(&nested)));
}

#[cfg(unix)]
#[test]
fn retirement_ownership_and_receipts_follow_symlink_aliases_cas_e4aa() {
    use super::indexing::{code_file_in_root, code_project_root_from, code_scan_key};
    let fixture = Worktrees::new();
    let alias = fixture.main.parent().unwrap().join("repo-alias");
    std::os::unix::fs::symlink(&fixture.main, &alias).unwrap();
    let normalized =
        cas_store::SqliteCodeStore::normalize_path(&alias.join("gone.rs").to_string_lossy());
    assert!(code_file_in_root(&normalized, &fixture.main));
    assert_eq!(code_scan_key(&alias), code_scan_key(&fixture.main));
    assert_eq!(code_project_root_from(&fixture.cas_root, &alias), alias);
    assert!(fixture.scan(&alias).errors.is_empty());
    let canonical_scan = fixture.scan(&fixture.main);
    assert_eq!(
        canonical_scan.files_deleted, 0,
        "alias spelling retired an existing source"
    );
    assert!(canonical_scan.errors.is_empty());
    let store = crate::store::open_code_store(&fixture.cas_root).unwrap();
    let files = store.list_files("repo", None).unwrap();
    let cfg = crate::config::Config::load(&fixture.cas_root)
        .unwrap()
        .code();
    let paths = collect_source_files(
        &[fixture.main.clone()],
        &cfg.extensions,
        &cfg.exclude_patterns,
    );
    assert_eq!(
        super::indexing::checkout_indexed_file_count(&files, &paths),
        2
    );
}

#[test]
fn daemon_retries_a_deferred_retirement_without_new_events_cas_e4aa() {
    use super::{CodeWatcher, WatcherConfig};
    let fixture = Worktrees::new();
    assert!(fixture.scan(&fixture.main).errors.is_empty());
    std::fs::remove_file(fixture.main.join("new.rs")).unwrap();
    std::fs::remove_file(fixture.main.join("extra.rs")).unwrap();
    let watcher = CodeWatcher::new(WatcherConfig {
        watch_paths: vec![fixture.main.clone()],
        extensions: vec!["rs".into()],
        ..Default::default()
    });
    watcher.seed_initial([]);
    let holder =
        cas_search::Bm25Index::open(&super::indexing::code_index_dir(&fixture.cas_root)).unwrap();
    holder.delete_batch(["lock-probe"]).unwrap();
    let deferred = super::indexing::run_code_index_cycle(&watcher, &fixture.cas_root).unwrap();
    assert_eq!(deferred.files_deferred, 2);
    assert!(deferred.errors.is_empty(), "{:?}", deferred.errors);
    drop(holder);
    let retry = super::indexing::run_code_index_cycle(&watcher, &fixture.cas_root).unwrap();
    assert_eq!(retry.files_deleted, 2);
    assert_eq!(retry.files_deferred, 0);
    assert!(retry.errors.is_empty(), "{:?}", retry.errors);
}

fn nested_watch_cycle(explicit_nested_root: bool) {
    use super::{CodeWatcher, WatcherConfig};
    let fixture = Worktrees::new();
    let nested = fixture.main.join("nested/repo");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    git(
        &fixture.main,
        &[
            "worktree",
            "add",
            "--detach",
            nested.to_str().unwrap(),
            "HEAD",
        ],
    );
    assert!(fixture.scan(&fixture.main).errors.is_empty());
    assert!(fixture.scan(&nested).errors.is_empty());
    let scans = cas_store::SqliteCodeVectorStore::open(&fixture.cas_root).unwrap();
    let key = super::indexing::code_scan_key(&nested);
    let before = scans.index_state(&key).unwrap().unwrap();
    std::fs::remove_file(fixture.main.join("extra.rs")).unwrap();
    std::fs::remove_file(nested.join("extra.rs")).unwrap();
    std::fs::write(nested.join("new.rs"), "pub fn nested_modified() {}\n").unwrap();
    let mut roots = vec![fixture.main.clone()];
    if explicit_nested_root {
        roots.push(nested.clone());
    }
    let mut watcher = CodeWatcher::new(WatcherConfig {
        watch_paths: roots,
        extensions: vec!["rs".into()],
        ..Default::default()
    });
    // Actual production path emitter accepts the recursive nested modified
    // event and the outer deletion. The cycle must retain configured authority.
    watcher.emit_test_path(nested.join("new.rs"));
    watcher.emit_test_path(fixture.main.join("extra.rs"));
    let cycle = super::indexing::run_code_index_cycle(&watcher, &fixture.cas_root).unwrap();
    assert!(cycle.errors.is_empty(), "{:?}", cycle.errors);
    assert_eq!(
        cycle.files_deleted,
        if explicit_nested_root { 2 } else { 1 }
    );
    let store = crate::store::open_code_store(&fixture.cas_root).unwrap();
    let nested_manifest = store
        .get_file_by_path("repo", &nested.join("extra.rs").to_string_lossy())
        .unwrap();
    let after = scans.index_state(&key).unwrap().unwrap();
    if explicit_nested_root {
        assert!(nested_manifest.is_none());
        assert_eq!(
            (
                after.eligible_files,
                after.indexed_files,
                after.failed_files
            ),
            (1, 1, 0)
        );
        assert_eq!(
            cycle.files_indexed, 1,
            "configured nested modification was ignored"
        );
    } else {
        assert!(
            nested_manifest.is_some(),
            "outer cycle retired another checkout's manifest"
        );
        assert_eq!(
            after.last_scan_at, before.last_scan_at,
            "incidental event overwrote a full scan receipt"
        );
        assert_eq!((after.eligible_files, after.indexed_files), (2, 2));
        assert_eq!(
            cycle.files_indexed, 0,
            "unconfigured nested event widened the scan"
        );
    }
}

#[test]
fn nested_watch_event_cannot_expand_outer_fullscan_authority_cas_e4aa() {
    nested_watch_cycle(false);
}

#[test]
fn explicitly_configured_nested_watch_root_still_reconciles_cas_e4aa() {
    nested_watch_cycle(true);
}
