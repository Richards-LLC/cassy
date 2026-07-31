//! MCP Protocol Integration Tests
//!
//! Tests the actual MCP server protocol by spawning the server and
//! communicating via JSON-RPC over stdio.
//!
//! These tests verify that:
//! 1. The server responds correctly to MCP protocol messages
//! 2. Tool calls work end-to-end through the protocol
//! 3. Error handling follows MCP spec

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

mod support;
use support::{CasSandbox, assert_command_is_sandboxed};

// ============================================================================
// MCP Protocol Types
// ============================================================================

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

impl JsonRpcRequest {
    fn new(id: u64, method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<Value>,
}

// ============================================================================
// Test Helpers
// ============================================================================

/// Helper to communicate with MCP server
struct McpTestClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl McpTestClient {
    /// Spawn MCP server (7 meta-tools)
    fn spawn(sandbox: &CasSandbox) -> Self {
        Self::spawn_command(sandbox.command())
    }

    fn spawn_command(mut cmd: Command) -> Self {
        cmd.arg("serve");
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to spawn cas serve");

        let stdin = child.stdin.take().expect("Failed to get stdin");
        let stdout = BufReader::new(child.stdout.take().expect("Failed to get stdout"));

        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn spawn_capturing_stderr(sandbox: &CasSandbox) -> Self {
        let mut cmd = sandbox.command();
        cmd.arg("serve");
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas serve");

        let stdin = child.stdin.take().expect("Failed to get stdin");
        let stdout = BufReader::new(child.stdout.take().expect("Failed to get stdout"));

        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn stop_and_read_stderr(&mut self) -> String {
        let _ = self.child.kill();
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            pipe.read_to_string(&mut stderr)
                .expect("read cas serve stderr");
        }
        let _ = self.child.wait();
        stderr
    }

    fn send_request(&mut self, method: &str, params: Option<Value>) -> JsonRpcResponse {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest::new(id, method, params);
        let request_json = serde_json::to_string(&request).expect("Failed to serialize request");

        // Send request
        writeln!(self.stdin, "{request_json}").expect("Failed to write request");
        self.stdin.flush().expect("Failed to flush");

        // Read response (skip notifications with no id)
        loop {
            let mut response_line = String::new();
            self.stdout
                .read_line(&mut response_line)
                .expect("Failed to read response");

            let response: JsonRpcResponse =
                serde_json::from_str(&response_line).expect("Failed to parse response");
            assert_eq!(response.jsonrpc, "2.0", "Invalid JSON-RPC version");

            match response.id {
                Some(resp_id) => {
                    assert_eq!(resp_id, id, "Response ID should match request");
                    return response;
                }
                None => {
                    // Notification or event; continue reading.
                    continue;
                }
            }
        }
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> JsonRpcResponse {
        self.send_request(
            "tools/call",
            Some(json!({
                "name": name,
                "arguments": arguments
            })),
        )
    }

    fn initialize(&mut self) -> JsonRpcResponse {
        let response = self.send_request(
            "initialize",
            Some(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "cas-test-client",
                    "version": "1.0.0"
                }
            })),
        );

        // Send the required 'initialized' notification after successful initialize
        if response.error.is_none() {
            self.send_notification("notifications/initialized", None);
        }

        response
    }

    /// Send a notification (no response expected)
    fn send_notification(&mut self, method: &str, params: Option<Value>) {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(json!({}))
        });
        let notification_json = serde_json::to_string(&notification).expect("Failed to serialize");
        writeln!(self.stdin, "{notification_json}").expect("Failed to write notification");
        self.stdin.flush().expect("Failed to flush");
    }

    fn list_tools(&mut self) -> JsonRpcResponse {
        self.send_request("tools/list", None)
    }
}

impl Drop for McpTestClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

// ============================================================================
// Protocol Tests
// ============================================================================

#[test]
fn test_mcp_initialize() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    let response = client.initialize();

    assert!(response.error.is_none(), "Initialize should succeed");
    assert!(response.result.is_some(), "Should have result");

    let result = response.result.unwrap();
    assert!(result.get("protocolVersion").is_some());
    assert!(result.get("serverInfo").is_some());
    assert!(result.get("capabilities").is_some());

    let server_info = result.get("serverInfo").unwrap();
    assert_eq!(server_info.get("name").unwrap().as_str().unwrap(), "cas");
}

#[test]
fn test_mcp_list_tools() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    let response = client.list_tools();

    assert!(response.error.is_none(), "List tools should succeed");
    assert!(response.result.is_some());

    let result = response.result.unwrap();
    let tools = result.get("tools").and_then(|t| t.as_array());
    assert!(tools.is_some(), "Should have tools array");

    let tools = tools.unwrap();
    assert!(!tools.is_empty(), "Should have at least one tool");

    // Check that expected tools exist
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();

    // We should have the 8 meta-tools
    assert!(
        tool_names.contains(&"memory") && tool_names.contains(&"task"),
        "Should have meta-tools (memory, task): {tool_names:?}"
    );
}

#[test]
fn test_mcp_tool_call_remember() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Call the memory tool with remember action (meta-tool API)
    let response = client.call_tool(
        "memory",
        json!({
            "action": "remember",
            "content": "Test memory from MCP protocol test",
            "entry_type": "learning",
            "tags": "test,mcp"
        }),
    );

    assert!(
        response.error.is_none(),
        "Tool call should succeed: {:?}",
        response.error
    );
    assert!(response.result.is_some());

    let result = response.result.unwrap();
    // MCP tool results have a "content" array
    let content = result.get("content").and_then(|c| c.as_array());
    assert!(content.is_some(), "Should have content array");

    let content = content.unwrap();
    assert!(!content.is_empty(), "Content should not be empty");

    // First content item should have text
    let text = content[0].get("text").and_then(|t| t.as_str());
    assert!(text.is_some(), "Should have text in content");
}

#[test]
fn test_mcp_tool_call_task_create() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Create a task (meta-tool API)
    let response = client.call_tool(
        "task",
        json!({
            "action": "create",
            "title": "MCP Protocol Test Task",
            "priority": 2,
            "task_type": "task"
        }),
    );

    assert!(
        response.error.is_none(),
        "Task create should succeed: {:?}",
        response.error
    );

    // Verify task was created by listing tasks
    let list_response = client.call_tool(
        "task",
        json!({
            "action": "list"
        }),
    );

    assert!(list_response.error.is_none());
    let result = list_response.result.unwrap();

    // Content should mention the task we created
    let empty_vec = vec![];
    let content = result
        .get("content")
        .and_then(|c| c.as_array())
        .unwrap_or(&empty_vec);

    let content_text: String = content
        .iter()
        .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
        .collect();

    assert!(
        content_text.contains("MCP Protocol Test Task") || content_text.contains("cas-"),
        "Task list should include our task"
    );
}

#[test]
fn malformed_verification_issues_never_cross_protocol_logs_or_sqlite() {
    let sandbox = CasSandbox::new();
    let raw_capability = "vcap-11111111111111111111111111111111.22222222222222222222222222222222";
    let raw_path = "/home/verifier/private-proof.json";
    let raw_pat = "ghp_verifier-authored-secret";
    let malformed_issues =
        format!(r#"[{{"file":"{raw_path}","severity":"{raw_capability}","problem":"{raw_pat}""#);
    let mut client = McpTestClient::spawn_capturing_stderr(&sandbox);
    client.initialize();

    let response = client.call_tool(
        "verification",
        json!({
            "action": "add",
            "task_id": "cas-missing",
            "status": "approved",
            "summary": "safe summary",
            "issues": malformed_issues.clone(),
        }),
    );
    let response_text = format!("{response:?}");
    assert!(
        response_text.contains("Invalid verification issues")
            && response_text.contains("input omitted"),
        "public MCP rejection must be static and actionable: {response_text}"
    );
    let stderr = client.stop_and_read_stderr();

    for (surface, payload) in [
        ("response", response_text.as_str()),
        ("stderr", stderr.as_str()),
    ] {
        for unsafe_value in [raw_capability, raw_path, raw_pat, malformed_issues.as_str()] {
            assert!(
                !payload.contains(unsafe_value),
                "{surface} leaked malformed verifier input: {unsafe_value:?}"
            );
        }
    }
    assert!(
        !stderr.contains("Input was:"),
        "malformed verification diagnostics must not log caller input: {stderr}"
    );

    for suffix in ["", "-wal", "-shm"] {
        let path = sandbox.cas_root().join(format!("cas.db{suffix}"));
        if let Ok(bytes) = std::fs::read(path) {
            for unsafe_value in [raw_capability, raw_path, raw_pat] {
                assert!(
                    !bytes
                        .windows(unsafe_value.len())
                        .any(|window| window == unsafe_value.as_bytes()),
                    "SQLite {suffix} leaked rejected verifier input: {unsafe_value:?}"
                );
            }
        }
    }
}

#[test]
fn test_mcp_task_create_cannot_escape_cas_sandbox() {
    let live_store_sentinel = CasSandbox::new();
    let sandbox = CasSandbox::new();
    let live_count_before = live_store_sentinel.task_count();

    // Model a test process launched by a factory worker: both store resolvers
    // initially point at a live project, along with an arbitrary future CAS_*
    // variable that a fixed scrub list would miss.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cas"));
    cmd.env("CAS_ROOT", live_store_sentinel.cas_root())
        .env("CAS_DIR", live_store_sentinel.cas_root())
        .env("CLAUDE_PROJECT_DIR", live_store_sentinel.path())
        .env("CAS_FUTURE_TEST_LEAK_VECTOR", "must-be-removed");
    sandbox.configure_command(&mut cmd);
    assert_command_is_sandboxed(&cmd, &sandbox);
    assert!(
        cmd.get_envs()
            .all(|(key, value)| key != "CAS_FUTURE_TEST_LEAK_VECTOR" || value.is_none()),
        "sandbox must remove arbitrary inherited CAS_* variables"
    );

    let mut client = McpTestClient::spawn_command(cmd);
    client.initialize();
    let response = client.call_tool(
        "task",
        json!({
            "action": "create",
            "title": "Hermetic sandbox escape sentinel"
        }),
    );
    assert!(
        response.error.is_none(),
        "sandbox task create should succeed: {:?}",
        response.error
    );
    drop(client);

    assert_eq!(
        live_store_sentinel.task_count(),
        live_count_before,
        "task creation escaped into the live-store sentinel"
    );
    assert_eq!(
        sandbox.task_count(),
        1,
        "task must be written into the sandbox store"
    );
}

#[test]
fn test_sandbox_init_and_serve_never_touch_inherited_host_known_repos() {
    let inherited = tempfile::tempdir().expect("create inherited host sentinel");
    let host_home = inherited.path().join("home");
    let host_xdg = inherited.path().join("xdg-config");
    let host_cas = host_home.join(".cas");
    std::fs::create_dir_all(&host_cas).unwrap();
    std::fs::create_dir_all(&host_xdg).unwrap();
    let host_config_sentinel = host_xdg.join("sentinel.toml");
    std::fs::write(&host_config_sentinel, b"host-config-must-not-change").unwrap();
    let host_db = host_cas.join("cas.db");
    {
        let conn = rusqlite::Connection::open(&host_db).unwrap();
        conn.execute_batch(
            "CREATE TABLE known_repos (
                path TEXT PRIMARY KEY,
                first_seen_at TEXT NOT NULL,
                last_touched_at TEXT NOT NULL,
                touch_count INTEGER NOT NULL DEFAULT 1
             );
             INSERT INTO known_repos
                (path, first_seen_at, last_touched_at, touch_count)
             VALUES
                ('/sentinel/host/repo', 'HOST-BEFORE', 'HOST-BEFORE', 41);",
        )
        .unwrap();
    }
    let host_db_before = std::fs::read(&host_db).unwrap();
    let host_config_before = std::fs::read(&host_config_sentinel).unwrap();

    // The initializer starts with explicit inherited host paths. CasSandbox
    // must replace them before launching the normal, non-cfg(test) `cas init`.
    let sandbox = CasSandbox::new_with_host_environment(&host_home, &host_xdg);
    assert_eq!(
        std::fs::read(&host_db).unwrap(),
        host_db_before,
        "sandbox init mutated the inherited host known-repo database"
    );
    assert_eq!(
        std::fs::read(&host_config_sentinel).unwrap(),
        host_config_before,
        "sandbox init mutated inherited XDG config state"
    );

    let sandbox_host_db = sandbox.home_dir().join(".cas/cas.db");
    assert!(
        sandbox_host_db.is_file(),
        "sandbox init must create its host registry under sandbox HOME"
    );
    let (registered_path, touches_after_init): (String, i64) = {
        let conn = rusqlite::Connection::open(&sandbox_host_db).unwrap();
        conn.query_row("SELECT path, touch_count FROM known_repos", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
    };
    assert_eq!(
        std::path::PathBuf::from(registered_path)
            .canonicalize()
            .unwrap(),
        sandbox.path().canonicalize().unwrap(),
        "sandbox init must register only the sandbox project"
    );
    assert_eq!(touches_after_init, 1);

    // Prove the same overwrite is applied to the serve path, even when a
    // caller seeds hostile HOME/XDG values on the command itself.
    let mut serve = Command::new(env!("CARGO_BIN_EXE_cas"));
    serve
        .env("HOME", &host_home)
        .env("XDG_CONFIG_HOME", &host_xdg);
    sandbox.configure_command(&mut serve);
    assert_command_is_sandboxed(&serve, &sandbox);
    let mut client = McpTestClient::spawn_command(serve);
    let initialized = client.initialize();
    assert!(
        initialized.error.is_none(),
        "sandbox serve initialization failed: {:?}",
        initialized.error
    );
    drop(client);

    let touches_after_serve: i64 = rusqlite::Connection::open(&sandbox_host_db)
        .unwrap()
        .query_row(
            "SELECT touch_count FROM known_repos WHERE path = ?1",
            [sandbox.path().canonicalize().unwrap().to_string_lossy()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        touches_after_serve >= 2,
        "sandbox serve must register/touch the sandbox-local repo"
    );
    assert_eq!(
        std::fs::read(&host_db).unwrap(),
        host_db_before,
        "sandbox serve mutated the inherited host known-repo database"
    );
    assert_eq!(
        std::fs::read(&host_config_sentinel).unwrap(),
        host_config_before,
        "sandbox serve mutated inherited XDG config state"
    );
    let host_row: (String, String, i64) = rusqlite::Connection::open(&host_db)
        .unwrap()
        .query_row(
            "SELECT first_seen_at, last_touched_at, touch_count
             FROM known_repos WHERE path = '/sentinel/host/repo'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(host_row, ("HOST-BEFORE".into(), "HOST-BEFORE".into(), 41));
}

#[test]
fn test_mcp_tool_call_search() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Add some content first (meta-tool API)
    client.call_tool(
        "memory",
        json!({
            "action": "remember",
            "content": "Rust programming language with ownership and borrowing",
            "tags": "rust,programming"
        }),
    );

    // Search for it (meta-tool API)
    let response = client.call_tool(
        "search",
        json!({
            "action": "search",
            "query": "rust ownership"
        }),
    );

    assert!(response.error.is_none(), "Search should succeed");
    assert!(response.result.is_some());
}

#[test]
fn test_mcp_tool_call_invalid_arguments() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Call with missing required argument (meta-tool API)
    let response = client.call_tool(
        "task",
        json!({
            "action": "create"
            // Missing required "title"
        }),
    );

    // Should get an error response
    assert!(
        response.error.is_some() || {
            // Some implementations return success with error in content
            response
                .result
                .as_ref()
                .and_then(|r| r.get("isError"))
                .and_then(|e| e.as_bool())
                .unwrap_or(false)
        },
        "Should indicate error for missing required field"
    );
}

#[test]
fn test_mcp_unknown_tool() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    let response = client.call_tool("nonexistent_tool", json!({}));

    // Should return an error
    assert!(
        response.error.is_some(),
        "Unknown tool should return error: {response:?}"
    );
}

#[test]
fn test_mcp_rule_lifecycle() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Create a rule (meta-tool API)
    let create_response = client.call_tool(
        "rule",
        json!({
            "action": "create",
            "content": "Always use descriptive variable names in tests",
            "tags": "testing,style"
        }),
    );

    assert!(
        create_response.error.is_none(),
        "Rule create should succeed: {:?}",
        create_response.error
    );

    // List all rules (meta-tool API)
    let list_all_response = client.call_tool(
        "rule",
        json!({
            "action": "list_all"
        }),
    );
    assert!(list_all_response.error.is_none());
}

#[test]
fn test_mcp_context() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Add some data (meta-tool API)
    client.call_tool(
        "memory",
        json!({
            "action": "remember",
            "content": "Context test memory entry"
        }),
    );

    client.call_tool(
        "task",
        json!({
            "action": "create",
            "title": "Context test task"
        }),
    );

    // Get context (meta-tool API)
    let response = client.call_tool(
        "search",
        json!({
            "action": "context"
        }),
    );

    assert!(response.error.is_none(), "Context should succeed");
    assert!(response.result.is_some());

    let result = response.result.unwrap();
    let empty_vec = vec![];
    let content = result
        .get("content")
        .and_then(|c| c.as_array())
        .unwrap_or(&empty_vec);

    // Context should have content
    assert!(!content.is_empty(), "Context should have content");
}

#[test]
fn test_mcp_doctor() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Doctor is accessed via system with action: doctor
    let response = client.call_tool(
        "system",
        json!({
            "action": "doctor"
        }),
    );

    assert!(response.error.is_none(), "Doctor should succeed");
    assert!(response.result.is_some());
}

// ============================================================================
// Consolidated Tools Tests
// ============================================================================

#[test]
fn test_mcp_consolidated_memory_tool() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Test memory with "remember" action
    let response = client.call_tool(
        "memory",
        json!({
            "action": "remember",
            "content": "Consolidated memory test entry",
            "entry_type": "learning"
        }),
    );

    assert!(
        response.error.is_none(),
        "Memory remember should succeed: {:?}",
        response.error
    );
}

#[test]
fn test_mcp_consolidated_task_tool() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // Test task with "create" action
    let response = client.call_tool(
        "task",
        json!({
            "action": "create",
            "title": "Consolidated task test"
        }),
    );

    assert!(
        response.error.is_none(),
        "Task create should succeed: {:?}",
        response.error
    );
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_mcp_invalid_json_rpc() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    // Must initialize first per MCP protocol
    client.initialize();

    // Send request with invalid method
    let response = client.send_request("invalid/method", None);

    // Should get method not found error
    assert!(response.error.is_some(), "Invalid method should error");
    if let Some(error) = response.error {
        assert_eq!(error.code, -32601, "Should be method not found error");
        assert!(
            !error.message.is_empty(),
            "Error message should be populated"
        );
        let _ = error.data.as_ref();
    }
}

#[test]
fn test_mcp_resources_list_changed_capability() {
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    let response = client.initialize();

    assert!(response.error.is_none(), "Initialize should succeed");
    let result = response.result.unwrap();

    // Check that resources.listChanged capability is advertised
    let capabilities = result
        .get("capabilities")
        .expect("should have capabilities");
    let resources = capabilities.get("resources");

    assert!(
        resources.is_some(),
        "Should have resources capability: {capabilities:?}"
    );

    let resources = resources.unwrap();
    let list_changed = resources.get("listChanged").and_then(|v| v.as_bool());

    assert_eq!(
        list_changed,
        Some(true),
        "resources.listChanged should be true: {resources:?}"
    );
}

#[test]
fn test_mcp_mutation_with_notifications() {
    // This test verifies that mutations work correctly even with notification code path
    // (notifications are fire-and-forget, so we can't directly verify they were sent,
    // but we verify the mutation succeeds and doesn't crash)
    let sandbox = CasSandbox::new();

    let mut client = McpTestClient::spawn(&sandbox);
    client.initialize();

    // List resources first (this captures the peer for notifications)
    let list_response = client.send_request("resources/list", None);
    assert!(
        list_response.error.is_none(),
        "List resources should succeed"
    );

    // Create a memory (triggers notification)
    let response = client.call_tool(
        "memory",
        json!({
            "action": "remember",
            "content": "Test entry for notification test"
        }),
    );

    assert!(
        response.error.is_none(),
        "Memory create should succeed: {:?}",
        response.error
    );

    // Create a task (triggers notification)
    let response = client.call_tool(
        "task",
        json!({
            "action": "create",
            "title": "Test task for notification test"
        }),
    );

    assert!(
        response.error.is_none(),
        "Task create should succeed: {:?}",
        response.error
    );

    // Verify the resources were created by listing them
    let list_response = client.send_request("resources/list", None);
    assert!(list_response.error.is_none());

    let result = list_response.result.unwrap();
    let resources = result
        .get("resources")
        .and_then(|r| r.as_array())
        .expect("Should have resources array");

    // Should have at least one resource after mutations
    // (exact count may vary due to test parallelism)
    assert!(
        !resources.is_empty(),
        "Should have resources after mutations: {resources:?}"
    );
}

// ============================================================================
// cas-5c05: Startup must fail loud when stores cannot be opened
//
// Regression coverage for the "silent zero-tool mode" failure: a corrupt or
// unreadable cas.db must cause `cas serve` to exit non-zero with a diagnostic
// on stderr, NOT silently start a server that responds to tools/list with an
// empty registry (or hangs the MCP handshake until the client gives up).
// ============================================================================

#[test]
#[cfg(unix)]
fn test_serve_fails_fast_on_unreadable_cas_db() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = CasSandbox::new();
    let db_path = sandbox.cas_root().join("cas.db");

    // Restore permissions on exit so TempDir cleanup can proceed even if the
    // assertions below panic. The guard is created BEFORE the chmod so any
    // panic between chmod-and-spawn still triggers the restore on unwind
    // (review A3).
    struct RestorePerms(std::path::PathBuf);
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            if let Ok(meta) = std::fs::metadata(&self.0) {
                let mut p = meta.permissions();
                p.set_mode(0o644);
                let _ = std::fs::set_permissions(&self.0, p);
            }
        }
    }
    let _guard = RestorePerms(db_path.clone());

    // Strip every permission from cas.db. The next `Connection::open` from
    // `cas serve` will fail with EACCES. The previous code path swallowed this
    // error with `let _ = core.open_store()` and continued to "Starting MCP
    // server (13 tools)" — exactly the silent failure mode this test guards
    // against. (We use chmod-0000 rather than corrupt-bytes because SQLite's
    // WAL mode will happily rewrite a garbage header on first open.)
    let mut perms = std::fs::metadata(&db_path).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&db_path, perms).expect("chmod 000 cas.db");

    // Spawn `cas serve` and wait for it to fail. Send NOTHING on stdin —
    // a healthy server would block on the JSON-RPC handshake; a fail-fast
    // server exits on its own.
    let mut cmd = sandbox.command();
    cmd.arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn cas serve");

    // EACCES is returned synchronously by the very first store open — well
    // inside the production EAGER_INIT_BUDGET (45s) and orders of magnitude
    // faster than the budget itself would fire. Give the process a generous
    // 25s ceiling to avoid flaking on slow CI while still staying under the
    // budget so a regression that changed the *budget* path (rather than the
    // EACCES path) would be visible as a different failure shape.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let exit_status = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!(
                        "cas serve did not exit within 25s on unreadable cas.db — \
                         the silent-zero-tool regression is back. The server should \
                         abort during eager store init, not hang waiting for stdin."
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };

    assert!(
        !exit_status.success(),
        "cas serve must exit non-zero when cas.db is unreadable; got success exit"
    );

    // Drain stderr and require the exact context string anchored by
    // `with_context` in eager_init_stores. Anchoring on this specific phrase
    // ensures the test fails loudly if the diagnostic is ever stripped or
    // refactored — it cannot be satisfied by some incidentally-similar log
    // line from elsewhere in the binary (review T5).
    let mut stderr = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut stderr);
    }
    assert!(
        stderr.contains("eager store init failed at"),
        "stderr must contain the eager_init_stores diagnostic; got: {stderr}"
    );
}

#[test]
fn test_serve_starts_degraded_and_warns_when_m213_is_partially_applied() {
    let sandbox = CasSandbox::new();
    let db_path = sandbox.cas_root().join("cas.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_verification_capabilities_dispatch;
             DROP INDEX IF EXISTS idx_verifications_dispatch;
             ALTER TABLE verification_capabilities DROP COLUMN dispatch_id;
             ALTER TABLE verifications DROP COLUMN dispatch_id;
             DELETE FROM cas_migrations WHERE id = 213;",
        )
        .unwrap();
    }

    let mut client = McpTestClient::spawn_capturing_stderr(&sandbox);
    let response = client.initialize();
    assert!(
        response.error.is_none(),
        "cas serve must start in its documented degraded mode while m213 is pending: {:?}",
        response.error
    );
    let stderr = client.stop_and_read_stderr();
    assert!(
        stderr.contains("schema migration(s) pending"),
        "degraded serve startup must emit the pending-migration warning: {stderr}"
    );
    assert!(
        stderr.contains("cas update --schema-only"),
        "warning must name the repair command: {stderr}"
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    for table in ["verification_capabilities", "verifications"] {
        assert_eq!(
            conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{table}')
                     WHERE name = 'dispatch_id'"
                ),
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0,
            "cas serve must not silently run m213 for {table}"
        );
    }
}

#[test]
fn test_serve_logs_actual_tool_list_on_startup() {
    // Companion check: in the happy path, `cas serve` must log the *actual*
    // tool count and tool names, not the historical hard-coded "13 tools"
    // string. This is what gives a supervisor (or human reading logs) a
    // chance to notice if the registry shrinks unexpectedly.
    let sandbox = CasSandbox::new();

    let mut cmd = sandbox.command();
    cmd.arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn cas serve");

    // Poll stderr line-by-line until we see the banner (or a 30s deadline
    // expires) instead of an unconditional sleep — under CI load, store
    // init can exceed any fixed sleep window and a kill-too-early would
    // produce a spurious failure on exactly the slow environments where
    // this regression matters most (review T4/A4).
    let stderr_pipe = child.stderr.take().expect("stderr piped");
    let mut reader = BufReader::new(stderr_pipe);
    let mut collected = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

    let banner_seen = loop {
        if std::time::Instant::now() >= deadline {
            break false;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break false, // EOF — process exited before printing banner
            Ok(_) => {
                collected.push_str(&line);
                if line.contains("Starting MCP server") {
                    break true;
                }
            }
            Err(_) => break false,
        }
    };

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        banner_seen,
        "expected startup banner in stderr within 30s; got: {collected}"
    );
    // Banner must include at least one canonical tool name to prove the count
    // is derived from the live registry, not a string literal.
    assert!(
        collected.contains("memory") && collected.contains("task"),
        "startup banner should list registered tool names (memory, task, ...); \
         got: {collected}"
    );
}
