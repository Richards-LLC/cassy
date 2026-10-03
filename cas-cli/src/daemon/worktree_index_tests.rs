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
        git(&main, &["worktree", "add", "--detach", sibling.to_str().unwrap(), &old]);
        let cas_root = main.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        std::fs::write(cas_root.join("config.toml"), "[code]\nenabled = true\n").unwrap();
        Self { _temp: temp, main, sibling, cas_root }
    }

    pub fn scan(&self, root: &Path) -> super::CodeIndexResult {
        let cfg = crate::config::Config::load(&self.cas_root).unwrap().code();
        let roots = vec![root.to_path_buf()];
        let files = collect_source_files(&roots, &cfg.extensions, &cfg.exclude_patterns);
        reconcile_code_tree(&files, &roots, &self.cas_root, false).unwrap()
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").arg("-C").arg(root)
        .args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn different_commit_worktree_scans_preserve_each_others_files_cas_e4aa() {
    let fixture = Worktrees::new();
    for root in [&fixture.main, &fixture.sibling, &fixture.main, &fixture.sibling] {
        let result = fixture.scan(root);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.files_deleted, 0, "retired a sibling's existing file");
    }
    let store = crate::store::open_code_store(&fixture.cas_root).unwrap();
    let files = store.list_files("repo", None).unwrap();
    assert_eq!(files.len(), 3);
    assert!(store.get_file_by_path("repo", &fixture.sibling.join("old.rs").to_string_lossy()).unwrap().is_some());
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
    let holder = cas_search::Bm25Index::open(&super::indexing::code_index_dir(&fixture.cas_root)).unwrap();
    holder.delete_batch(["lock-probe"]).unwrap();
    let result = fixture.scan(&fixture.main);
    assert_eq!(result.files_deleted, 0);
    assert!(result.errors.is_empty(), "false retirement under writer lock: {:?}", result.errors);
    assert_eq!(crate::store::open_code_store(&fixture.cas_root).unwrap().list_files("repo", None).unwrap().len(), 3);
}

#[test]
fn held_tantivy_writer_defers_deleted_files_then_retries_cas_e4aa() {
    let fixture = Worktrees::new();
    assert!(fixture.scan(&fixture.main).errors.is_empty());
    std::fs::remove_file(fixture.main.join("new.rs")).unwrap();
    std::fs::remove_file(fixture.main.join("extra.rs")).unwrap();
    let holder = cas_search::Bm25Index::open(&super::indexing::code_index_dir(&fixture.cas_root)).unwrap();
    holder.delete_batch(["lock-probe"]).unwrap();
    let result = fixture.scan(&fixture.main);
    assert_eq!(result.files_deleted, 0);
    assert!(result.errors.is_empty(), "contention inflated file failures: {:?}", result.errors);
    assert_eq!(crate::store::open_code_store(&fixture.cas_root).unwrap().list_files("repo", None).unwrap().len(), 2);
    drop(holder);
    let retry = fixture.scan(&fixture.main);
    assert_eq!(retry.files_deleted, 2);
    assert!(retry.errors.is_empty(), "{:?}", retry.errors);
    assert_eq!(crate::store::open_code_store(&fixture.cas_root).unwrap().list_files("repo", None).unwrap().len(), 0);
}
