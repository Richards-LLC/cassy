//! End-to-end coverage for the native multi-project update sweep (cas-4ee9).

use assert_cmd::Command;
use cas::hybrid_search::{SearchIndex, SearchOptions};
use cas::types::Entry;
use std::path::{Path, PathBuf};
use tantivy::directory::{Directory, META_LOCK};
use tempfile::TempDir;

fn cas_cmd(root: &Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

fn init_project(root: &Path, project: &Path) {
    std::fs::create_dir_all(project).unwrap();
    cas_cmd(root)
        .current_dir(project)
        .args(["init", "--yes"])
        .assert()
        .success();
}

fn worker_skill(project: &Path) -> PathBuf {
    project.join(".claude/skills/cas-worker/SKILL.md")
}

fn seed_stray_memory(project: &Path) -> String {
    let cas_root = project.join(".cas");
    let store = cas::store::open_store(&cas_root).expect("open project store");
    let entry = Entry::new(
        "legacy-update-memory".to_string(),
        "legacy update repair makes this memory searchable".to_string(),
    );
    store.add(&entry).expect("add entry");
    SearchIndex::open(&cas_root.join("index"))
        .expect("open legacy index")
        .index_entry(&entry)
        .expect("seed legacy index");
    store
        .mark_indexed(&entry.id)
        .expect("mark entry indexed in legacy root");
    entry.id
}

#[test]
fn all_projects_dry_run_is_non_mutating_then_syncs_every_discovered_project() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let first = projects.join("first");
    let second = projects.join("nested/second");
    init_project(root, &first);
    init_project(root, &second);

    let missing = worker_skill(&first);
    std::fs::remove_file(&missing).unwrap();

    let dry = cas_cmd(root)
        .current_dir(root)
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects", "--dry-run"])
        .assert()
        .success();
    let dry_out = String::from_utf8_lossy(&dry.get_output().stdout);
    assert!(
        dry_out.contains("2 local Cassy project(s)"),
        "output was:\n{dry_out}"
    );
    assert!(dry_out.contains("DRY RUN"), "output was:\n{dry_out}");
    assert!(!missing.exists(), "dry run must not restore deleted skills");

    let synced = cas_cmd(root)
        .current_dir(root)
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let synced_out = String::from_utf8_lossy(&synced.get_output().stdout);
    assert!(
        synced_out.contains("2 succeeded, 0 failed"),
        "output was:\n{synced_out}"
    );
    assert!(
        synced_out.contains("membership: skipped: not cloud-linked"),
        "offline/unlinked team phase must be an advisory skip; output was:\n{synced_out}"
    );
    assert!(
        synced_out.contains("cloud sync: skipped: not cloud-linked"),
        "offline/unlinked cloud phase must be an advisory skip; output was:\n{synced_out}"
    );
    assert!(
        missing.is_file(),
        "native sweep must restore the stale builtin"
    );
}

#[test]
fn all_projects_repairs_stray_search_roots_and_reports_clean_projects() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let stranded = projects.join("stranded");
    let clean = projects.join("clean");
    init_project(root, &stranded);
    init_project(root, &clean);
    let entry_id = seed_stray_memory(&stranded);

    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);
    assert!(
        output.contains("[OK] search index: repaired 1 stranded memories (1 re-queued)"),
        "repair receipt missing from output:\n{output}"
    );
    assert!(
        output.contains("[OK] search index: no stray root"),
        "clean-project receipt missing from output:\n{output}"
    );
    assert!(
        !stranded.join(".cas/index/meta.json").exists(),
        "the legacy Tantivy root must be retired"
    );

    let store = cas::store::open_store(&stranded.join(".cas")).expect("reopen store");
    let entries = store.list().expect("list entries");
    let hits = SearchIndex::open(&stranded.join(".cas/index/tantivy"))
        .expect("open canonical index")
        .search(
            &SearchOptions {
                query: "legacy update repair".to_string(),
                ..Default::default()
            },
            &entries,
        )
        .expect("search repaired index");
    assert_eq!(
        hits.first().map(|hit| hit.id.as_str()),
        Some(entry_id.as_str()),
        "the stranded memory must be searchable after update"
    );
}

#[test]
fn all_projects_warns_on_a_held_legacy_lock_but_still_succeeds() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let project = projects.join("busy");
    init_project(root, &project);
    seed_stray_memory(&project);

    let legacy = tantivy::Index::open_in_dir(project.join(".cas/index")).unwrap();
    let _held = legacy
        .directory()
        .acquire_lock(&META_LOCK)
        .expect("hold legacy metadata lock");
    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);
    assert!(
        output.contains("[WARN] search index: busy"),
        "busy repair receipt missing from output:\n{output}"
    );
    assert!(
        output.contains("Total: 1 succeeded, 0 failed"),
        "busy repair must not fail the project refresh:\n{output}"
    );
}
