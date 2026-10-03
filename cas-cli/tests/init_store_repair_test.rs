//! `cas init` on a `.cas` that has its configuration but no store (cas-563f).
//!
//! A fresh clone of a project that commits `.cas/config.toml` has a `.cas`
//! directory without `cas.db`. `cas init --yes --no-integrations --force` used
//! to exit 0 there without creating the store: init_cas_dir returned as soon
//! as `.cas` existed. Init must create and migrate the store, and keep the
//! committed configuration as written, with or without --force.

use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

const COMMITTED_CONFIG: &str = "[sync]\nenabled = false\ntarget = \".claude/rules/custom\"\n";

fn cas_init_in(home: &Path, cwd: &Path, extra: &[&str]) -> Output {
    Command::new(cas::test_paths::cas_binary())
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("CAS_ROOT")
        .args(["init", "--yes", "--no-integrations"])
        .args(extra)
        .output()
        .expect("run cas init")
}

/// A project directory whose `.cas` holds only a committed config.toml.
fn config_only_project(temp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".cas")).unwrap();
    std::fs::write(project.join(".cas/config.toml"), COMMITTED_CONFIG).unwrap();
    (home, project)
}

fn assert_store_created_and_config_kept(project: &Path, output: &Output) {
    assert!(
        output.status.success(),
        "init must succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let db = project.join(".cas/cas.db");
    assert!(
        db.exists(),
        "init must create the store at {}",
        db.display()
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('entries', 'tasks', 'rules')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 3, "the store must be initialized and migrated");
    assert_eq!(
        std::fs::read_to_string(project.join(".cas/config.toml")).unwrap(),
        COMMITTED_CONFIG,
        "the committed config.toml must not be overwritten"
    );
}

#[test]
fn init_force_creates_the_store_in_a_config_only_cas() {
    let temp = TempDir::new().unwrap();
    let (home, project) = config_only_project(&temp);

    let output = cas_init_in(&home, &project, &["--force"]);

    assert_store_created_and_config_kept(&project, &output);
}

#[test]
fn init_without_force_also_creates_the_store_in_a_config_only_cas() {
    let temp = TempDir::new().unwrap();
    let (home, project) = config_only_project(&temp);

    let output = cas_init_in(&home, &project, &[]);

    assert_store_created_and_config_kept(&project, &output);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("already initialized"),
        "a .cas without its store is not initialized"
    );
}
