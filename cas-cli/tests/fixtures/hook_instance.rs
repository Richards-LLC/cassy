//! Hook test environment fixture - extends CasInstance with hook-specific setup
//!
//! Provides a properly initialized CAS environment for hook E2E tests,
//! including database operations for jail state and MCP testing.
#![allow(dead_code)]

use super::cas_instance::{CasInstance, new_cas_instance};
use rusqlite::{Connection, params};
use std::env;
use std::path::{Path, PathBuf};

/// Test environment with CAS initialized and hook testing capabilities
///
/// Composes CasInstance with hook-specific methods for:
/// - Jail state management (pending_verification, pending_worktree_merge)
/// - MCP testing (working_epics, task queries)
/// - File creation for test scenarios
pub struct HookTestEnv {
    pub cas: CasInstance,
}

/// Session ID used for hook simulations in tests
pub const HOOK_TEST_SESSION_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

fn cas_bin() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_cas")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cas"))
}

impl HookTestEnv {
    /// Create new environment (runs cas init, creates test files)
    pub fn new() -> Self {
        let cas = new_cas_instance();

        // Create test files commonly needed by hook tests
        std::fs::write(cas.temp_dir.path().join("test.txt"), "Test content for E2E")
            .expect("Failed to write test file");
        std::fs::write(
            cas.temp_dir.path().join("README.md"),
            "# Test Project\nThis is a test.",
        )
        .expect("Failed to write README");

        Self { cas }
    }

    /// Get the project directory path
    pub fn dir(&self) -> &Path {
        self.cas.temp_dir.path()
    }

    /// Get the database path
    pub fn db_path(&self) -> PathBuf {
        self.cas.cas_dir.join("cas.db")
    }

    /// Get the MCP config path
    pub fn mcp_config_path(&self) -> PathBuf {
        self.cas.temp_dir.path().join(".mcp.json")
    }

    /// Run a CAS command in this environment
    pub fn cas(&self, args: &[&str]) -> std::process::Output {
        self.cas
            .cas_cmd()
            .args(args)
            .output()
            .expect("Failed to run cas command")
    }

    /// Create a task and return its ID (starts immediately)
    pub fn create_task(&self, title: &str) -> String {
        self.cas.create_task_with_options(title, None, None, true)
    }

    /// Close a task
    pub fn close_task(&self, task_id: &str) {
        self.cas.close_task(task_id);
    }

    /// Create a file in the test environment
    pub fn create_file(&self, name: &str, content: &str) {
        std::fs::write(self.cas.temp_dir.path().join(name), content)
            .expect("Failed to create file");
    }

    // =========================================================================
    // Jail state methods (direct sqlite3 operations)
    // =========================================================================

    /// Set pending_verification on a task
    pub fn set_pending_verification(&self, task_id: &str, value: bool) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let val = if value { 1 } else { 0 };
        conn.execute(
            "UPDATE tasks SET pending_verification = ?1 WHERE id = ?2",
            params![val, task_id],
        )
        .expect("Failed to set pending_verification");

        // Associate with the hook test session so jails are scoped to this agent.
        if value {
            let _ = conn.execute(
                "UPDATE tasks SET assignee = COALESCE(assignee, ?1) WHERE id = ?2",
                params![HOOK_TEST_SESSION_ID, task_id],
            );
        }
    }

    /// Set pending_worktree_merge on a task
    ///
    /// Uses rusqlite to update the database directly (avoids sqlite3 CLI WAL issues).
    pub fn set_pending_worktree_merge(&self, task_id: &str, value: bool) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let val = if value { 1 } else { 0 };
        conn.execute(
            "UPDATE tasks SET pending_worktree_merge = ?1 WHERE id = ?2",
            params![val, task_id],
        )
        .expect("Failed to set pending_worktree_merge");

        // Associate with the hook test session so jails are scoped to this agent.
        if value {
            let _ = conn.execute(
                "UPDATE tasks SET assignee = COALESCE(assignee, ?1) WHERE id = ?2",
                params![HOOK_TEST_SESSION_ID, task_id],
            );
        }
    }

    /// Enable worktrees in config (required for worktree merge jail to work)
    pub fn enable_worktrees(&self) {
        self.append_config(
            r#"[worktrees]
enabled = true
base_path = "../worktrees"
branch_prefix = "cas/"
"#,
        );
    }

    /// Disable verification in config (useful for MCP integration tests
    /// that close tasks without wanting the verification jail flow)
    pub fn disable_verification(&self) {
        self.append_config(
            r#"[verification]
enabled = false
"#,
        );
    }

    /// Append content to config.toml (creates if needed)
    fn append_config(&self, content: &str) {
        let toml_path = self.cas.cas_dir.join("config.toml");
        let mut existing = std::fs::read_to_string(&toml_path).unwrap_or_default();
        existing.push('\n');
        existing.push_str(content);
        std::fs::write(&toml_path, existing).expect("Failed to write config.toml");
    }

    /// Query a task's jail state from database
    ///
    /// NOTE: Due to SQLite WAL mode on macOS, sqlite3 CLI cannot reliably read
    /// data that CAS CLI has written. This method returns (false, false) if it
    /// can't read the task. The tests using this are ignored.
    pub fn get_task_jail_state(&self, task_id: &str) -> (bool, bool) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let mut stmt = conn
            .prepare("SELECT pending_verification, pending_worktree_merge FROM tasks WHERE id = ?1")
            .expect("Failed to prepare jail state query");
        let mut rows = stmt
            .query(params![task_id])
            .expect("Failed to query jail state");
        if let Some(row) = rows.next().expect("Failed to read jail state row") {
            let pending_verification: i64 = row.get(0).unwrap_or(0);
            let pending_worktree_merge: i64 = row.get(1).unwrap_or(0);
            (pending_verification != 0, pending_worktree_merge != 0)
        } else {
            (false, false)
        }
    }

    /// Count tasks with pending_verification=true
    pub fn count_jailed_tasks(&self) -> usize {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE pending_verification = 1;",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize
    }

    /// Count tasks with pending_worktree_merge=true
    pub fn count_worktree_jailed_tasks(&self) -> usize {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE pending_worktree_merge = 1;",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize
    }

    /// Get total task count
    pub fn count_tasks(&self) -> usize {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        conn.query_row("SELECT COUNT(*) FROM tasks;", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }

    // =========================================================================
    // MCP-specific methods
    // =========================================================================

    /// Query working_epics table directly (via rusqlite to avoid WAL issues)
    pub fn get_working_epics(&self) -> Vec<(String, String)> {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let mut stmt = conn
            .prepare("SELECT agent_id, epic_id FROM working_epics")
            .expect("Failed to prepare working_epics query");
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })
        .expect("Failed to query working_epics")
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Get all tasks from database (via rusqlite to avoid WAL issues)
    pub fn get_tasks(&self) -> Vec<(String, String, String)> {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let mut stmt = conn
            .prepare("SELECT id, title, status FROM tasks")
            .expect("Failed to prepare tasks query");
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("Failed to query tasks")
        .filter_map(|r| r.ok())
        .collect()
    }

    // =========================================================================
    // Hook simulation methods (for true E2E testing)
    // =========================================================================

    /// Run PreToolUse hook with given tool name and input, return (success, output)
    pub fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
    ) -> (bool, String) {
        self.run_pre_tool_use_with_env(tool_name, tool_input, &[])
    }

    /// Run PreToolUse hook with extra environment variables (e.g., simulating
    /// a factory-worker session via CAS_AGENT_ROLE=worker + CAS_FACTORY_MODE=1).
    pub fn run_pre_tool_use_with_env(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
        extra_env: &[(&str, &str)],
    ) -> (bool, String) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let hook_input = serde_json::json!({
            "session_id": HOOK_TEST_SESSION_ID,
            "cwd": self.dir().to_string_lossy(),
            "hook_event_name": "PreToolUse",
            "tool_use_id": format!("hook-test-tool-{}", std::process::id()),
            "tool_name": tool_name,
            "tool_input": tool_input
        });

        let mut cmd = Command::new(cas_bin());
        cmd.args(["hook", "PreToolUse"])
            .current_dir(self.dir())
            .env("CAS_DIR", &self.cas.cas_dir)
            .env_remove("CAS_ROOT")
            .env("CAS_SKIP_FACTORY_TOOLING", "1");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas hook PreToolUse");

        {
            let stdin = child.stdin.as_mut().expect("Failed to get stdin");
            stdin
                .write_all(hook_input.to_string().as_bytes())
                .expect("Failed to write to stdin");
        }

        let output = child
            .wait_with_output()
            .expect("Failed to wait for cas hook");

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Debug output
        if !stderr.is_empty() {
            eprintln!("PreToolUse stderr: {}", stderr);
        }

        (output.status.success(), stdout)
    }

    /// Run PreToolUse for Task(task-verifier) - simulates spawning task-verifier
    pub fn run_pre_tool_use_task_verifier(&self) -> (bool, String) {
        self.run_pre_tool_use(
            "Task",
            serde_json::json!({
                "subagent_type": "task-verifier",
                "prompt": "Verify task completion"
            }),
        )
    }

    /// Run PreToolUse for Agent(task-verifier) - newer Claude Code renamed
    /// the "Task" tool to "Agent". Used to verify the jail accepts both names.
    pub fn run_pre_tool_use_agent_verifier(&self) -> (bool, String) {
        self.run_pre_tool_use(
            "Agent",
            serde_json::json!({
                "subagent_type": "task-verifier",
                "prompt": "Verify task completion"
            }),
        )
    }

    /// Run PreToolUse for a Read operation
    pub fn run_pre_tool_use_read(&self, file_path: &str) -> (bool, String) {
        self.run_pre_tool_use(
            "Read",
            serde_json::json!({
                "file_path": file_path
            }),
        )
    }

    /// Run SubagentStart hook for the task-verifier
    pub fn run_subagent_start_task_verifier(&self) -> (bool, String) {
        self.run_subagent_start("task-verifier")
    }

    /// Run SubagentStart hook with given subagent_type
    pub fn run_subagent_start(&self, subagent_type: &str) -> (bool, String) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let hook_input = serde_json::json!({
            "session_id": HOOK_TEST_SESSION_ID,
            "cwd": self.dir().to_string_lossy(),
            "hook_event_name": "SubagentStart",
            "agent_id": format!("hook-test-child-{}", std::process::id()),
            "agent_type": subagent_type
        });

        let mut child = Command::new(cas_bin())
            .args(["hook", "SubagentStart"])
            .current_dir(self.dir())
            .env("CAS_DIR", &self.cas.cas_dir)
            .env_remove("CAS_ROOT")
            .env("CAS_SKIP_FACTORY_TOOLING", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas hook SubagentStart");

        {
            let stdin = child.stdin.as_mut().expect("Failed to get stdin");
            stdin
                .write_all(hook_input.to_string().as_bytes())
                .expect("Failed to write to stdin");
        }

        let output = child
            .wait_with_output()
            .expect("Failed to wait for cas hook");

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !stderr.is_empty() {
            eprintln!("SubagentStart stderr: {}", stderr);
        }

        (output.status.success(), stdout)
    }

    /// Set task assignee
    pub fn set_task_assignee(&self, task_id: &str, assignee: &str) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        conn.execute(
            "UPDATE tasks SET assignee = ?1 WHERE id = ?2",
            params![assignee, task_id],
        )
        .expect("Failed to set assignee");
    }

    /// Get pending_verification flag for a task
    pub fn get_pending_verification(&self, task_id: &str) -> bool {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        conn.query_row(
            "SELECT pending_verification FROM tasks WHERE id = ?1",
            params![task_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            != 0
    }

    /// Check if hook output indicates denial (blocked by jail)
    pub fn is_hook_denied(&self, hook_output: &str) -> bool {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(hook_output) {
            // Check for permissionDecision: "deny" in hookSpecificOutput
            if let Some(specific) = json.get("hookSpecificOutput") {
                if let Some(decision) = specific.get("permissionDecision") {
                    return decision.as_str() == Some("deny");
                }
            }
        }
        false
    }

    /// Check if verifier unjail marker file exists
    pub fn marker_file_exists(&self) -> bool {
        self.cas.cas_dir.join(".verifier_unjail_marker").exists()
    }

    /// Remove verifier unjail marker file (for cleanup)
    pub fn remove_marker_file(&self) {
        let _ = std::fs::remove_file(self.cas.cas_dir.join(".verifier_unjail_marker"));
    }

    /// Register an agent in the database
    pub fn register_agent(&self, agent_id: &str, name: &str, agent_type: &str) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO agents (id, name, agent_type, status, registered_at, last_heartbeat)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
            params![agent_id, name, agent_type, now, now],
        )
        .expect("Failed to register agent");
    }

    /// Register an agent with a specific role (supervisor, worker, standard)
    pub fn register_agent_with_role(
        &self,
        agent_id: &str,
        name: &str,
        role: &str,
    ) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO agents (id, name, role, status, registered_at, last_heartbeat)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
            params![agent_id, name, role, &now, &now],
        )
        .expect("Failed to register agent with role");
    }

    /// Add working epic entry (agent_id <-> epic_id association)
    pub fn add_working_epic(&self, agent_id: &str, epic_id: &str) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO working_epics (agent_id, epic_id, started_at) VALUES (?1, ?2, ?3)",
            params![agent_id, epic_id, now],
        )
        .expect("Failed to add working epic");
    }

    /// Add a dependency between tasks
    ///
    /// Common dep_type values (kebab-case, matching serde serialization):
    /// - "blocks" - Hard blocker
    /// - "related" - Soft link
    /// - "parent-child" - Epic/subtask (from=subtask, to=epic)
    pub fn add_dependency(&self, from_id: &str, to_id: &str, dep_type: &str) {
        let conn = Connection::open(self.db_path()).expect("Failed to open sqlite db");
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO dependencies (from_id, to_id, dep_type, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![from_id, to_id, dep_type, now],
        )
        .expect("Failed to add dependency");
    }

    /// Run a Stop hook, return (success, output)
    pub fn run_stop_hook(&self) -> (bool, String) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let hook_input = serde_json::json!({
            "session_id": HOOK_TEST_SESSION_ID,
            "cwd": self.dir().to_string_lossy(),
            "hook_event_name": "Stop",
            "stop_hook_active": true
        });

        let mut child = Command::new(cas_bin())
            .args(["hook", "Stop"])
            .current_dir(self.dir())
            .env("CAS_DIR", &self.cas.cas_dir)
            .env_remove("CAS_ROOT")
            .env("CAS_SKIP_FACTORY_TOOLING", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas hook Stop");

        {
            let stdin = child.stdin.as_mut().expect("Failed to get stdin");
            stdin
                .write_all(hook_input.to_string().as_bytes())
                .expect("Failed to write to stdin");
        }

        let output = child
            .wait_with_output()
            .expect("Failed to wait for cas hook");

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !stderr.is_empty() {
            eprintln!("Stop stderr: {}", stderr);
        }

        (output.status.success(), stdout)
    }
}
