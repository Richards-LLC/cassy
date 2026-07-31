//! Regression tests for verification hook isolation.
//!
//! Verification enforcement is scoped to the affected task's transition.
//! These tests drive the `cas hook PreToolUse` subprocess with a real CAS
//! database and pin that pending verification never blocks unrelated hook
//! traffic, regardless of legacy tool names or factory environment flags.

use assert_cmd::Command;
use rusqlite::{params, Connection};
use tempfile::TempDir;

/// Session ID injected into hook input JSON and set as task assignee.
const C496_SESSION: &str = "c496-0000-test-session-0000-000000000001";

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create a `cas` command rooted in `dir`.
///
/// Factory variables are removed so each test controls its own environment.
fn cas_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("cas").expect("cas binary must be built");
    cmd.current_dir(dir.path());
    // Prevent parent factory-worker env from leaking into test subprocesses.
    cmd.env_remove("CAS_ROOT");
    cmd.env_remove("CAS_AGENT_ROLE");
    cmd.env_remove("CAS_FACTORY_MODE");
    cmd.env_remove("CAS_FACTORY_WORKER_CLI");
    cmd.env_remove("CAS_FACTORY_SUPERVISOR_CLI");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

fn init_cas(dir: &TempDir) {
    cas_cmd(dir).args(["init", "--yes"]).assert().success();
}

/// Insert a minimal task row + set `pending_verification=1` and
/// `assignee=C496_SESSION` directly in SQLite.
///
/// There is no `cas task create` CLI command — tasks are created via MCP only.
/// Direct DB manipulation mirrors what `fixtures/cas_instance.rs` does in
/// other integration tests and avoids starting an MCP server for test setup.
fn create_jailed_task(dir: &TempDir, task_id: &str) {
    let db_path = dir.path().join(".cas/cas.db");
    let conn = Connection::open(&db_path).expect("open cas.db");
    let now = "2026-06-26T00:00:00+00:00";
    conn.execute(
        "INSERT OR REPLACE INTO agents
         (id, name, agent_type, role, status, registered_at, last_heartbeat)
         VALUES (?1, 'c496-parent', 'primary', 'standard', 'active', ?2, ?2)",
        params![C496_SESSION, now],
    )
    .expect("insert authenticated parent agent");
    conn.execute(
        "INSERT INTO tasks (id, title, status, task_type, priority, assignee, \
         pending_verification, created_at, updated_at) \
         VALUES (?1, ?2, 'in_progress', 'task', 0, ?3, 1, ?4, ?4)",
        params!["cas-c496-test-task-001", task_id, C496_SESSION, now],
    )
    .expect("insert jailed task");
    conn.execute(
        "INSERT INTO verification_dispatches
         (id, task_id, requester_agent_id, owner_agent_id, state,
          requested_at, deadline_at)
         VALUES ('vdispatch-c496', 'cas-c496-test-task-001', ?1, ?1, 'pending',
                 ?2, '2099-01-01T00:00:00+00:00')",
        params![C496_SESSION, now],
    )
    .expect("insert exact task-scoped verification dispatch");
}

/// Run `cas hook PreToolUse` with the given JSON input and extra env vars.
/// Returns the full stdout of the hook process.
fn run_hook(dir: &TempDir, input: &serde_json::Value, env: &[(&str, &str)]) -> String {
    run_hook_event(dir, "PreToolUse", input, env)
}

fn run_hook_event(
    dir: &TempDir,
    event: &str,
    input: &serde_json::Value,
    env: &[(&str, &str)],
) -> String {
    let mut cmd = cas_cmd(dir);
    cmd.args(["hook", event]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .write_stdin(serde_json::to_string(input).unwrap())
        .output()
        .expect("hook command must not panic");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn pre_tool_input(tool_name: &str, tool_input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "session_id": C496_SESSION,
        "cwd": "/test",
        "hook_event_name": "PreToolUse",
        "tool_use_id": format!("jail-guard-tool-{}", std::process::id()),
        "tool_name": tool_name,
        "tool_input": tool_input,
    })
}

// ── Test 1 — pending verification does not gate hooks ───────────────────────

#[test]
fn pending_verification_allows_unrelated_and_verifier_hooks() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    create_jailed_task(&dir, "verification hook isolation test");

    let read_out = run_hook(
        &dir,
        &pre_tool_input("Read", serde_json::json!({"file_path": "foo.txt"})),
        &[],
    );
    assert!(
        !read_out.contains("deny"),
        "pending verification must not deny unrelated Read traffic.\n\
         Hook output: {read_out}"
    );

    let agent_input = pre_tool_input(
        "Agent",
        serde_json::json!({
            "subagent_type": "task-verifier",
            "prompt": "verify task cas-c496-test-task-001"
        }),
    );
    let agent_out = run_hook(&dir, &agent_input, &[]);
    assert!(
        !agent_out.contains("deny"),
        "Agent(task-verifier) must not be denied.\n\
         Hook output: {agent_out}"
    );
    let mut failed_agent_input = agent_input;
    failed_agent_input["hook_event_name"] =
        serde_json::Value::String("PostToolUseFailure".to_string());
    run_hook_event(&dir, "PostToolUseFailure", &failed_agent_input, &[]);

    let task_out = run_hook(
        &dir,
        &pre_tool_input(
            "Task",
            serde_json::json!({
                "subagent_type": "task-verifier",
                "prompt": "verify task cas-c496-test-task-001"
            }),
        ),
        &[],
    );
    assert!(
        !task_out.contains("deny"),
        "legacy Task(task-verifier) must not be denied.\n\
         Hook output: {task_out}"
    );

    let conn = Connection::open(dir.path().join(".cas/cas.db")).expect("open cas.db");
    let pending: i64 = conn
        .query_row(
            "SELECT pending_verification FROM tasks WHERE id = 'cas-c496-test-task-001'",
            [],
            |row| row.get(0),
        )
        .expect("read pending verification");
    assert_eq!(
        pending, 1,
        "hook traffic must not implicitly clear task-scoped verification"
    );
}

// ── Test 2 — environment-independent isolation ──────────────────────────────

#[test]
fn pending_verification_does_not_depend_on_factory_environment() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    create_jailed_task(&dir, "verification environment isolation test");

    let read_input = pre_tool_input("Read", serde_json::json!({"file_path": "foo.txt"}));

    let no_env_out = run_hook(&dir, &read_input, &[]);
    assert!(
        !no_env_out.contains("deny"),
        "pending verification must not deny without factory env vars.\n\
         Hook output: {no_env_out}"
    );

    let role_only_out = run_hook(&dir, &read_input, &[("CAS_AGENT_ROLE", "worker")]);
    assert!(
        !role_only_out.contains("deny"),
        "pending verification must not deny with only CAS_AGENT_ROLE.\n\
         Hook output: {role_only_out}"
    );

    let worker_out = run_hook(
        &dir,
        &read_input,
        &[("CAS_AGENT_ROLE", "worker"), ("CAS_FACTORY_MODE", "1")],
    );
    assert!(
        !worker_out.contains("deny"),
        "pending verification must not deny a factory worker.\n\
         Hook output: {worker_out}"
    );
}
