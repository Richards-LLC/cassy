//! Integration tests for blame and attribution functionality

use std::process::Command;
use tempfile::TempDir;

/// Helper to run git commands in a directory
fn git_cmd(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("Failed to run git command")
}

/// Helper to run cas commands
fn cas_cmd(dir: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::new(cas::test_paths::cas_binary());
    let home = dir.join(".test-home");
    let xdg = dir.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    // Clear CAS_ROOT to prevent env pollution from parent shell
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Setup a test repository with CAS initialized
fn setup_test_repo() -> TempDir {
    let temp = TempDir::new().unwrap();
    let path = temp.path();

    // Initialize git repo
    git_cmd(path, &["init"]);
    git_cmd(path, &["config", "user.email", "test@test.com"]);
    git_cmd(path, &["config", "user.name", "Test User"]);

    // Initialize CAS
    cas_cmd(path)
        .current_dir(path)
        .args(["init", "--yes"])
        .assert()
        .success();

    temp
}

#[test]
fn test_file_change_storage_and_retrieval() {
    use cas_store::FileChangeStore;

    let temp = setup_test_repo();
    let path = temp.path();
    let cas_dir = path.join(".cas");

    let store = cas_store::SqliteFileChangeStore::open(&cas_dir).unwrap();
    store.init().unwrap();

    let session_id = "test-session-123";

    // Add a file change
    let change = cas_types::FileChange::new(
        "fc-test-1".to_string(),
        session_id.to_string(),
        "agent-1".to_string(),
        "test-repo".to_string(),
        "src/main.rs".to_string(),
        cas_types::ChangeType::Created,
        "Write".to_string(),
        "abc123".to_string(),
    );
    store.add(&change).unwrap();

    // Retrieve and verify
    let changes = store.list_by_session(session_id, 100).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].file_path, "src/main.rs");
    assert_eq!(changes[0].change_type, cas_types::ChangeType::Created);
}
