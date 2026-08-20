//! Public-hook regression coverage for sealed verifier handoff cleanup.
//!
//! These tests intentionally use the `cas hook` subprocess surface for both
//! issuance and terminal routing. Store APIs are used only to seed proof-cycle
//! fixtures and to move one already-bound row to its consumed audit state.

use assert_cmd::Command;
use rusqlite::{Connection, params};
use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-07-31T12:00:00+00:00";
const DEADLINE: &str = "2099-01-01T00:00:00+00:00";

fn cas_cmd(dir: &TempDir) -> Command {
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
    for name in [
        "CAS_ROOT",
        "CAS_AGENT_ROLE",
        "CAS_FACTORY_MODE",
        "CAS_FACTORY_WORKER_CLI",
        "CAS_FACTORY_SUPERVISOR_CLI",
        "CAS_SESSION_ID",
    ] {
        cmd.env_remove(name);
    }
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

fn run_hook(dir: &TempDir, event: &str, input: &Value) -> Value {
    let output = cas_cmd(dir)
        .args(["hook", event])
        .write_stdin(serde_json::to_string(input).expect("serialize hook input"))
        .output()
        .expect("hook command must run");
    assert!(
        output.status.success(),
        "{event} hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{event} hook returned invalid JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn seed_flow(conn: &Connection, task_id: &str, parent_id: &str, dispatch_id: &str) {
    conn.execute(
        "INSERT INTO agents
         (id, name, agent_type, role, status, registered_at, last_heartbeat)
         VALUES (?1, ?1, 'primary', 'standard', 'active', ?2, ?2)",
        params![parent_id, NOW],
    )
    .expect("seed parent agent");
    conn.execute(
        "INSERT INTO tasks
         (id, title, status, task_type, priority, assignee,
          pending_verification, created_at, updated_at)
         VALUES (?1, ?1, 'in_progress', 'task', 0, ?2, 1, ?3, ?3)",
        params![task_id, parent_id, NOW],
    )
    .expect("seed task");
    conn.execute(
        "INSERT INTO verification_dispatches
         (id, task_id, requester_agent_id, owner_agent_id, state,
          requested_at, deadline_at)
         VALUES (?1, ?2, ?3, ?3, 'pending', ?4, ?5)",
        params![dispatch_id, task_id, parent_id, NOW, DEADLINE],
    )
    .expect("seed verification dispatch");
}

fn pre_tool_input(parent_id: &str, task_id: &str, tool_use_id: &str) -> Value {
    serde_json::json!({
        "session_id": parent_id,
        "cwd": "/portable/project",
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_use_id": tool_use_id,
        "tool_input": {
            "subagent_type": "task-verifier",
            "prompt": format!("Review exact CAS task {task_id}; public-cleanup-transcript-marker")
        }
    })
}

fn bind_input(parent_id: &str, child_id: &str) -> Value {
    serde_json::json!({
        "session_id": parent_id,
        "cwd": "/portable/project",
        "hook_event_name": "SubagentStart",
        "agent_id": child_id,
        "agent_type": "task-verifier",
        "agent_transcript_path": "/portable/verifier.jsonl",
        "permission_mode": "default"
    })
}

#[derive(Debug, PartialEq)]
struct FlowSnapshot {
    capability_id: String,
    token_hash: String,
    capability_child: Option<String>,
    capability_consumed_at: Option<String>,
    handoff_child: Option<String>,
    handoff_state: String,
    handoff_bound_at: Option<String>,
    handoff_consumed_at: Option<String>,
    dispatch_child: Option<String>,
    dispatch_capability: Option<String>,
    dispatch_state: String,
}

fn flow_snapshot(conn: &Connection, task_id: &str) -> FlowSnapshot {
    conn.query_row(
        "SELECT c.id, c.token_hash, c.verifier_agent_id, c.consumed_at,
                h.verifier_agent_id, h.state, h.bound_at, h.consumed_at,
                d.verifier_agent_id, d.capability_id, d.state
         FROM verification_capabilities c
         JOIN verification_handoffs h ON h.capability_id = c.id
         JOIN verification_dispatches d ON d.id = c.dispatch_id
         WHERE c.task_id = ?1",
        [task_id],
        |row| {
            Ok(FlowSnapshot {
                capability_id: row.get(0)?,
                token_hash: row.get(1)?,
                capability_child: row.get(2)?,
                capability_consumed_at: row.get(3)?,
                handoff_child: row.get(4)?,
                handoff_state: row.get(5)?,
                handoff_bound_at: row.get(6)?,
                handoff_consumed_at: row.get(7)?,
                dispatch_child: row.get(8)?,
                dispatch_capability: row.get(9)?,
                dispatch_state: row.get(10)?,
            })
        },
    )
    .expect("load exact flow snapshot")
}

#[test]
fn permission_denied_public_route_cleans_only_exact_unbound_handoff() {
    let dir = tempfile::tempdir().expect("tempdir");
    cas_cmd(&dir).args(["init", "--yes"]).assert().success();
    let cas_root = dir.path().join(".cas");
    let db_path = cas_root.join("cas.db");
    let conn = Connection::open(&db_path).expect("open cas.db");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL privacy surface");

    let flows = [
        ("cas-d7d3-target", "d7d3-target-parent", "vdisp-d7d3-target"),
        (
            "cas-d7d3-unrelated",
            "d7d3-unrelated-parent",
            "vdisp-d7d3-unrelated",
        ),
        ("cas-d7d3-bound", "d7d3-bound-parent", "vdisp-d7d3-bound"),
        (
            "cas-d7d3-consumed",
            "d7d3-consumed-parent",
            "vdisp-d7d3-consumed",
        ),
    ];
    for (task_id, parent_id, dispatch_id) in flows {
        seed_flow(&conn, task_id, parent_id, dispatch_id);
    }

    let target_tool = "tool-use-permission-denied-target-sentinel";
    let unrelated_tool = "tool-use-unrelated-flow-sentinel";
    let bound_tool = "tool-use-bound-flow-sentinel";
    let consumed_tool = "tool-use-consumed-flow-sentinel";
    let target_input = pre_tool_input("d7d3-target-parent", "cas-d7d3-target", target_tool);
    let unrelated_input = pre_tool_input(
        "d7d3-unrelated-parent",
        "cas-d7d3-unrelated",
        unrelated_tool,
    );
    let bound_input = pre_tool_input("d7d3-bound-parent", "cas-d7d3-bound", bound_tool);
    let consumed_input = pre_tool_input("d7d3-consumed-parent", "cas-d7d3-consumed", consumed_tool);
    let mut transcript = Vec::new();
    for input in [
        &target_input,
        &unrelated_input,
        &bound_input,
        &consumed_input,
    ] {
        let output = run_hook(&dir, "PreToolUse", input);
        assert_eq!(output, serde_json::json!({}));
        transcript.push(serde_json::to_string(input).expect("serialize transcript input"));
        transcript.push(serde_json::to_string(&output).expect("serialize transcript output"));
    }

    assert_eq!(
        run_hook(
            &dir,
            "SubagentStart",
            &bind_input("d7d3-bound-parent", "d7d3-bound-child")
        ),
        serde_json::json!({})
    );
    assert_eq!(
        run_hook(
            &dir,
            "SubagentStart",
            &bind_input("d7d3-consumed-parent", "d7d3-consumed-child")
        ),
        serde_json::json!({})
    );
    let consumed = flow_snapshot(&conn, "cas-d7d3-consumed");
    cas_store::consume_server_verifier_handoff_with_conn(
        &conn,
        &consumed.capability_id,
        "cas-d7d3-consumed",
        "d7d3-consumed-child",
    )
    .expect("consume bound audit row");

    let target_before = flow_snapshot(&conn, "cas-d7d3-target");
    let unrelated_before = flow_snapshot(&conn, "cas-d7d3-unrelated");
    let bound_before = flow_snapshot(&conn, "cas-d7d3-bound");
    let consumed_before = flow_snapshot(&conn, "cas-d7d3-consumed");
    assert_eq!(target_before.handoff_state, "pending");
    assert_eq!(unrelated_before.handoff_state, "pending");
    assert_eq!(bound_before.handoff_state, "bound");
    assert_eq!(consumed_before.handoff_state, "consumed");

    let mut wrong_denial = target_input.clone();
    wrong_denial["hook_event_name"] = Value::String("PermissionDenied".to_string());
    wrong_denial["tool_use_id"] = Value::String("tool-use-wrong-id".to_string());
    assert_eq!(
        run_hook(&dir, "PermissionDenied", &wrong_denial),
        serde_json::json!({})
    );
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-target"), target_before);
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-unrelated"), unrelated_before);
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-bound"), bound_before);
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-consumed"), consumed_before);

    let mut exact_denial = target_input.clone();
    exact_denial["hook_event_name"] = Value::String("PermissionDenied".to_string());
    assert_eq!(
        run_hook(&dir, "PermissionDenied", &exact_denial),
        serde_json::json!({})
    );
    let target_rows: (i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM verification_capabilities WHERE task_id = ?1),
                (SELECT COUNT(*) FROM verification_handoffs
                 WHERE issuer_agent_id = ?2)",
            params!["cas-d7d3-target", "d7d3-target-parent"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("target cleanup counts");
    assert_eq!(
        target_rows,
        (0, 0),
        "exact PermissionDenied must remove capability and m215 handoff together"
    );
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-unrelated"), unrelated_before);
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-bound"), bound_before);
    assert_eq!(flow_snapshot(&conn, "cas-d7d3-consumed"), consumed_before);

    let retry_tool = "tool-use-after-permission-denied-cleanup";
    let retry_input = pre_tool_input("d7d3-target-parent", "cas-d7d3-target", retry_tool);
    let retry_output = run_hook(&dir, "PreToolUse", &retry_input);
    assert_eq!(
        retry_output,
        serde_json::json!({}),
        "the same exact proof cycle must permit a fresh spawn after cleanup"
    );
    let retry = flow_snapshot(&conn, "cas-d7d3-target");
    assert_eq!(retry.handoff_state, "pending");
    assert_eq!(retry.dispatch_state, "pending");
    assert_eq!(retry.dispatch_capability, None);
    assert_ne!(retry.capability_id, target_before.capability_id);
    transcript.push(serde_json::to_string(&exact_denial).expect("serialize denial input"));
    transcript.push(serde_json::to_string(&retry_output).expect("serialize retry output"));
    let transcript = transcript.join("\n");
    let transcript_path = dir.path().join("permission-denied-transcript.jsonl");
    std::fs::write(&transcript_path, &transcript).expect("write public hook transcript fixture");
    let transcript = std::fs::read_to_string(&transcript_path).expect("read hook transcript");

    for snapshot in [&unrelated_before, &bound_before, &consumed_before, &retry] {
        assert!(!transcript.contains(&snapshot.capability_id));
        assert!(!transcript.contains(&snapshot.token_hash));
    }
    for name in ["cas.db", "cas.db-wal", "cas.db-shm"] {
        let path = cas_root.join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            for raw in [
                target_tool,
                unrelated_tool,
                bound_tool,
                consumed_tool,
                retry_tool,
                "public-cleanup-transcript-marker",
            ] {
                assert!(
                    !bytes
                        .windows(raw.len())
                        .any(|window| window == raw.as_bytes()),
                    "raw hook correlation or prompt leaked into {name}: {raw}"
                );
            }
        }
    }
}
