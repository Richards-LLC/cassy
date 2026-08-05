//! End-to-end coverage of the server registry MCP surface (cas-7c93, GH #87).
//!
//! Drives real `coordination action=server_*` calls against real processes:
//! register, query, stop. The point of the issue is a *lifecycle*, so nothing
//! here mocks the process side.

use std::path::PathBuf;

use cas::mcp::{CasCore, CasService};
use cas::store::init_cas_dir;
use cas_mcp::types::CoordinationRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

struct TestEnv {
    _temp: TempDir,
    workdir: PathBuf,
    service: CasService,
}

impl TestEnv {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let workdir = temp.path().join("project");
        std::fs::create_dir_all(&workdir).unwrap();
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing("server-registry-test".to_string());
        Self {
            _temp: temp,
            workdir,
            service: CasService::new(core, None),
        }
    }

    async fn call(&self, req: serde_json::Value) -> String {
        let req: CoordinationRequest = serde_json::from_value(req).expect("CoordinationRequest");
        match self.service.coordination(Parameters(req)).await {
            Ok(result) => result
                .content
                .iter()
                .filter_map(|c| match &c.raw {
                    RawContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Err(error) => format!("MCP_ERROR: {error}"),
        }
    }
}

fn extract_id(output: &str) -> String {
    output
        .split_whitespace()
        .find(|token| token.starts_with("srv-"))
        .map(|token| token.trim_end_matches(')').to_string())
        .unwrap_or_else(|| panic!("no server id in output: {output}"))
}

fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists() || {
        // SAFETY: signal 0 only probes for existence.
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, 0) == 0
        }
        #[cfg(not(unix))]
        false
    }
}

fn extract_pid(output: &str) -> u32 {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid: "))
        .and_then(|pid| pid.trim().parse().ok())
        .unwrap_or_else(|| panic!("no pid in output: {output}"))
}

/// AC1: start → list → stop, end to end through the MCP surface.
#[tokio::test]
async fn server_start_list_stop_round_trip() {
    let env = TestEnv::new();

    let started = env
        .call(serde_json::json!({
            "action": "server_start",
            "id": "dev-web",
            "command": "sleep 300",
            "cwd": env.workdir.to_str().unwrap(),
            "port": 5173,
            "task_id": "cas-7c93",
        }))
        .await;
    assert!(
        started.contains("Started server 'dev-web'"),
        "unexpected start output: {started}"
    );
    let id = extract_id(&started);
    let pid = extract_pid(&started);
    assert!(pid_alive(pid), "the server must actually be running");
    assert!(
        started.contains("dies at teardown"),
        "an unshared server must say it is contained: {started}"
    );

    let listed = env.call(serde_json::json!({"action": "server_list"})).await;
    assert!(listed.contains("Running servers (1)"), "{listed}");
    assert!(listed.contains("dev-web"), "{listed}");
    assert!(listed.contains(&id), "{listed}");
    assert!(listed.contains("cas-7c93"), "owner task shown: {listed}");
    assert!(listed.contains("sleep 300"), "command shown: {listed}");

    let stopped = env
        .call(serde_json::json!({"action": "server_stop", "id": id}))
        .await;
    assert!(stopped.contains("Stopped server 'dev-web'"), "{stopped}");

    for _ in 0..80 {
        if !pid_alive(pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!pid_alive(pid), "server_stop must actually stop it");

    let after = env.call(serde_json::json!({"action": "server_list"})).await;
    assert!(
        after.contains("No servers currently running"),
        "a stopped server must leave the running set: {after}"
    );
    assert!(
        after.contains("Recent history"),
        "and remain visible as history: {after}"
    );
}

/// The registry must be selectable by task, so a supervisor can ask "what did
/// this task leave running?".
#[tokio::test]
async fn server_list_filters_by_owning_task() {
    let env = TestEnv::new();
    for (name, task) in [("srv-a", "cas-1111"), ("srv-b", "cas-2222")] {
        env.call(serde_json::json!({
            "action": "server_start",
            "id": name,
            "command": "sleep 300",
            "cwd": env.workdir.to_str().unwrap(),
            "task_id": task,
        }))
        .await;
    }

    let filtered = env
        .call(serde_json::json!({"action": "server_list", "task_id": "cas-1111"}))
        .await;
    assert!(filtered.contains("srv-a"), "{filtered}");
    assert!(!filtered.contains("srv-b"), "{filtered}");

    let empty = env
        .call(serde_json::json!({"action": "server_list", "task_id": "cas-9999"}))
        .await;
    assert!(
        empty.contains("No registered servers for cas-9999"),
        "{empty}"
    );

    for name in ["srv-a", "srv-b"] {
        env.call(serde_json::json!({"action": "server_stop", "id": name}))
            .await;
    }
}

#[tokio::test]
async fn server_start_validates_its_inputs() {
    let env = TestEnv::new();

    let no_command = env
        .call(serde_json::json!({"action": "server_start"}))
        .await;
    assert!(no_command.contains("requires `command`"), "{no_command}");

    let bad_port = env
        .call(serde_json::json!({
            "action": "server_start",
            "command": "sleep 1",
            "cwd": env.workdir.to_str().unwrap(),
            "port": 70000,
        }))
        .await;
    assert!(bad_port.contains("outside 1-65535"), "{bad_port}");

    let bad_cwd = env
        .call(serde_json::json!({
            "action": "server_start",
            "command": "sleep 1",
            "cwd": "/definitely/not/here",
        }))
        .await;
    assert!(bad_cwd.contains("cwd does not exist"), "{bad_cwd}");
}

/// Starting the same named server twice must not silently orphan the first
/// one — that is how ambient duplicates on the same port happen.
#[tokio::test]
async fn starting_a_duplicate_name_is_refused_while_the_first_is_alive() {
    let env = TestEnv::new();
    let first = env
        .call(serde_json::json!({
            "action": "server_start",
            "id": "only-one",
            "command": "sleep 300",
            "cwd": env.workdir.to_str().unwrap(),
        }))
        .await;
    let pid = extract_pid(&first);

    let second = env
        .call(serde_json::json!({
            "action": "server_start",
            "id": "only-one",
            "command": "sleep 300",
            "cwd": env.workdir.to_str().unwrap(),
        }))
        .await;
    assert!(second.contains("already running"), "{second}");
    assert!(
        second.contains("server_stop"),
        "and says how to fix it: {second}"
    );
    assert!(pid_alive(pid), "the original must be untouched");

    env.call(serde_json::json!({"action": "server_stop", "id": "only-one"}))
        .await;
}

#[tokio::test]
async fn server_stop_reports_an_unknown_handle_instead_of_failing_silently() {
    let env = TestEnv::new();
    let out = env
        .call(serde_json::json!({"action": "server_stop", "id": "srv-nope"}))
        .await;
    assert!(
        out.contains("no registered server matches 'srv-nope'"),
        "{out}"
    );
    assert!(out.contains("server_list"), "{out}");

    let missing_id = env.call(serde_json::json!({"action": "server_stop"})).await;
    assert!(missing_id.contains("requires `id`"), "{missing_id}");
}

/// The empty listing is the teaching moment the issue asks for: it must point
/// at `server_start` rather than leaving `npm run dev &` as the obvious move.
#[tokio::test]
async fn an_empty_listing_teaches_the_sanctioned_path() {
    let env = TestEnv::new();
    let out = env.call(serde_json::json!({"action": "server_list"})).await;
    assert!(out.contains("No registered servers"), "{out}");
    assert!(out.contains("server_start"), "{out}");
    assert!(
        out.contains("npm run dev &"),
        "the unsanctioned pattern must be named as the thing not to do: {out}"
    );
    assert!(out.contains("shared=true"), "{out}");
}

/// A shared server is placed outside worker containment; the response has to
/// say so, because that is the whole reason to pass the flag.
#[tokio::test]
async fn shared_servers_announce_that_they_outlive_teardown() {
    let env = TestEnv::new();
    let started = env
        .call(serde_json::json!({
            "action": "server_start",
            "id": "shared-web",
            "command": "sleep 300",
            "cwd": env.workdir.to_str().unwrap(),
            "shared": true,
        }))
        .await;
    assert!(started.contains("survives worker teardown"), "{started}");

    let listed = env.call(serde_json::json!({"action": "server_list"})).await;
    assert!(
        listed.contains("[shared: survives worker teardown]"),
        "{listed}"
    );

    env.call(serde_json::json!({"action": "server_stop", "id": "shared-web"}))
        .await;
}
