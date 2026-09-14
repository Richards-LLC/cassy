//! Integration tests for CAS hooks system
//!
//! Tests the full flow of hook events from Claude Code through to storage.

use assert_cmd::Command;
use cas::store::{
    open_agent_store, open_file_change_store, open_rule_store_local, open_store_local,
    open_task_store_local,
};
use cas::types::{
    Agent, ClaimResult, Dependency, DependencyType, Entry, EntryType, Rule, Task, TaskStatus,
    TaskType,
};
pub(crate) use predicates::prelude::*;
use tempfile::TempDir;

// =============================================================================
// Test Utilities
// =============================================================================

/// Create cas command for temp directory
pub(crate) fn cas_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = dir.path().join(".test-home");
    let xdg = dir.path().join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.current_dir(dir.path());
    // Clear CAS_ROOT to prevent env pollution from parent shell
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Initialize CAS in temp directory
pub(crate) fn init_cas(dir: &TempDir) {
    cas_cmd(dir).args(["init", "--yes"]).assert().success();
}

/// Initialize CAS with dev mode enabled
#[allow(dead_code)]
pub(crate) fn init_cas_dev_mode(dir: &TempDir) {
    init_cas(dir);
    enable_cas_dev_mode(dir);
}

/// Enable the current dev-tracer config in an initialized fixture.
pub(crate) fn enable_cas_dev_mode(dir: &TempDir) {
    // Config is now saved as TOML (auto-migrated from YAML if it existed)
    let config_path = dir.path().join(".cas/config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[dev]\ndev_mode = true\n");
    std::fs::write(&config_path, config).unwrap();
}

/// Send hook event via stdin and return stdout
pub(crate) fn send_hook(dir: &TempDir, event: &str, input: &serde_json::Value) -> String {
    let output = cas_cmd(dir)
        .args(["hook", event])
        .write_stdin(serde_json::to_string(input).unwrap())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Create PostToolUse input for Write tool
pub(crate) fn write_tool_input(session_id: &str, file_path: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": file_path, "content": "test content\nline 2\nline 3"},
        "tool_response": {}
    })
}

/// Create PostToolUse input for Edit tool
pub(crate) fn edit_tool_input(
    session_id: &str,
    file_path: &str,
    old_lines: usize,
    new_lines: usize,
) -> serde_json::Value {
    let old_string = (0..old_lines)
        .map(|i| format!("old line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    let new_string = (0..new_lines)
        .map(|i| format!("new line {}", i))
        .collect::<Vec<_>>()
        .join("\n");

    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": file_path,
            "old_string": old_string,
            "new_string": new_string
        },
        "tool_response": {}
    })
}

/// Create PostToolUse input for Bash with exit code
pub(crate) fn bash_tool_input(
    session_id: &str,
    command: &str,
    exit_code: i32,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
        "tool_response": {
            "exitCode": exit_code,
            "stderr": if exit_code != 0 { "error: command failed" } else { "" }
        }
    })
}

/// Create SessionStart hook input
pub(crate) fn session_start_input(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "SessionStart"
    })
}

/// Create Stop hook input
pub(crate) fn stop_input(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "Stop"
    })
}

/// List entries in CAS through the local store fixture.
pub(crate) fn open_entries(dir: &TempDir) -> Vec<Entry> {
    open_store_local(&dir.path().join(".cas"))
        .expect("entry store should open")
        .list()
        .expect("entries should list")
}

/// Count entries in CAS through the local store fixture.
pub(crate) fn count_entries(dir: &TempDir) -> usize {
    open_entries(dir).len()
}

/// Return durable file-attribution records for assertions on the actual hook
/// capture contract.
pub(crate) fn file_changes(dir: &TempDir) -> Vec<cas::types::FileChange> {
    open_file_change_store(&dir.path().join(".cas"))
        .expect("file-change store should open")
        .list_recent(1_000)
        .expect("file changes should list")
}

/// Count observations in the dev tracer's durable buffer.
///
/// PostToolUse intentionally buffers raw observations only when dev mode is
/// enabled. The public TraceStore API requires a session ID, while each hook
/// process owns a generated tracer session, so this fixture queries the
/// bounded buffer table directly to verify persistence across hook processes.
pub(crate) fn count_buffered_observations(dir: &TempDir) -> usize {
    buffered_observations(dir).len()
}

/// Return buffered tool observations, including content and error metadata.
pub(crate) fn buffered_observations(dir: &TempDir) -> Vec<(String, String, bool, Option<i32>)> {
    buffered_observations_with_sessions(dir)
        .into_iter()
        .map(|(_, tool_name, content, is_error, exit_code)| {
            (tool_name, content, is_error, exit_code)
        })
        .collect()
}

/// Return buffered tool observations with their persisted session identity.
pub(crate) fn buffered_observations_with_sessions(
    dir: &TempDir,
) -> Vec<(String, String, String, bool, Option<i32>)> {
    let trace_path = dir.path().join(".cas/traces.db");
    if !trace_path.exists() {
        return Vec::new();
    }

    let db = rusqlite::Connection::open(trace_path).expect("trace store should open");
    let mut stmt = db
        .prepare(
            "SELECT session_id, tool_name, content, is_error, exit_code
             FROM observation_buffer ORDER BY id",
        )
        .expect("buffer query should prepare");
    stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get::<_, i32>(3)? != 0,
            row.get(4)?,
        ))
    })
    .expect("buffer rows should query")
    .collect::<Result<Vec<_>, _>>()
    .expect("buffer rows should decode")
}

/// Seed a buffer row under the harness session ID used by a Stop fixture.
///
/// The production tracer generates a process-local ID, so a multi-process
/// hook integration test cannot otherwise create a row that Stop will match.
/// This keeps the fixture on the real SQLite schema while exercising the
/// production synthesis and clear path unchanged.
pub(crate) fn seed_buffered_observation(
    dir: &TempDir,
    session_id: &str,
    tool_name: &str,
    content: &str,
    exit_code: Option<i32>,
    is_error: bool,
) {
    cas::tracing::TraceStore::open(&dir.path().join(".cas/traces.db"))
        .expect("trace store should initialize");
    let db = rusqlite::Connection::open(dir.path().join(".cas/traces.db"))
        .expect("trace store should open");
    db.execute(
        "INSERT INTO observation_buffer
         (session_id, tool_name, file_path, content, exit_code, is_error, timestamp)
         VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            session_id,
            tool_name,
            content,
            exit_code,
            is_error as i32,
            chrono::Utc::now().to_rfc3339(),
        ],
    )
    .expect("buffered observation should seed");
}

// Check if entries contain a specific substring
// =============================================================================
// Part A: PostToolUse Handler Tests
// =============================================================================

pub(crate) fn register_agent(dir: &TempDir, session_id: &str, name: &str) -> String {
    let agent_store = open_agent_store(&dir.path().join(".cas")).expect("agent store should open");
    let mut agent = Agent::new(session_id.to_string(), name.to_string());
    agent.cc_session_id = Some(session_id.to_string());
    agent_store.register(&agent).expect("agent should register");
    session_id.to_string()
}

/// Helper to create a task through the local task-store fixture.
pub(crate) fn create_task(dir: &TempDir, title: &str) -> String {
    let task_store =
        open_task_store_local(&dir.path().join(".cas")).expect("task store should open");
    let id = task_store.generate_id().expect("task id should generate");
    task_store
        .add(&Task::new(id.clone(), title.to_string()))
        .expect("task should be created");
    id
}

/// Helper to claim a task through the local lease/task-store fixtures.
pub(crate) fn claim_task(dir: &TempDir, task_id: &str, agent_id: &str) {
    let cas_root = dir.path().join(".cas");
    let agent_store = open_agent_store(&cas_root).expect("agent store should open");
    let result = agent_store
        .try_claim(task_id, agent_id, 600, Some("hooks test fixture"))
        .expect("task should be claimed");
    assert!(
        matches!(result, ClaimResult::Success(_)),
        "task claim failed: {result:?}"
    );

    let task_store = open_task_store_local(&cas_root).expect("task store should open");
    let mut task = task_store.get(task_id).expect("claimed task should exist");
    task.assignee = Some(agent_id.to_string());
    if task.status == TaskStatus::Open {
        task.status = TaskStatus::InProgress;
    }
    task_store
        .update(&task)
        .expect("claimed task should update");
}

/// Mark a fixture task closed through the local task store.
pub(crate) fn close_task(dir: &TempDir, task_id: &str) {
    let task_store =
        open_task_store_local(&dir.path().join(".cas")).expect("task store should open");
    let mut task = task_store.get(task_id).expect("task should exist");
    task.status = TaskStatus::Closed;
    task_store.update(&task).expect("task should close");
}

/// Create an epic fixture through the local task store.
pub(crate) fn create_epic(dir: &TempDir, title: &str) -> String {
    let task_store =
        open_task_store_local(&dir.path().join(".cas")).expect("task store should open");
    let id = task_store.generate_id().expect("epic id should generate");
    let mut epic = Task::new(id.clone(), title.to_string());
    epic.task_type = TaskType::Epic;
    task_store.add(&epic).expect("epic should be created");
    id
}

/// Attach a fixture subtask to an epic through the task-store dependency API.
pub(crate) fn add_epic_subtask(dir: &TempDir, task_id: &str, epic_id: &str) {
    let task_store =
        open_task_store_local(&dir.path().join(".cas")).expect("task store should open");
    task_store
        .add_dependency(&Dependency::new(
            task_id.to_string(),
            epic_id.to_string(),
            DependencyType::ParentChild,
        ))
        .expect("epic dependency should be created");
}

/// Test that Stop is blocked when agent has claimed tasks.
pub(crate) fn add_learning(dir: &TempDir, content: &str) {
    let store = open_store_local(&dir.path().join(".cas")).expect("entry store should open");
    let id = store.generate_id().expect("entry id should generate");
    let mut entry = Entry::new(id, content.to_string());
    entry.entry_type = EntryType::Learning;
    store.add(&entry).expect("learning should be created");
}

/// Create a draft rule through the local rule-store fixture.
pub(crate) fn add_draft_rule(dir: &TempDir, content: &str) {
    let store = open_rule_store_local(&dir.path().join(".cas")).expect("rule store should open");
    let id = store.generate_id().expect("rule id should generate");
    store
        .add(&Rule::new(id, content.to_string()))
        .expect("draft rule should be created");
}

/// Create an entry through the local entry-store fixture.
pub(crate) fn add_entry(dir: &TempDir, content: &str) {
    let store = open_store_local(&dir.path().join(".cas")).expect("entry store should open");
    let id = store.generate_id().expect("entry id should generate");
    store
        .add(&Entry::new(id, content.to_string()))
        .expect("entry should be created");
}

mod exit_and_summary;
mod learning_rule_duplicate;
/// Test that Stop blocks when duplicate_detection is enabled and threshold is exceeded
mod post_tool_and_flow;
