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
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[path = "support/mod.rs"]
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

/// Upper bound on how long the harness will wait for a single JSON-RPC
/// response before failing the test.
///
/// WHY THIS EXISTS (cas-7bb94): this harness previously did an unbounded
/// blocking `read_line` on the server's stdout. If a response never arrived
/// the test thread wedged forever and was eventually killed by nextest's
/// 600s slow-timeout — which reddens CI, costs ~10 minutes of runner time,
/// and gets misattributed to whatever unrelated commit happened to be in
/// flight. A bounded read turns that into a fast, legible failure.
///
/// The value is chosen to sit *above* every server-side bound so a genuine
/// server fault still surfaces as the server's own diagnostic rather than
/// being masked by the harness:
///   * `server_handler::call_tool` self-times-out at 55s and replies with a
///     structured "timed out after 55s" error.
///   * `runtime::EAGER_INIT_BUDGET` checks the startup sequence at 45s.
/// 90s clears both with slack for loaded CI, and is ~6.6x under the 600s
/// nextest kill, so a wedge can never again consume the slow-timeout.
const RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// How much of the child's stderr and of the server log a timeout diagnostic
/// carries (cas-5eb9). Bounded so a chatty server can neither bloat memory
/// nor bury the CI log; enough for the last few dozen lines.
const DIAGNOSTIC_TAIL_BYTES: usize = 4096;

/// The tracing-log directory of the `cas serve` this command will start: the
/// default `logs/` under the `CAS_ROOT` it is pinned to (`CasSandbox` always
/// pins one).
fn server_log_dir(cmd: &Command) -> Option<std::path::PathBuf> {
    cmd.get_envs()
        .find_map(|(key, value)| (key == "CAS_ROOT").then_some(value).flatten())
        .map(|root| std::path::Path::new(root).join("logs"))
}

/// A byte buffer that keeps only its last `cap` bytes.
#[derive(Debug)]
struct BoundedTail {
    bytes: std::collections::VecDeque<u8>,
    cap: usize,
}

impl BoundedTail {
    fn new(cap: usize) -> Self {
        Self {
            bytes: std::collections::VecDeque::with_capacity(cap),
            cap,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        let chunk = &chunk[chunk.len().saturating_sub(self.cap)..];
        let overflow = (self.bytes.len() + chunk.len()).saturating_sub(self.cap);
        self.bytes.drain(..overflow);
        self.bytes.extend(chunk);
    }

    fn render(&self) -> String {
        String::from_utf8_lossy(&self.bytes.iter().copied().collect::<Vec<u8>>()).into_owned()
    }
}

/// The last `cap` bytes of the newest `cas-*.log` in `log_dir`, the daily
/// tracing log `cas serve` writes under its store (`logging::init`).
fn newest_log_tail(log_dir: &std::path::Path, cap: usize) -> Option<(std::path::PathBuf, String)> {
    use std::io::{Read, Seek, SeekFrom};
    let newest = std::fs::read_dir(log_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("cas-") && name.ends_with(".log") && !name.starts_with("cas-serve-")
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max_by_key(|(modified, _)| *modified)?
        .1;
    let mut file = std::fs::File::open(&newest).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(cap as u64))).ok()?;
    let mut bytes = Vec::with_capacity(cap);
    file.take(cap as u64).read_to_end(&mut bytes).ok()?;
    Some((newest, String::from_utf8_lossy(&bytes).into_owned()))
}

/// Why a request failed to produce a usable response. Returned by
/// [`McpTestClient::try_send_request`] so tests can assert on the failure
/// shape instead of relying on a panic message.
#[derive(Debug)]
enum TransportError {
    /// No response within the client's response timeout — the server is
    /// alive (or at least its stdout is still open) but silent.
    Timeout {
        method: String,
        id: u64,
        waited: std::time::Duration,
        child_status: Option<std::process::ExitStatus>,
        /// Last [`DIAGNOSTIC_TAIL_BYTES`] of the child's stderr (cas-5eb9).
        stderr_tail: String,
        /// Last [`DIAGNOSTIC_TAIL_BYTES`] of the server's own tracing log,
        /// where `MCP call_tool START ... id=<id>` is written: its presence
        /// shows the request was read (cas-5eb9).
        server_log_tail: Option<(std::path::PathBuf, String)>,
    },
    /// The server closed stdout (or exited) without answering.
    Closed { method: String, id: u64 },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Timeout {
                method,
                id,
                waited,
                child_status,
                stderr_tail,
                server_log_tail,
            } => {
                write!(
                    f,
                    "no JSON-RPC response to method '{method}' (id {id}) within {}s; \
                     cas serve child status: {}. The request path produced no response; \
                     this alone cannot distinguish a request that was never read, a \
                     handler that never completed, or a response that was never flushed. \
                     A server-log line `MCP call_tool START ... id={id}` below means the \
                     request was read.",
                    waited.as_secs(),
                    match child_status {
                        Some(status) => format!("exited {status}"),
                        None => "still running".to_string(),
                    }
                )?;
                write!(
                    f,
                    "\n--- cas serve stderr (last {DIAGNOSTIC_TAIL_BYTES} bytes) ---\n{}",
                    if stderr_tail.is_empty() { "<empty>" } else { stderr_tail }
                )?;
                match server_log_tail {
                    Some((path, tail)) => write!(
                        f,
                        "\n--- cas serve log {} (last {DIAGNOSTIC_TAIL_BYTES} bytes) ---\n{}",
                        path.display(),
                        if tail.is_empty() { "<empty>" } else { tail }
                    ),
                    None => write!(f, "\n--- cas serve log: none found ---"),
                }
            }
            TransportError::Closed { method, id } => write!(
                f,
                "cas serve closed stdout without responding to method '{method}' (id {id})"
            ),
        }
    }
}

/// Helper to communicate with MCP server
struct McpTestClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    /// Lines drained from the child's stdout by a dedicated reader thread.
    /// Using a channel (rather than reading the pipe inline) is what makes
    /// the response wait bounded: `recv_timeout` can give up, a blocking
    /// `read_line` cannot.
    stdout_lines: std::sync::mpsc::Receiver<String>,
    /// Accumulated stderr, present only when stderr is piped. Drained by a
    /// dedicated thread so the child can never wedge on a full stderr pipe
    /// mid-test (the same unbounded-blocking class as the stdout read).
    stderr_buf: Option<std::sync::Arc<std::sync::Mutex<String>>>,
    /// The last [`DIAGNOSTIC_TAIL_BYTES`] of stderr, kept for every child so
    /// a timeout can say what the server last printed (cas-5eb9).
    stderr_tail: std::sync::Arc<std::sync::Mutex<BoundedTail>>,
    /// The server's tracing-log directory, when known.
    server_log_dir: Option<std::path::PathBuf>,
    next_id: u64,
    response_timeout: std::time::Duration,
}

impl McpTestClient {
    /// Spawn MCP server (7 meta-tools)
    fn spawn(sandbox: &CasSandbox) -> Self {
        Self::spawn_command(sandbox.command())
    }

    fn spawn_command(mut cmd: Command) -> Self {
        cmd.arg("serve");
        let server_log_dir = server_log_dir(&cmd);
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Piped and drained into a bounded tail, never left undrained
            // (a full pipe would wedge the server) nor unbounded (cas-5eb9).
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas serve");

        Self::from_child(child, false, server_log_dir)
    }

    fn spawn_capturing_stderr(sandbox: &CasSandbox) -> Self {
        let mut cmd = sandbox.command();
        cmd.arg("serve");
        let server_log_dir = server_log_dir(&cmd);
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas serve");

        Self::from_child(child, true, server_log_dir)
    }

    fn spawn_command_capturing_stderr(mut cmd: Command) -> Self {
        cmd.arg("serve");
        let server_log_dir = server_log_dir(&cmd);
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn cas serve");

        Self::from_child(child, true, server_log_dir)
    }

    /// Wire a spawned child up to the draining reader threads. A piped
    /// stderr is always drained into the bounded tail; `capture_stderr`
    /// additionally keeps all of it for [`Self::stop_and_read_stderr`].
    fn from_child(
        mut child: std::process::Child,
        capture_stderr: bool,
        server_log_dir: Option<std::path::PathBuf>,
    ) -> Self {
        let stdin = child.stdin.take().expect("Failed to get stdin");
        let stdout = child.stdout.take().expect("Failed to get stdout");

        let (tx, stdout_lines) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    // EOF: dropping `tx` disconnects the channel, which the
                    // request path reports as `TransportError::Closed`.
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line).is_err() {
                            break; // client dropped
                        }
                    }
                }
            }
        });

        let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(BoundedTail::new(
            DIAGNOSTIC_TAIL_BYTES,
        )));
        let stderr_buf = capture_stderr
            .then(|| std::sync::Arc::new(std::sync::Mutex::new(String::new())));
        if let Some(mut pipe) = child.stderr.take() {
            let tail = std::sync::Arc::clone(&stderr_tail);
            let full = stderr_buf.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                // Fixed-size reads: a newline-free flood cannot grow a line
                // buffer without bound the way `read_line` would.
                let mut chunk = [0u8; 4096];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            tail.lock().expect("stderr tail poisoned").push(&chunk[..n]);
                            if let Some(full) = &full {
                                full.lock()
                                    .expect("stderr buffer poisoned")
                                    .push_str(&String::from_utf8_lossy(&chunk[..n]));
                            }
                        }
                    }
                }
            });
        }

        Self {
            child,
            stdin,
            stdout_lines,
            stderr_buf,
            stderr_tail,
            server_log_dir,
            next_id: 1,
            response_timeout: RESPONSE_TIMEOUT,
        }
    }

    fn stop_and_read_stderr(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let buf = self
            .stderr_buf
            .as_ref()
            .expect("client was not spawned with stderr captured");
        // The child is dead, so the drain thread is at (or racing toward)
        // EOF. Poll briefly for it to finish flushing rather than joining,
        // so a stuck reader can never wedge the test either.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            let snapshot = buf.lock().expect("stderr buffer poisoned").clone();
            if !snapshot.is_empty() && snapshot == last {
                break;
            }
            last = snapshot;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        last
    }

    /// Send a request and wait, with a bound, for its response.
    ///
    /// Returns `Err` instead of blocking forever when the server never
    /// answers. `send_request` is the panicking convenience wrapper used by
    /// the protocol tests; this variant exists so the regression test can
    /// assert the bound actually fires.
    fn try_send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<JsonRpcResponse, TransportError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest::new(id, method, params);
        let request_json = serde_json::to_string(&request).expect("Failed to serialize request");

        // Send request
        writeln!(self.stdin, "{request_json}").expect("Failed to write request");
        self.stdin.flush().expect("Failed to flush");

        // Read response (skip notifications with no id). The deadline covers
        // the whole exchange, so a server that streams endless notifications
        // without ever answering cannot extend the wait indefinitely either.
        let started = std::time::Instant::now();
        let deadline = started + self.response_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let response_line = match self.stdout_lines.recv_timeout(remaining) {
                Ok(line) => line,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err(TransportError::Timeout {
                        method: method.to_string(),
                        id,
                        waited: started.elapsed(),
                        child_status: self.child.try_wait().ok().flatten(),
                        stderr_tail: self.stderr_tail.lock().expect("stderr tail poisoned").render(),
                        server_log_tail: self
                            .server_log_dir
                            .as_deref()
                            .and_then(|dir| newest_log_tail(dir, DIAGNOSTIC_TAIL_BYTES)),
                    });
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(TransportError::Closed {
                        method: method.to_string(),
                        id,
                    });
                }
            };

            let response: JsonRpcResponse =
                serde_json::from_str(&response_line).expect("Failed to parse response");
            assert_eq!(response.jsonrpc, "2.0", "Invalid JSON-RPC version");

            match response.id {
                Some(resp_id) => {
                    assert_eq!(resp_id, id, "Response ID should match request");
                    return Ok(response);
                }
                None => {
                    // Notification or event; continue reading.
                    continue;
                }
            }
        }
    }

    fn send_request(&mut self, method: &str, params: Option<Value>) -> JsonRpcResponse {
        match self.try_send_request(method, params) {
            Ok(response) => response,
            Err(err) => panic!("MCP transport failure: {err}"),
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

    /// Attach to a stand-in "server" that consumes stdin and never writes a
    /// byte to stdout — the exact shape of the non-response that used to
    /// wedge this harness for 600s. Used only by the regression test below.
    #[cfg(unix)]
    fn spawn_silent_stub(response_timeout: std::time::Duration) -> Self {
        Self::spawn_hung_stub("cat >/dev/null", None, response_timeout)
    }

    /// A stand-in server that runs `script` under `sh` — which may write to
    /// stderr or a log under `server_log_dir` — and never answers on stdout.
    /// The script must keep stdout open (e.g. `exec 3>&1` before redirecting
    /// a final command), or the client sees `Closed` rather than a timeout.
    #[cfg(unix)]
    fn spawn_hung_stub(
        script: &str,
        server_log_dir: Option<std::path::PathBuf>,
        response_timeout: std::time::Duration,
    ) -> Self {
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hung stub");
        let mut client = Self::from_child(child, false, server_log_dir);
        client.response_timeout = response_timeout;
        client
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
            "risk": "none",
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
    let mut cmd = Command::new(support::cas_binary());
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
            "risk": "none",
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
    let mut serve = Command::new(support::cas_binary());
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
            "action": "create",
            "risk": "none"
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

/// Regression guard for cas-7bb94.
///
/// `test_mcp_unknown_tool` was observed wedging on CI until nextest's 600s
/// slow-timeout killed it, turning Fast Validation red on a commit that
/// touched only an insta snapshot fixture. The 600s was owned by this
/// harness: `send_request` did an unbounded blocking `read_line`, so a
/// server that never answered produced an infinite wait instead of a test
/// failure.
///
/// This test pins the invariant that made the 600s possible: a request that
/// produces no response must surface as a bounded, legible
/// `TransportError::Timeout` — never as an unbounded block. It drives a
/// deliberately silent stub so the non-response is deterministic rather than
/// inferring which unobserved server phase caused the original CI occurrence.
#[cfg(unix)]
#[test]
fn test_response_wait_is_bounded_when_server_never_answers() {
    let budget = std::time::Duration::from_secs(2);
    let mut client = McpTestClient::spawn_silent_stub(budget);

    let started = std::time::Instant::now();
    let result = client.try_send_request("tools/list", None);
    let elapsed = started.elapsed();

    let err = match result {
        Err(err) => err,
        Ok(response) => panic!("silent stub must not produce a response: {response:?}"),
    };

    match &err {
        TransportError::Timeout { child_status, .. } => {
            assert!(
                child_status.is_none(),
                "stub should still be running; got {child_status:?}"
            );
        }
        other => panic!("expected a bounded timeout, got: {other}"),
    }

    // Generous ceiling so this cannot flake on a loaded runner, while still
    // being orders of magnitude below the 600s nextest kill it replaces.
    assert!(
        elapsed < budget + std::time::Duration::from_secs(30),
        "response wait must be bounded by the client timeout; waited {elapsed:?}"
    );

    // The diagnostic must name the method without claiming which unobserved
    // phase failed. The original CI log did not establish that the server read
    // the request, so "accepted the request" would turn a symptom into a false
    // root-cause claim.
    let rendered = err.to_string();
    assert!(
        rendered.contains("tools/list")
            && rendered.contains("cannot distinguish")
            && !rendered.contains("accepted the request"),
        "timeout diagnostic must identify the silent request without overclaiming: {rendered}"
    );
}

/// cas-5eb9: a non-response must arrive with what the server last said.
///
/// 834 failed CI runs since the bounded wait landed carried no occurrence, and
/// the one wedge on record had its stderr nulled, so nothing showed whether the
/// request was ever read. The timeout diagnostic now carries a bounded tail of
/// the child's stderr and of the server's tracing log (where `MCP call_tool
/// START ... id=<id>` lands). This drives a deliberately hung child that floods
/// stderr past the bound and logs a START, then asserts both tails reach the
/// message and the stderr tail kept the end, not the start, within its bound.
#[cfg(unix)]
#[test]
fn test_timeout_diagnostic_carries_bounded_stderr_and_server_log_tails() {
    let log_dir = tempfile::tempdir().expect("server log dir");
    let log_file = log_dir.path().join("cas-2026-10-03.log");
    let script = format!(
        "printf 'stub-first-words-cas-5eb9\\n' >&2; \
         head -c {flood} /dev/zero | tr '\\000' x >&2; \
         printf 'INFO MCP call_tool START method=\"tools/call\" tool=nonexistent_tool id=1\\n' > '{log}'; \
         printf '\\nstub-last-words-cas-5eb9\\n' >&2; \
         exec 3>&1; cat >/dev/null",
        flood = DIAGNOSTIC_TAIL_BYTES * 5,
        log = log_file.display(),
    );
    let mut client = McpTestClient::spawn_hung_stub(
        &script,
        Some(log_dir.path().to_path_buf()),
        std::time::Duration::from_secs(3),
    );

    let err = match client.try_send_request("tools/call", Some(json!({"name": "nonexistent_tool"}))) {
        Err(err) => err,
        Ok(response) => panic!("hung stub must not produce a response: {response:?}"),
    };
    let rendered = err.to_string();
    let TransportError::Timeout {
        child_status,
        stderr_tail,
        server_log_tail,
        ..
    } = err
    else {
        panic!("expected a bounded timeout, got: {rendered}");
    };

    assert!(child_status.is_none(), "stub should still be running: {child_status:?}");
    assert!(
        stderr_tail.contains("stub-last-words-cas-5eb9"),
        "the stderr tail keeps what the child printed last: {stderr_tail}"
    );
    assert!(
        !stderr_tail.contains("stub-first-words-cas-5eb9") && stderr_tail.len() <= DIAGNOSTIC_TAIL_BYTES,
        "the stderr tail is bounded to the last {DIAGNOSTIC_TAIL_BYTES} bytes, got {} bytes",
        stderr_tail.len()
    );
    let (path, log_tail) = server_log_tail.expect("the server log tail is captured");
    assert_eq!(path, log_file);
    assert!(log_tail.contains("MCP call_tool START") && log_tail.contains("id=1"));
    assert!(
        rendered.contains("stub-last-words-cas-5eb9")
            && rendered.contains("MCP call_tool START")
            && rendered.contains("cannot distinguish"),
        "both tails reach the timeout message: {rendered}"
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
            "risk": "none",
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
            "risk": "none",
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
            "risk": "none",
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

    // Schema reconciliation now runs before eager store initialization, so an
    // unreadable database fails at that earlier boundary. Keep asserting the
    // user-actionable diagnostic rather than a private eager-init label: the
    // important contract is a loud non-zero startup failure that tells the
    // operator how to repair the inaccessible database.
    let mut stderr = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut stderr);
    }
    assert!(
        stderr.contains("database error: unable to open database file"),
        "stderr must name the unreadable database failure; got: {stderr}"
    );
    assert!(
        stderr.contains("cas doctor") && stderr.contains("permissions"),
        "stderr must give an actionable database-permissions diagnostic; got: {stderr}"
    );
    assert!(
        !stderr.contains("Starting MCP server"),
        "cas serve must not start the MCP server after an unreadable-database failure; got: {stderr}"
    );
}

#[test]
fn test_serve_repairs_m213_before_starting() {
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
        "cas serve must start after repairing pending m213: {:?}",
        response.error
    );
    let stderr = client.stop_and_read_stderr();
    assert!(
        stderr.contains("Applied 1 pending schema migration(s) before MCP startup."),
        "startup must report that it repaired m213 before serving: {stderr}"
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
            1,
            "cas serve must restore m213's dispatch_id column for {table}"
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM cas_migrations WHERE id = 213",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1,
        "automatic startup repair must record m213 as applied"
    );
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

/// cas-8256: a factory worker's `cas serve` never runs the code indexer
/// against the shared store; the canonical checkout's serve still does.
/// Every worker used to index its whole worktree on boot (58 copies, 364k
/// symbols in the cassy store).
#[test]
fn factory_worker_serve_never_starts_the_code_indexer_cas_8256() {
    let sandbox = CasSandbox::new();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(sandbox.path())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet"]);
    std::fs::create_dir_all(sandbox.path().join("src")).unwrap();
    std::fs::write(
        sandbox.path().join("src/lib.rs"),
        "pub fn sandbox_symbol() {}\n",
    )
    .unwrap();

    let mut worker = sandbox.command();
    worker.env("CAS_AGENT_ROLE", "worker");
    let mut client = McpTestClient::spawn_command_capturing_stderr(worker);
    client.initialize();
    let stderr = client.stop_and_read_stderr();
    assert!(
        stderr.contains("Code indexing off in this process: factory worker"),
        "worker serve did not report its reader role: {stderr}"
    );
    assert!(
        !stderr.contains("Code watcher started"),
        "worker serve started the code indexer: {stderr}"
    );
    let files: i64 = rusqlite::Connection::open(sandbox.path().join(".cas/cas.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'code_files'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if files > 0 {
        let indexed: i64 = rusqlite::Connection::open(sandbox.path().join(".cas/cas.db"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM code_files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(indexed, 0, "worker serve indexed source files");
    }

    // Control: the canonical checkout's serve does start the indexer, so the
    // absence above is the role gate, not a broken watcher.
    let mut canonical = McpTestClient::spawn_command_capturing_stderr(sandbox.command());
    canonical.initialize();
    let stderr = canonical.stop_and_read_stderr();
    assert!(
        stderr.contains("Code watcher started"),
        "canonical serve did not start the code indexer: {stderr}"
    );
    assert!(!stderr.contains("Code indexing off in this process"), "{stderr}");
}
