//! Regression test for cas-f5e4 / EPIC cas-c351.
//!
//! On 2026-04-16 `cas serve` crashed on every `search` call that
//! surfaced the "CAS factory daemon boot order" entry: six open-coded
//! `&s[..max_len-3]` preview-truncation paths cut inside the three
//! bytes of `→`, Rust's string slice panicked, the Tokio worker
//! unwound, the stdio transport closed, and the MCP client saw only
//! "Connection closed". The fix (commit ad60df3) centralised truncation
//! into `cas_types::preview::truncate_preview` with `is_char_boundary`
//! walkback; unit-tested in the `cas-types` crate against the exact
//! fixture.
//!
//! This file reproduces the failure end-to-end over MCP stdio against
//! a real `cas serve` subprocess. Fails if `ad60df3` is reverted —
//! `search.error` catches the post-cas-a436 INTERNAL_ERROR path, the
//! liveness probe catches the pre-cas-a436 server-death path.
//!
//! Runs in default `cargo test`, no network / no cloud auth.

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod support;
use support::{CasSandbox, assert_command_is_sandboxed};

/// The exact entry content that crashed `cas serve` on 2026-04-16. Byte
/// index 57 lands inside the three bytes of `→`. Changing this string
/// means you are no longer reproducing the real regression — introduce
/// a second fixture instead of mutating this one.
const CRASH_FIXTURE: &str =
    "CAS factory daemon boot order: `build_configs_for_mux` → `FactoryApp::new` (spawns PTYs)";

/// ASCII substring of [`CRASH_FIXTURE`] that the BM25 preview must
/// include when the entry is surfaced. Sits before the multi-byte cut
/// so it survives any `truncate_preview(..., 60)` output, and is
/// distinctive enough that a stray "no results found" response cannot
/// contain it by accident.
const FIXTURE_PREVIEW_MARKER: &str = "build_configs_for_mux";

// ============================================================================
// JSON-RPC plumbing (copied from mcp_protocol_test.rs to keep this
// regression test self-contained; cross-file test-util extraction is
// tracked as a separate refactor under the EPIC).
// ============================================================================

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields are consumed by {:?} in assertion messages
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields are consumed by {:?} in assertion messages
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<Value>,
}

struct McpClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// Spawn `cas serve` in a hermetic CAS sandbox.
    fn spawn(sandbox: &CasSandbox) -> Self {
        let mut cmd = sandbox.command();
        assert_command_is_sandboxed(&cmd, sandbox);
        cmd.arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().expect("Failed to spawn cas serve");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Send a JSON-RPC request and read the matching response. If the
    /// child has died (e.g., from a preview panic), the underlying
    /// read/write fails — the caller gets to observe that as the
    /// regression signal.
    fn send_request(&mut self, method: &str, params: Option<Value>) -> JsonRpcResponse {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&req).expect("serialize");
        writeln!(self.stdin, "{line}").expect("write stdin — server pipe closed?");
        self.stdin
            .flush()
            .expect("flush stdin — server pipe closed?");

        // Skip notifications (no id) and any non-matching responses
        // (e.g., server-initiated requests per later MCP spec
        // revisions) until we see our matching response.
        loop {
            let mut line = String::new();
            let n = self
                .stdout
                .read_line(&mut line)
                .expect("read stdout — server pipe closed? likely preview() panic");
            assert!(n > 0, "stdout EOF before response — server died");

            let resp: JsonRpcResponse = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("parse response '{line}': {e}"));
            assert_eq!(resp.jsonrpc, "2.0", "invalid JSON-RPC version");
            match resp.id {
                Some(rid) if rid == id => return resp,
                Some(_) | None => continue,
            }
        }
    }

    fn send_notification(&mut self, method: &str, params: Option<Value>) {
        let n = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(json!({}))
        });
        let line = serde_json::to_string(&n).expect("serialize");
        writeln!(self.stdin, "{line}").expect("write notif");
        self.stdin.flush().expect("flush notif");
    }

    fn initialize(&mut self) {
        let resp = self.send_request(
            "initialize",
            Some(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "cas-f5e4-test", "version": "1.0.0" }
            })),
        );
        assert!(resp.error.is_none(), "initialize failed: {:?}", resp.error);
        self.send_notification("notifications/initialized", None);
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> JsonRpcResponse {
        self.send_request(
            "tools/call",
            Some(json!({ "name": name, "arguments": arguments })),
        )
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Collect the concatenated `text` from every text-content block in a
/// `tools/call` result. Used to assert that the BM25-surfaced preview
/// actually contains the fixture marker — the discriminating signal
/// that proves `Entry::preview(60)` ran on our seeded entry.
fn collect_response_text(result: &Value) -> String {
    let Some(arr) = result.get("content").and_then(|c| c.as_array()) else {
        return String::new();
    };
    arr.iter()
        .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check the `is_error: true` flag that the tool-call result envelope
/// uses to signal tool-level failures (distinct from JSON-RPC errors,
/// which surface in the `.error` field).
fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ============================================================================
// The regression
// ============================================================================

#[test]
fn search_with_multibyte_boundary_content_does_not_crash_server() {
    // Sanity: confirm the fixture actually lands a multi-byte char
    // across the byte-57 cut, so this test stays grounded in the real
    // bug. If the constant drifts and byte 57 becomes an ASCII
    // boundary, the panic path is not being exercised and the test
    // would be worthless.
    assert!(
        !CRASH_FIXTURE.is_char_boundary(57),
        "fixture lost its mid-char cut; test no longer reproduces the 2026-04-16 crash"
    );
    assert!(
        CRASH_FIXTURE.len() > 60,
        "fixture shorter than 60 bytes — preview(60) would return it unchanged"
    );
    // FIXTURE_PREVIEW_MARKER must appear in the pre-cut prefix so a
    // well-behaved `truncate_preview(..., 60)` still surfaces it.
    let prefix_end = {
        let mut e = 57usize.min(CRASH_FIXTURE.len());
        while e > 0 && !CRASH_FIXTURE.is_char_boundary(e) {
            e -= 1;
        }
        e
    };
    assert!(
        CRASH_FIXTURE[..prefix_end].contains(FIXTURE_PREVIEW_MARKER),
        "preview marker not in pre-cut prefix — assertion would miss even on a correct preview"
    );

    let sandbox = CasSandbox::new();

    let mut client = McpClient::spawn(&sandbox);
    client.initialize();

    // Step 1: seed the crashing entry. `bypass_overlap=true` keeps the
    // test focused on the preview regression — without the bypass, an
    // overlap check could in principle short-circuit the store with
    // an `is_error: true` tool-result envelope that carries no
    // JSON-RPC error, making the seed silently no-op.
    let remember = client.call_tool(
        "memory",
        json!({
            "action": "remember",
            "content": CRASH_FIXTURE,
            "entry_type": "learning",
            "tags": "cas-f5e4,regression,factory",
            "bypass_overlap": true
        }),
    );
    assert!(
        remember.error.is_none(),
        "memory.remember failed at JSON-RPC layer: {:?}",
        remember.error
    );
    let remember_result = remember
        .result
        .as_ref()
        .expect("memory.remember must return a result");
    assert!(
        !is_tool_error(remember_result),
        "memory.remember returned a tool-level error: {}",
        collect_response_text(remember_result)
    );

    // Step 2: trigger the panic path. Before ad60df3, this call would
    // crash the server inside entry.preview(60). After ad60df3, the
    // truncate walks back to byte 55 and returns cleanly. If cas-a436
    // is in place but ad60df3 is reverted, the panic is caught and
    // surfaced as INTERNAL_ERROR — still a test failure.
    let search = client.call_tool(
        "search",
        json!({
            "action": "search",
            "query": "factory daemon boot"
        }),
    );
    assert!(
        search.error.is_none(),
        "search.search errored at JSON-RPC layer (preview truncation regression?): {:?}",
        search.error
    );
    let search_result = search
        .result
        .as_ref()
        .expect("search.search must return a result");
    assert!(
        !is_tool_error(search_result),
        "search.search returned a tool-level error: {}",
        collect_response_text(search_result)
    );

    // Discriminating assertion: the BM25 index must have surfaced our
    // seeded entry and `Entry::preview(60)` must have emitted text
    // containing the fixture marker. Without this, a "no results
    // found" success response would pass the earlier checks and the
    // preview path would never run — the regression would not be
    // caught even if introduced.
    let search_text = collect_response_text(search_result);
    assert!(
        search_text.contains(FIXTURE_PREVIEW_MARKER),
        "search result did not surface the seeded fixture — BM25 miss or reader-lag; \
         preview() was not exercised, test is vacuously green. Response text:\n{search_text}"
    );

    // Step 3: server-liveness probe on the SAME pipe. With the A2
    // panic catcher shipped (cas-a436), a preview panic no longer
    // closes the pipe, so this probe is a belt-and-suspenders check
    // for the pre-A2 failure mode (worker panic crashes the whole
    // server). If cas-a436 were removed AND ad60df3 were reverted,
    // the pipe would be dead and this call would error from the read
    // side of send_request.
    let list = client.call_tool("memory", json!({ "action": "list", "limit": 1 }));
    assert!(
        list.error.is_none(),
        "memory.list after search must succeed — server appears to have died"
    );
    drop(client);
    assert_eq!(
        sandbox.task_count(),
        0,
        "search regression must not create tasks in its sandbox"
    );
}
