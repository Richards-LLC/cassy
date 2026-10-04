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
    cas_root: PathBuf,
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
        cas::store::open_agent_store(&cas_root)
            .unwrap()
            .register(&cas::types::Agent::new(
                "server-registry-test".to_string(),
                "operator".to_string(),
            ))
            .unwrap();
        Self {
            _temp: temp,
            workdir,
            cas_root,
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

    async fn factory_list(&self, fields: serde_json::Value) -> String {
        let mut req = fields;
        req["action"] = "server_list".into();
        let req: CoordinationRequest = serde_json::from_value(req).unwrap();
        match self.service.factory(Parameters(req)).await {
            Ok(result) => result
                .content
                .iter()
                .filter_map(|c| match &c.raw {
                    RawContent::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Err(error) => format!("MCP_ERROR: {error}"),
        }
    }

    fn seed_history(&self) {
        let dir = self.cas_root.join("factory-servers");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..125 {
            let record = serde_json::json!({
                "id": format!("srv-history-{i}"),
                "name": format!("history-{i}"),
                "command": "界\n".repeat(2000),
                "cwd": "/repo/app",
                "pid": i32::MAX,
                "pid_starttime": 1,
                "owner_task": if i % 2 == 0 { "cas-a" } else { "cas-b" },
                "owner_worker": if i % 3 == 0 { "alice" } else { "bob" },
                "owner_agent_id": if i % 3 == 0 { "agent-alice" } else { "agent-bob" },
                "shared": false,
                "started_at": "2026-01-01T00:00:00Z",
                "state": if i == 0 { "running" } else { "stopped" },
                "ended_at": chrono::Utc::now(),
            });
            std::fs::write(
                dir.join(format!("srv-history-{i}.json")),
                record.to_string(),
            )
            .unwrap();
        }
    }
}

#[tokio::test]
async fn cas_ced2_server_list_defaults_to_running_without_history() {
    let env = TestEnv::new();
    env.seed_history();
    let listed = env.factory_list(serde_json::json!({})).await;
    assert!(listed.contains("No servers currently running"), "{listed}");
    assert!(
        !listed.contains("history-"),
        "historical entries leaked into the default listing"
    );
    assert!(
        listed.len() < 1024,
        "default listing emitted {} bytes",
        listed.len()
    );
}

#[tokio::test]
async fn cas_ced2_server_list_filters_and_caps_history() {
    let env = TestEnv::new();
    env.seed_history();
    let rows = |text: &str| text.lines().filter(|line| line.starts_with("  ")).count();

    let all = env.factory_list(serde_json::json!({"status": "all"})).await;
    assert_eq!(rows(&all), 20, "{all}");
    assert!(all.contains("Showing 20 of 125 matches"), "{all}");

    let capped = env
        .factory_list(serde_json::json!({"status": "all", "limit": 10000}))
        .await;
    assert_eq!(rows(&capped), 50, "{capped}");
    assert!(
        capped.len() < 27000,
        "listing emitted {} bytes",
        capped.len()
    );
    assert!(
        capped
            .lines()
            .filter(|line| line.starts_with("  "))
            .all(|line| line.len() <= 512)
    );

    let filtered = env
        .factory_list(serde_json::json!({
            "status": " stopped ", "task_id": " cas-a ", "owner": " alice ", "limit": 3,
        }))
        .await;
    assert_eq!(rows(&filtered), 3, "{filtered}");
    for row in filtered.lines().filter(|line| line.starts_with("  ")) {
        assert!(
            row.contains("stopped") && row.contains("started by alice for cas-a"),
            "{row}"
        );
    }
    assert!(filtered.contains("Showing 3 of 20 matches"), "{filtered}");

    let by_id = env
        .factory_list(serde_json::json!({"status": "all", "owner": "agent-bob", "limit": 1}))
        .await;
    assert_eq!(rows(&by_id), 1, "{by_id}");
    assert!(by_id.contains("started by bob"), "{by_id}");

    let dead = env
        .factory_list(serde_json::json!({"status": "dead"}))
        .await;
    assert_eq!(rows(&dead), 1, "{dead}");
    assert!(
        dead.contains("history-0") && dead.contains("dead"),
        "{dead}"
    );

    let none = env
        .factory_list(serde_json::json!({"status": "all", "owner": "missing"}))
        .await;
    assert_eq!(rows(&none), 0, "{none}");
    assert!(none.contains("No registered servers"), "{none}");

    let invalid = env
        .factory_list(serde_json::json!({"status": "typo"}))
        .await;
    assert!(
        invalid.contains("MCP_ERROR") && invalid.contains("status must be"),
        "{invalid}"
    );
    let zero = env.factory_list(serde_json::json!({"limit": 0})).await;
    assert!(
        zero.contains("MCP_ERROR") && zero.contains("greater than zero"),
        "{zero}"
    );
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
    assert!(
        started.contains("pgid:"),
        "the group id is recorded: {started}"
    );

    let listed = env.call(serde_json::json!({"action": "server_list"})).await;
    assert!(listed.contains("Running servers (1)"), "{listed}");
    assert!(listed.contains("dev-web"), "{listed}");
    assert!(listed.contains(&id), "{listed}");
    assert!(listed.contains("cas-7c93"), "owner task shown: {listed}");
    assert!(listed.contains("sleep 300"), "command shown: {listed}");
    assert!(listed.contains("pgid"), "process group shown: {listed}");
    assert!(
        listed.contains("live descendants"),
        "live descendant count shown: {listed}"
    );

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
    assert!(!after.contains(&id), "history is opt-in: {after}");
    let history = env
        .call(serde_json::json!({"action": "server_list", "status": "all"}))
        .await;
    assert!(
        history.contains(&id) && history.contains("stopped"),
        "history remains available: {history}"
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

/// Registered identity controls both factory and legacy coordination routes.
#[tokio::test]
async fn cas_9723_worker_stops_owned_server_but_not_another_workers() {
    use cas::types::{Agent, AgentRole};
    let env = TestEnv::new();
    let mut alice = Agent::new_with_role("alice-id".into(), "alice".into(), AgentRole::Worker);
    alice.factory_session = Some("session-a".into());
    let mut bob = Agent::new_with_role("bob-id".into(), "bob".into(), AgentRole::Worker);
    bob.factory_session = Some("session-a".into());
    let service = |agent: &Agent| {
        let core = CasCore::with_daemon(env.cas_root.clone(), None, None);
        cas::store::open_agent_store(&env.cas_root)
            .unwrap()
            .register(agent)
            .unwrap();
        core.set_agent_id_for_testing(agent.id.clone());
        CasService::new(core, None)
    };
    let alice_service = service(&alice);
    let bob_service = service(&bob);
    let call = |svc: CasService, req: serde_json::Value| async move {
        let result = svc
            .factory(Parameters(serde_json::from_value(req).unwrap()))
            .await;
        match result {
            Ok(result) => result
                .content
                .iter()
                .filter_map(|c| match &c.raw {
                    RawContent::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Err(error) => format!("MCP_ERROR: {error}"),
        }
    };
    let started = call(
        alice_service.clone(),
        serde_json::json!({
            "action":"server_start", "id":"owned", "command":"sleep 300", "cwd":env.workdir,
        }),
    )
    .await;
    assert!(started.contains("Started server"), "{started}");
    let id = extract_id(&started);
    let refused = call(
        bob_service,
        serde_json::json!({"action":"server_stop", "id":id}),
    )
    .await;
    assert!(
        refused.contains("workers may stop only servers they started"),
        "{refused}"
    );
    let still_running = call(
        alice_service.clone(),
        serde_json::json!({"action":"server_list"}),
    )
    .await;
    assert!(
        still_running.contains("Running servers (1)"),
        "refusal must preserve the workload: {still_running}"
    );
    let stopped = call(
        alice_service,
        serde_json::json!({"action":"server_stop", "id":id}),
    )
    .await;
    assert!(stopped.contains("Stopped server"), "{stopped}");
}
