//! CLI claim and recovery integration tests for external Slack-thread work.

#![cfg(feature = "mcp-proxy")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cas::types::{Agent, AgentRole};
use serde_json::{Value, json};

#[path = "support/mod.rs"]
mod support;

use support::CasSandbox;

struct FakeCloud {
    endpoint: String,
    request_count: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
    _server: Arc<tiny_http::Server>,
}

impl FakeCloud {
    fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("fake Cloud binds"));
        let endpoint = format!("http://{}", server.server_addr().to_ip().unwrap());
        let request_count = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (thread_server, thread_count, thread_requests) = (
            Arc::clone(&server),
            Arc::clone(&request_count),
            Arc::clone(&requests),
        );
        std::thread::spawn(move || {
            for mut request in thread_server.incoming_requests().take(1) {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let payload: Value = serde_json::from_str(&body).expect("Cloud request is JSON");
                thread_count.fetch_add(1, Ordering::SeqCst);
                thread_requests.lock().unwrap().push(payload.clone());
                let request_id = payload["request_id"].as_str().unwrap_or_default();
                let response = json!({
                    "schema": "cassy.coordination/v1",
                    "code": "acquired",
                    "guidance": "Claim recorded.",
                    "receipt": {
                        "operation_id": "00000b01-1111-4111-8111-000000000b01",
                        "request_id": request_id,
                        "resource": {"id": "6d1f0c3a-2b4e-4f6a-8c9d-0e1f2a3b4c5d", "kind": "slack_thread", "state": "claimed"},
                        "visible_owner": {"slot_id": "a1a1a1a1-0000-4000-8000-00000000000a", "registration_id": "cli-supervisor"},
                        "generation": 1,
                        "expires_at": "2026-10-16T14:05:00.000Z",
                        "observed_at": "2026-10-16T14:00:00.000Z",
                        "next_action": "proceed"
                    }
                });
                let response = tiny_http::Response::from_string(response.to_string())
                    .with_status_code(200)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    );
                request.respond(response).expect("respond to Cloud request");
            }
        });
        Self {
            endpoint,
            request_count,
            requests,
            _server: server,
        }
    }
}

#[test]
fn cli_claim_then_claim_status_reuses_the_journaled_receipt() {
    let sandbox = CasSandbox::new();
    let session_id = "coordination-cli-supervisor";
    cas::store::open_agent_store(&sandbox.cas_root())
        .unwrap()
        .register(&Agent::new_with_role(
            session_id.to_string(),
            "coordination CLI supervisor".to_string(),
            AgentRole::Supervisor,
        ))
        .unwrap();

    let cloud = FakeCloud::start();
    std::fs::write(
        sandbox.cas_root().join("config.toml"),
        "[project]\ncanonical_id = \"github.com/richards-llc/cassy\"\n",
    )
    .unwrap();
    std::fs::write(
        sandbox.cas_root().join("cloud.json"),
        json!({"endpoint": cloud.endpoint, "token": "integration-test-token"}).to_string(),
    )
    .unwrap();

    let server_script = sandbox.path().join("fake_violet.py");
    let violet_calls = sandbox.path().join("violet_calls.txt");
    std::fs::write(
        &server_script,
        r#"import json, os, sys
identity = {"ok": True, "schema_version": 1, "identity": {
    "workspace_id": "T011K7W2FK8", "channel_id": "C0B44GUKDK2",
    "root_thread_ts": "1792161452.497159"}}
for line in sys.stdin:
    try:
        request = json.loads(line)
    except Exception:
        continue
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "fixture-violet", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "violet_read", "description": "Read thread identity",
                   "inputSchema": {"type": "object", "properties": {"external_ref": {"type": "string"}},
                                   "required": ["external_ref"]}}]}
    elif method == "tools/call":
        with open(os.environ["VIOLET_CALL_COUNT"], "a", encoding="utf-8") as calls:
            calls.write("call\n")
        result = {"content": [{"type": "text", "text": json.dumps(identity)}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#,
    )
    .unwrap();
    std::fs::write(
        sandbox.cas_root().join("proxy.toml"),
        format!(
            "allowlist = [\"violet.violet_read\"]\n\n[servers.violet]\ntransport = \"stdio\"\ncommand = \"python3\"\nargs = [{:?}]\n[servers.violet.env]\nVIOLET_CALL_COUNT = {:?}\n",
            server_script.to_string_lossy(),
            violet_calls.to_string_lossy(),
        ),
    )
    .unwrap();

    let request_id = "00000001-2222-4222-8222-000000000001";
    let external_ref = "https://richards-llc.slack.com/archives/C0B44GUKDK2/p1792161452497159";
    let claim = sandbox
        .command()
        .env("CAS_SESSION_ID", session_id)
        .args([
            "coordination",
            "claim",
            "--external-ref",
            external_ref,
            "--request-id",
            request_id,
        ])
        .output()
        .expect("run CLI coordination claim");
    assert!(
        claim.status.success(),
        "claim failed: {}",
        String::from_utf8_lossy(&claim.stderr)
    );
    let claim: Value = serde_json::from_slice(&claim.stdout).expect("claim output is JSON");
    assert_eq!(claim["code"], "acquired");
    assert_eq!(
        claim.pointer("/receipt/request_id").and_then(Value::as_str),
        Some(request_id)
    );

    let status = sandbox
        .command()
        .env("CAS_SESSION_ID", session_id)
        .args(["coordination", "claim-status", "--request-id", request_id])
        .output()
        .expect("run CLI coordination claim-status");
    assert!(
        status.status.success(),
        "claim-status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("status output is JSON");
    assert_eq!(status["code"], "acquired");
    assert_eq!(status["receipt"], claim["receipt"]);
    assert_eq!(cloud.request_count.load(Ordering::SeqCst), 1);
    assert_eq!(cloud.requests.lock().unwrap()[0]["request_id"], request_id);
    assert_eq!(
        std::fs::read_to_string(violet_calls)
            .unwrap()
            .lines()
            .count(),
        1,
        "claim-status should reuse the local receipt without resolving or dispatching again"
    );
}
