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
    let git = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(project)
        .status()
        .expect("initialize fixture Git root");
    assert!(git.success(), "initialize fixture Git root");
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
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
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
        .env("CAS_ROOT", first.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects", "--dry-run"])
        .assert()
        .success();
    let dry_out = String::from_utf8_lossy(&dry.get_output().stdout);
    assert!(
        dry_out.contains("projects/first") && dry_out.contains("nested/second"),
        "output was:\n{dry_out}"
    );
    assert!(dry_out.contains("DRY RUN"), "output was:\n{dry_out}");
    assert!(!missing.exists(), "dry run must not restore deleted skills");

    let synced = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", first.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let synced_out = String::from_utf8_lossy(&synced.get_output().stdout);
    assert!(
        synced_out.contains(&format!("Cassy {} · 2 projects refreshed · 0 failed", env!("CARGO_PKG_VERSION"))),
        "output was:\n{synced_out}"
    );
    assert!(
        synced_out.contains("not cloud-linked"),
        "offline/unlinked team phase must be an advisory skip; output was:\n{synced_out}"
    );
    assert!(
        !synced_out.contains("Syncing .claude files")
            && !synced_out.contains("[OK] Schema up to date")
            && !synced_out.contains("built-ins up to date"),
        "compact output must suppress successful phase details; output was:\n{synced_out}"
    );
    assert!(
        missing.is_file(),
        "native sweep must restore the stale builtin"
    );
}

/// cas-9d5c: the sweep used to stop descending at the first directory carrying
/// a `.cas/`, so a project nested inside another project was invisible, and the
/// user-level store was never migrated at all — while the banner still reported
/// a clean run.
#[test]
fn all_projects_refreshes_nested_projects_and_the_user_level_store() {
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let parent = projects.join("workspace");
    let nested = parent.join("inner");
    init_project(root, &parent);
    init_project(root, &nested);
    // A `.cas/` with no store: nothing to migrate, so it must be listed rather
    // than silently dropped from the receipt.
    std::fs::create_dir_all(projects.join("storeless/.cas")).unwrap();
    std::fs::write(
        projects.join("storeless/.cas/config.toml"),
        "[project]\ncanonical_id = \"storeless\"\n",
    )
    .unwrap();

    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", parent.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);

    assert!(
        output.contains("workspace/inner"),
        "a project nested under another project must be refreshed:\n{output}"
    );
    assert!(
        output.contains(&format!(
            "Cassy {} · 2 projects refreshed · 0 failed · 1 unregistered store(s) not refreshed",
            env!("CARGO_PKG_VERSION")
        )),
        "banner must count the skipped store:\n{output}"
    );
    assert!(
        output.contains("not refreshed (skipped_unregistered): 1"),
        "the storeless directory must be named:\n{output}"
    );
    assert!(
        output.contains("user-level store:"),
        "the user-level store needs its own reported phase:\n{output}"
    );

    let json = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", parent.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["--json", "update", "--all-projects"])
        .assert()
        .success();
    let json_out = String::from_utf8_lossy(&json.get_output().stdout);
    let receipt: serde_json::Value = json_out
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value.get("projects").is_some())
        .expect("all-projects JSON receipt");

    let stores = receipt["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|project| project["store"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(
        stores
            .iter()
            .any(|store| store.ends_with("workspace/inner/.cas")),
        "every project receipt names the store it migrated: {stores:?}"
    );
    assert_eq!(
        receipt["skipped_unregistered"]
            .as_array()
            .expect("skipped_unregistered array")
            .len(),
        1,
        "receipt was: {receipt}"
    );
    assert_eq!(
        receipt["user_level_store"]["store"]
            .as_str()
            .expect("user-level store path"),
        root.join(".test-home/.cas").to_string_lossy(),
        "receipt was: {receipt}"
    );
}

/// cas-9d5c: the user-level store must be migrated by the sweep, not merely
/// mentioned. Before this fix `~/.cas` only received the builtin distribution,
/// so on the reporting host it sat at schema 248 with 6 pending migrations
/// while the banner claimed every project was refreshed.
#[test]
fn all_projects_runs_migrations_against_the_user_level_store() {
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    let root = temp.path();
    let home = root.join(".test-home");
    let projects = root.join("projects");
    let project = projects.join("demo");
    init_project(root, &project);
    // Make `$HOME/.cas` a real store rather than the bare known-repo registry.
    // `cas init` guards the home directory, which is exactly the distinction
    // this phase exists to honour: the user-level store is host state.
    std::fs::create_dir_all(&home).unwrap();
    cas_cmd(root)
        .current_dir(&home)
        .args(["init", "--yes", "--allow-non-project"])
        .assert()
        .success();

    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", project.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["--json", "update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);

    let user_store = home.join(".cas").to_string_lossy().into_owned();
    let migrated_user_store = output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|value| {
            value.get("schema_status").is_some() && value["store"].as_str() == Some(&user_store)
        });
    assert!(
        migrated_user_store,
        "no schema_status receipt names the user-level store {user_store}:\n{output}"
    );
    assert!(
        !output.contains(&format!("\"project\":\"{}\"", home.to_string_lossy())),
        "the home directory must never appear as a project:\n{output}"
    );
}

#[test]
fn all_projects_repairs_stray_search_roots_and_reports_clean_projects() {
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let stranded = projects.join("stranded");
    let clean = projects.join("clean");
    init_project(root, &stranded);
    init_project(root, &clean);
    let entry_id = seed_stray_memory(&stranded);

    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", stranded.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);
    assert!(
        output.contains("projects/clean") && output.contains("projects/stranded"),
        "project table rows missing from output:\n{output}"
    );
    assert!(
        !output.contains("[OK] search index:"),
        "successful phase details must stay out of compact output:\n{output}"
    );
    assert!(
        !stranded.join(".cas/index/meta.json").exists(),
        "the legacy Tantivy root must be retired"
    );

    let store = cas::store::open_store(&stranded.join(".cas")).expect("reopen store");
    let entries = store.list().expect("list entries");
    let hits = SearchIndex::open(&cas::hybrid_search::tantivy_index_dir(
        &stranded.join(".cas"),
    ))
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
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
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
        // Pin the current project to the fixture so discovery never walks up
        // from the fixture into the real user-level ~/.cas.
        .env("CAS_ROOT", project.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);
    assert!(
        output.contains("[WARN] search index: busy"),
        "busy repair receipt missing from output:\n{output}"
    );
    let banner = format!(
        "Cassy {} · 1 projects refreshed · 0 failed",
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        output.contains(&banner),
        "busy repair must not fail the project refresh:\n{output}"
    );
}

fn open_project_db(project: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(project.join(".cas/cas.db")).expect("open project store");
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .expect("busy timeout");
    conn
}

fn column_exists(conn: &rusqlite::Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
        [column],
        |row| row.get::<_, i64>(0),
    )
    .expect("read table info")
        > 0
}

/// cas-91a3: m260 refused by another process's write lock must stop that
/// project's later phases with one message naming the holders. Before the
/// fix the skills phase still ran and died on "no such column: origin_project".
#[test]
fn all_projects_skips_later_phases_when_a_held_write_lock_blocks_migration() {
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let project = projects.join("locked");
    init_project(root, &project);
    {
        // The store as a 3.34 session left it: m260 not yet applied.
        let conn = open_project_db(&project);
        conn.execute_batch(
            "ALTER TABLE rules DROP COLUMN origin_project;
             ALTER TABLE entries DROP COLUMN origin_project;
             DELETE FROM cas_migrations WHERE id >= 260;",
        )
        .expect("rewind the store to before m260");
    }

    // Another session holds the WAL writer for the whole update.
    let holder = open_project_db(&project);
    holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold the store's write lock");
    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", project.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .failure();
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&update.get_output().stdout),
        String::from_utf8_lossy(&update.get_output().stderr)
    );
    holder
        .execute_batch("ROLLBACK")
        .expect("release the write lock");
    drop(holder);

    assert!(
        !output.contains("no such column"),
        "a stale store must never reach a phase that reads the new schema:\n{output}"
    );
    assert!(
        output.contains("database is locked"),
        "the migration failure names its cause:\n{output}"
    );
    assert!(
        output.contains(&format!("pid {}", std::process::id())),
        "the process holding the store is named:\n{output}"
    );
    assert!(
        output.contains("stop the Cassy sessions in") && output.contains("rerun `cas update`"),
        "the message says what to do:\n{output}"
    );
    assert_eq!(
        output.matches("stop the Cassy sessions in").count(),
        1,
        "one actionable message, not one per skipped phase:\n{output}"
    );

    // With the session gone, the same command converges.
    cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", project.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let conn = open_project_db(&project);
    assert!(column_exists(&conn, "rules", "origin_project"));
    assert!(column_exists(&conn, "entries", "origin_project"));
}

/// cas-3af4: a legacy store (entries, rules, metadata and sessions only; no
/// task table, no migration ledger, user_version 0, rules without
/// operator_authority) used to be called "not initialized": its migration
/// phase reported ok and the skills phase failed on "no such column:
/// operator_authority". It is migrated first now, and the refresh succeeds.
#[test]
fn all_projects_migrates_a_legacy_store_before_reading_its_rules() {
    let temp = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let project = projects.join("legacy");
    init_project(root, &project);
    for name in ["cas.db", "cas.db-wal", "cas.db-shm"] {
        let _ = std::fs::remove_file(project.join(".cas").join(name));
    }
    {
        // The current entries/rules schema without the columns m259/m260 add.
        // (SQLite's DROP COLUMN cannot rewrite this commented CREATE TABLE.)
        let legacy = cas_store::ENTRIES_RULES_SCHEMA
            .lines()
            .filter(|line| !line.contains("operator_authority") && !line.contains("cas-5372"))
            .collect::<Vec<_>>()
            .join("\n")
            .replace(",\n    origin_project TEXT\n)", "\n)");
        assert!(!legacy.contains("origin_project") && !legacy.contains("operator_authority"));
        let conn = open_project_db(&project);
        conn.execute_batch(&legacy)
            .expect("create the legacy entries/rules-only store");
        assert!(!column_exists(&conn, "rules", "operator_authority"));
    }

    let update = cas_cmd(root)
        .current_dir(root)
        .env("CAS_ROOT", project.join(".cas"))
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&update.get_output().stdout);
    assert!(
        !output.contains("no such column") && !output.contains("not initialized"),
        "the legacy store is migrated, not skipped or read stale:\n{output}"
    );

    let conn = open_project_db(&project);
    assert!(column_exists(&conn, "rules", "operator_authority"));
    assert!(column_exists(&conn, "rules", "origin_project"));
    let tasks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'tasks'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tasks, 1, "the missing task table is installed");
}
