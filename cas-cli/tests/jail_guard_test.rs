//! Regression tests for cas-c496: verification-jail deadlock.
//!
//! Two root causes were fixed and this file pins both:
//!
//! **Fix 1 — Task→Agent rename** (`pre_tool.rs:386`):
//! Newer Claude Code renamed the `Task` tool to `Agent`. The jail's unjail
//! check only accepted `tool_name == "Task"`, so `Agent(task-verifier)` was
//! treated as an ordinary blocked call — circular deadlock. Fix: accept both
//! `"Task"` and `"Agent"`.
//!
//! **Fix 2 — CAS_FACTORY_MODE not propagated** (`pty.rs:124`):
//! `is_factory_worker` in `pre_tool.rs` requires BOTH `CAS_AGENT_ROLE=worker`
//! AND `CAS_FACTORY_MODE=1`. The PTY builder was not setting `CAS_FACTORY_MODE`,
//! so the AND was always false and workers fell into the non-exempt jail path.
//! Fix: pty.rs now sets `CAS_FACTORY_MODE=1` alongside `CAS_AGENT_ROLE`.
//!
//! These tests drive the `cas hook PreToolUse` subprocess with a real CAS
//! database (not mocked) so the jail state is accurately reflected.

use assert_cmd::Command;
use rusqlite::{Connection, params};
use tempfile::TempDir;

/// Session ID injected into hook input JSON and set as task assignee.
/// The jail is scoped per-agent (via `assignee` field) so these must match.
const C496_SESSION: &str = "c496-0000-test-session-0000-000000000001";

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create a `cas` command rooted in `dir`.
///
/// Critically, this function *removes* `CAS_AGENT_ROLE`, `CAS_FACTORY_MODE`,
/// `CAS_FACTORY_WORKER_CLI`, and `CAS_FACTORY_SUPERVISOR_CLI` from the
/// subprocess environment even when the test runner is itself a factory
/// worker/supervisor. Without this, the subprocess would see
/// `is_factory_worker=true` and bypass the jail, making the deny assertions
/// silent no-ops.
///
/// `CAS_FACTORY_WORKER_CLI` in particular (cas-5d96): `pre_tool.rs` gates the
/// entire verification-jail block on
/// `worker_harness_from_env().capabilities().supports_subagents` — true for
/// Claude, false for Codex (one-shot, no subagent spawn). When this test
/// runs inside a live factory session where the ambient env has
/// `CAS_FACTORY_WORKER_CLI=codex` (e.g. Codex is the configured default
/// worker harness), that value leaked into the subprocess and silently
/// skipped the whole jail check, making the hook return an empty `{}`
/// decision instead of `deny` — this is exactly the "hook returns empty
/// decision" precondition failure reported in cas-5d96. Root cause was test
/// hermeticity, not a jail-logic regression: removing the var here restores
/// the deny.
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
    cas_cmd(dir)
        .args(["init", "--yes"])
        .assert()
        .success();
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
}

/// Re-set `pending_verification=1` (used after the Agent unjail call clears it).
fn rejail_task(dir: &TempDir) {
    let db_path = dir.path().join(".cas/cas.db");
    let conn = Connection::open(&db_path).expect("open cas.db");
    conn.execute(
        "UPDATE tasks SET pending_verification = 1 WHERE id = 'cas-c496-test-task-001'",
        [],
    )
    .expect("rejail task");
}

/// Run `cas hook PreToolUse` with the given JSON input and extra env vars.
/// Returns the full stdout of the hook process.
fn run_hook(dir: &TempDir, input: &serde_json::Value, env: &[(&str, &str)]) -> String {
    let mut cmd = cas_cmd(dir);
    cmd.args(["hook", "PreToolUse"]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .write_stdin(serde_json::to_string(input).unwrap())
        .output()
        .expect("hook PreToolUse must not panic");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn pre_tool_input(tool_name: &str, tool_input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "session_id": C496_SESSION,
        "cwd": "/test",
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
    })
}

// ── Test 1 — Task→Agent rename (cas-c496 root cause 1) ───────────────────────

/// **Regression for cas-c496 fix 1**: newer Claude Code renamed the `Task` tool
/// to `Agent`. The jail's unjail check at `pre_tool.rs:386` now accepts both
/// `"Task"` and `"Agent"`. Before the fix, `Agent(task-verifier)` was blocked
/// by the same jail it was supposed to clear — circular deadlock.
///
/// Asserts:
/// 1. A plain `Read` is denied while the jail is active (precondition).
/// 2. `Agent(subagent_type="task-verifier")` is NOT denied (the fix).
/// 3. Legacy `Task(subagent_type="task-verifier")` is also NOT denied.
#[test]
fn c496_agent_tool_task_verifier_bypasses_jail() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    // Insert a task directly into the DB with pending_verification=1 and
    // assignee=C496_SESSION so the jail fires for this session.
    create_jailed_task(&dir, "c496 agent-rename jail test");

    // 1. Precondition: plain Read is denied while jail is active.
    let read_out = run_hook(
        &dir,
        &pre_tool_input("Read", serde_json::json!({"file_path": "foo.txt"})),
        &[],
    );
    assert!(
        read_out.contains("deny"),
        "precondition failed — Read must be denied while jail is active.\n\
         Hook output: {read_out}"
    );

    // 2. Agent(task-verifier) must bypass the jail (the Agent rename fix).
    //    This call also clears pending_verification for the task.
    let agent_out = run_hook(
        &dir,
        &pre_tool_input(
            "Agent",
            serde_json::json!({
                "subagent_type": "task-verifier",
                "prompt": "verify task cas-c496-test-task-001"
            }),
        ),
        &[],
    );
    assert!(
        !agent_out.contains("deny"),
        "cas-c496 fix 1: Agent(task-verifier) must NOT be denied.\n\
         pre_tool.rs must accept 'Agent' tool name alongside legacy 'Task'.\n\
         Hook output: {agent_out}"
    );

    // 3. Re-jail and verify the legacy Task name still works too.
    rejail_task(&dir);
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
        "legacy Task(task-verifier) form must also NOT be denied.\n\
         Hook output: {task_out}"
    );
}

// ── Test 2 — Factory worker exemption (cas-c496 root cause 2) ────────────────

/// **Regression for cas-c496 fix 2**: factory workers must be fully exempt
/// from the verification jail. The exemption in `pre_tool.rs` requires BOTH
/// `CAS_AGENT_ROLE=worker` AND `CAS_FACTORY_MODE=1`. Before the fix, the PTY
/// builder did not propagate `CAS_FACTORY_MODE`, so the AND was always false
/// and workers got jailed even though the flag was set.
///
/// Asserts:
/// 1. No factory env vars → jailed (precondition).
/// 2. `CAS_AGENT_ROLE=worker` alone → still jailed (both vars required).
/// 3. `CAS_AGENT_ROLE=worker` + `CAS_FACTORY_MODE=1` → exempt (the fix).
#[test]
fn c496_factory_worker_exempt_from_verification_jail() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    create_jailed_task(&dir, "c496 factory-worker exemption test");

    let read_input = pre_tool_input("Read", serde_json::json!({"file_path": "foo.txt"}));

    // 1. No factory env vars → jailed (baseline).
    let no_env_out = run_hook(&dir, &read_input, &[]);
    assert!(
        no_env_out.contains("deny"),
        "precondition failed — Read must be denied with no factory env vars.\n\
         Hook output: {no_env_out}"
    );

    // 2. CAS_AGENT_ROLE=worker alone → still jailed.
    //    Both env vars are required; one is not enough.
    let role_only_out = run_hook(&dir, &read_input, &[("CAS_AGENT_ROLE", "worker")]);
    assert!(
        role_only_out.contains("deny"),
        "CAS_AGENT_ROLE=worker alone must NOT exempt from jail \
         (CAS_FACTORY_MODE=1 is also required).\n\
         Hook output: {role_only_out}"
    );

    // 3. Both CAS_AGENT_ROLE=worker AND CAS_FACTORY_MODE=1 → fully exempt.
    let worker_out = run_hook(
        &dir,
        &read_input,
        &[("CAS_AGENT_ROLE", "worker"), ("CAS_FACTORY_MODE", "1")],
    );
    assert!(
        !worker_out.contains("deny"),
        "cas-c496 fix 2: factory worker with CAS_AGENT_ROLE=worker + CAS_FACTORY_MODE=1 \
         must NOT be denied by the verification jail.\n\
         Hook output: {worker_out}"
    );
}
