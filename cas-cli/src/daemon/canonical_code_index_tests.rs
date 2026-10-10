//! cas-8256: one code index per project.
use std::path::{Path, PathBuf};
use std::time::Duration;

use cas_store::{CodeStore, SqliteCodeVectorStore, WriteBatching};

use super::*;
use crate::daemon::indexing::{code_scan_key, collect_source_files, reconcile_code_tree};

struct Project {
    _temp: tempfile::TempDir,
    main: PathBuf,
    cas_root: PathBuf,
}

impl Project {
    /// A canonical checkout `proj` with its store at `proj/.cas`, laid out
    /// the way the factory lays out worker worktrees.
    fn new() -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let main = root.join("proj");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.name", "Fixture"]);
        git(&main, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(main.join(".gitignore"), ".cas/\n").unwrap();
        std::fs::write(main.join("alpha.rs"), "pub fn canonical_alpha() {}\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "init"]);
        let cas_root = main.join(".cas");
        std::fs::create_dir_all(cas_root.join("worktrees")).unwrap();
        std::fs::write(cas_root.join("config.toml"), "[code]\nenabled = true\n").unwrap();
        Self {
            _temp: temp,
            main,
            cas_root,
        }
    }

    fn add_worker(&self, name: &str) -> PathBuf {
        let path = self.cas_root.join("worktrees").join(name);
        git(
            &self.main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("factory/{name}"),
                path.to_str().unwrap(),
            ],
        );
        std::fs::write(
            path.join("worker.rs"),
            format!("pub fn worker_only_{}() {{}}\n", name.replace('-', "_")),
        )
        .unwrap();
        path
    }

    /// What a worker's `cas serve` used to do on every boot.
    fn scan(&self, root: &Path) {
        let cfg = crate::config::Config::load(&self.cas_root).unwrap().code();
        let roots = vec![root.to_path_buf()];
        let files = collect_source_files(&roots, &cfg.extensions, &cfg.exclude_patterns);
        let result = reconcile_code_tree(&files, &roots, &self.cas_root, false).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    fn repositories(&self) -> Vec<(String, usize)> {
        cas_store::SqliteCodeIndexPurge::open_existing(&self.cas_root)
            .unwrap()
            .unwrap()
            .repository_file_counts()
            .unwrap()
    }

    fn search(&self, cas_root: &Path, query: &str) -> Vec<cas_search::CodeSearchResult> {
        crate::hybrid_search::code::open_code_search(cas_root)
            .unwrap()
            .search(&cas_search::CodeSearchOptions {
                query: query.to_string(),
                limit: 10,
                kind: None,
                language: None,
                include_source: false,
                min_score: 0.0,
                semantic: false,
            })
            .unwrap()
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
fn only_the_canonical_checkout_process_writes_the_code_index_cas_8256() {
    let project = Project::new();
    let worktree = project.add_worker("crisp-jay-9");

    assert_eq!(canonical_code_root(&project.cas_root), project.main);
    assert_eq!(
        code_index_role_from(&project.cas_root, &project.main, false),
        CodeIndexRole::Writer
    );

    let worker_env = code_index_role_from(&project.cas_root, &project.main, true);
    assert!(!worker_env.is_writer(), "factory worker env must not index");

    for cwd in [worktree.clone(), worktree.join("nested-missing-dir")] {
        let role = code_index_role_from(&project.cas_root, &cwd, false);
        assert!(
            !role.is_writer(),
            "linked worktree {} would index a copy",
            cwd.display()
        );
    }

    // A process outside any checkout keeps writing the explicit store's own checkout.
    let outside = project.main.parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(
        code_index_role_from(&project.cas_root, &outside, false),
        CodeIndexRole::Writer
    );

    assert_eq!(
        canonical_code_repositories(&project.cas_root),
        Some(vec!["proj".to_string()])
    );
}

#[test]
fn purge_removes_worktree_copies_and_workers_still_find_canonical_symbols_cas_8256() {
    let project = Project::new();
    let worktree = project.add_worker("crisp-jay-9");
    project.scan(&project.main);
    project.scan(&worktree);
    assert_eq!(
        project.repositories(),
        vec![("crisp-jay-9".to_string(), 2), ("proj".to_string(), 1)]
    );
    assert!(
        !project
            .search(&project.cas_root, "worker_only_crisp_jay_9")
            .is_empty()
    );

    let outcome =
        purge_non_canonical_code_index(&project.cas_root, WriteBatching::new(1, Duration::ZERO))
            .unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert!(!outcome.deferred);
    assert_eq!(outcome.stats.files_deleted, 2);
    assert_eq!(outcome.stats.symbols_deleted, 2);
    assert_eq!(outcome.stats.scan_receipts_deleted, 1);
    assert!(outcome.stats.writes.largest_transaction_rows <= 1);
    assert_eq!(project.repositories(), vec![("proj".to_string(), 1)]);

    let scans = SqliteCodeVectorStore::open(&project.cas_root).unwrap();
    assert!(
        scans
            .index_state(&code_scan_key(&worktree))
            .unwrap()
            .is_none()
    );
    assert!(
        scans
            .index_state(&code_scan_key(&project.main))
            .unwrap()
            .is_some()
    );

    // A worker resolves the shared store from its worktree and reads the
    // canonical index: canonical symbols answer, the purged copy does not.
    let worker_root = crate::store::find_cas_root_ignoring_env(&worktree).unwrap();
    assert_eq!(
        worker_root.canonicalize().unwrap(),
        project.cas_root.canonicalize().unwrap()
    );
    let hits = project.search(&worker_root, "canonical_alpha");
    assert!(
        hits.iter()
            .any(|hit| hit.file_path.ends_with("proj/alpha.rs")),
        "canonical symbol missing after purge: {hits:?}"
    );
    assert!(
        hits.iter()
            .all(|hit| !hit.file_path.contains("crisp-jay-9")),
        "worktree copy still answers: {hits:?}"
    );
    assert!(
        project
            .search(&worker_root, "worker_only_crisp_jay_9")
            .is_empty()
    );

    let again =
        purge_non_canonical_code_index(&project.cas_root, WriteBatching::default()).unwrap();
    assert!(again.stats.is_noop(), "{:?}", again.stats);
}

#[test]
fn removing_a_worktree_purges_only_its_copy_cas_8256() {
    let project = Project::new();
    let gone = project.add_worker("crisp-jay-9");
    let kept = project.add_worker("brave-hound-32");
    project.scan(&project.main);
    project.scan(&gone);
    project.scan(&kept);
    assert_eq!(project.repositories().len(), 3);

    crate::worktree::GitOperations::new(project.main.clone())
        .remove_worktree(&gone, true)
        .unwrap();

    assert_eq!(
        project.repositories(),
        vec![("brave-hound-32".to_string(), 2), ("proj".to_string(), 1)]
    );
    let store = crate::store::open_code_store(&project.cas_root).unwrap();
    assert!(store.list_files("crisp-jay-9", None).unwrap().is_empty());
    assert!(
        project
            .search(&project.cas_root, "worker_only_crisp_jay_9")
            .is_empty()
    );
    assert!(
        !project
            .search(&project.cas_root, "worker_only_brave_hound_32")
            .is_empty()
    );
}

#[test]
fn removal_of_a_worktree_never_purges_the_canonical_repository_cas_8256() {
    let project = Project::new();
    project.scan(&project.main);
    // A path whose directory name collides with the canonical checkout.
    let impostor = project.cas_root.join("worktrees").join("proj");
    assert!(purge_removed_worktree_code_index(&project.main, &impostor).is_none());
    assert_eq!(project.repositories(), vec![("proj".to_string(), 1)]);
}
