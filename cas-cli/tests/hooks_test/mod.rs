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
    // Hook integration fixtures exercise end-user Stop semantics. Keep ambient
    // factory identity out of every subprocess so maintenance queueing and
    // attribution use the payload's session ID deterministically.
    for variable in [
        "CAS_AGENT_ROLE",
        "CAS_FACTORY_MODE",
        "CAS_SESSION_ID",
        "CAS_AGENT_NAME",
        "CAS_FACTORY_SESSION",
    ] {
        cmd.env_remove(variable);
    }
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    let test_bin = dir.path().join(".test-bin");
    if test_bin.join("codex").exists() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        cmd.env(
            "PATH",
            std::env::join_paths(std::iter::once(test_bin).chain(std::env::split_paths(&path)))
                .unwrap(),
        );
    }
    cmd
}

/// Replace the detached light-lane process with a local prompt recorder.
pub(crate) fn install_fake_maintenance_runner(dir: &TempDir) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.path().join(".test-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("codex");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Check both the immediate Stop response and the detached job's recorded prompt.
pub(crate) fn assert_maintenance_queued(
    dir: &TempDir,
    session_id: &str,
    name: &str,
    output: &str,
    prompt_fragments: &[&str],
) {
    assert_stop_allowed(output, None);
    let job_dir = dir.path().join(".cas/maintenance").join(session_id);
    assert!(
        job_dir.join(format!("{name}.queued")).exists(),
        "{name} queue marker missing"
    );
    let log = job_dir.join(format!("{name}.log"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut last_content = String::new();
    loop {
        if let Ok(content) = std::fs::read_to_string(&log) {
            if prompt_fragments
                .iter()
                .all(|fragment| content.contains(fragment))
            {
                break;
            }
            last_content = content;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{name} complete prompt was not recorded at {}: {last_content}",
            log.display(),
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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
    // Update the existing TOML table through the config writer so fixtures do
    // not silently create duplicate tables.
    set_config_value(dir, "dev.dev_mode", "true");
}

/// Update one existing config key through Cassy's config writer.
pub(crate) fn set_config_value(dir: &TempDir, key: &str, value: &str) {
    let config_path = dir.path().join(".cas/config.toml");
    let raw = std::fs::read_to_string(&config_path).expect("fixture config should exist");
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .expect("fixture config should be valid TOML");
    let mut parts = key.split('.').peekable();
    let leaf = parts.next_back().expect("config key should not be empty");
    let mut item = document.as_item_mut();
    for part in parts {
        if item.get(part).is_none() {
            item[part] = toml_edit::table();
        }
        item = item
            .get_mut(part)
            .unwrap_or_else(|| panic!("config table should exist for {part}"));
    }
    let value = value
        .parse::<toml_edit::Value>()
        .unwrap_or_else(|error| panic!("config value should be valid TOML: {error}"));
    item[leaf] = toml_edit::value(value);

    let updated = document.to_string();
    toml::from_str::<cas::config::Config>(&updated)
        .unwrap_or_else(|error| panic!("fixture config should remain valid after {key}: {error}"));
    std::fs::write(config_path, updated).expect("fixture config should be writable");
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

pub(crate) fn maintenance_stop_input(dir: &TempDir, session_id: &str) -> serde_json::Value {
    let mut input = stop_input(session_id);
    input["cwd"] = serde_json::json!(dir.path());
    input["transcript_path"] = serde_json::json!(dir.path().join("transcript.jsonl"));
    input
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
/// enabled. This fixture queries the bounded buffer table directly so tests
/// can inspect persisted rows across sessions without changing their identity.
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

/// Seed a buffer row under a known session ID for Stop component coverage.
///
/// Real subprocess E2E tests use PostToolUse to create rows. This helper
/// intentionally bypasses that caller path so the Stop synthesis/clear
/// component can be tested with a deterministic SQLite fixture.
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

/// Parse a Stop hook response using the serialized HookOutput schema.
pub(crate) fn parse_stop_output(output: &str) -> serde_json::Value {
    serde_json::from_str(output).unwrap_or_else(|error| {
        panic!("Stop hook output must be valid JSON: {error}; got {output:?}")
    })
}

/// Assert the serialized Stop response blocks and includes the expected
/// reason/context. `decision: "block"` and `reason` are the current
/// HookOutput schema; Stop context is carried in `systemMessage`.
pub(crate) fn assert_stop_blocked(
    output: &str,
    reason_fragments: &[&str],
    context_fragments: Option<&[&str]>,
) {
    let json = parse_stop_output(output);
    assert_eq!(
        json.get("decision").and_then(|value| value.as_str()),
        Some("block"),
        "Stop should serialize decision=block; got {json}"
    );

    let reason = json
        .get("reason")
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("blocked Stop must serialize reason; got {json}"));
    assert!(
        reason_fragments
            .iter()
            .any(|fragment| reason.contains(fragment)),
        "Stop reason {reason:?} should contain one of {reason_fragments:?}"
    );

    if let Some(context_fragments) = context_fragments {
        let context = json
            .get("systemMessage")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| panic!("blocked Stop should serialize systemMessage; got {json}"));
        assert!(
            context_fragments
                .iter()
                .any(|fragment| context.contains(fragment)),
            "Stop context {context:?} should contain one of {context_fragments:?}"
        );
    }
}

/// Assert an allowed Stop response. The HookOutput schema represents allow by
/// omitting `decision`, not by a `continue_session` or `continue_session=true`
/// field.
pub(crate) fn assert_stop_allowed(output: &str, forbidden_reason: Option<&str>) {
    let json = parse_stop_output(output);
    assert!(
        json.get("decision").is_none(),
        "allowed Stop must omit decision; got {json}"
    );
    assert!(
        json.get("reason").is_none(),
        "allowed Stop must omit reason; got {json}"
    );
    if let Some(fragment) = forbidden_reason {
        let context = json
            .get("systemMessage")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(
            !context.contains(fragment),
            "allowed Stop context must not contain {fragment:?}; got {context:?}"
        );
    }
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
