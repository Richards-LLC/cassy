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
    // GH #1368: disposable npm-exec -> shell -> server shape. The server
    // publishes its actual port; no package installation or fixed host port.
    let fixture = env.workdir.join("npm-exec.py");
    std::fs::write(&fixture, r#"import json, os, pathlib, socket, subprocess, sys, time
root = pathlib.Path(__file__).parent
if sys.argv[1] == "wrapper":
    (root / "wrapper.pid").write_text(str(os.getpid()))
    subprocess.run(["sh", "-c", 'printf "%s" "$$" > "$1"; "$2" "$3" server & wait',
                    "sh", str(root / "shell.pid"), sys.executable, __file__], check=True)
else:
    with socket.socket() as server:
        server.bind(("127.0.0.1", 0))
        server.listen()
        (root / "server.json").write_text(json.dumps({"pid": os.getpid(), "port": server.getsockname()[1]}))
        deadline = time.monotonic() + 60
        # Also clean up after a failed assertion removes the fixture directory.
        while pathlib.Path(__file__).exists() and time.monotonic() < deadline:
            time.sleep(0.025)
"#).unwrap();

    let started = env
        .call(serde_json::json!({
            "action": "server_start",
            "id": "dev-web",
            "command": "python3 npm-exec.py wrapper",
            "cwd": env.workdir.to_str().unwrap(),
            "task_id": "cas-7c93",
        }))
        .await;
    assert!(
        started.contains("Started server 'dev-web'"),
        "unexpected start output: {started}"
    );
    let id = extract_id(&started);
    let pid = extract_pid(&started);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let (wrapper, shell, server, port) = loop {
        let read_pid = |name: &str| std::fs::read_to_string(env.workdir.join(name))
            .ok()?.trim().parse::<u32>().ok();
        let ready = std::fs::read(env.workdir.join("server.json")).ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if let (Some(wrapper), Some(shell), Some(ready)) =
            (read_pid("wrapper.pid"), read_pid("shell.pid"), ready)
            && let (Some(server), Some(port)) = (ready["pid"].as_u64(), ready["port"].as_u64())
        {
            break (wrapper, shell, server as u32, port as u16);
        }
        if std::time::Instant::now() >= deadline {
            let cleanup = env.call(serde_json::json!({"action": "server_stop", "id": id})).await;
            panic!("nested fixture never became ready: {started}; cleanup: {cleanup}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let owned = [wrapper, shell, server];
    assert_eq!(pid, wrapper, "registered pid must be the npm-style wrapper");
    assert!(wrapper != shell && shell != server && wrapper != server);
    assert!(owned.iter().all(|pid| pid_alive(*pid)), "all three fixture processes must be live");
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(), "server must own a listening socket");
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
    assert!(listed.contains("npm-exec.py"), "command shown: {listed}");
    assert!(listed.contains("pgid"), "process group shown: {listed}");
    assert!(
        listed.contains("live descendants"),
        "live descendant count shown: {listed}"
    );

    let stopped = env
        .call(serde_json::json!({"action": "server_stop", "id": id}))
        .await;
    for _ in 0..80 {
        if owned.iter().all(|pid| !pid_alive(*pid)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let survivors: Vec<_> = owned.into_iter().filter(|pid| pid_alive(*pid)).map(|pid| {
        let command = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="])
            .output().map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned()).unwrap_or_default();
        format!("pid {pid} ({command})")
    }).collect();
    assert!(stopped.contains("Stopped server 'dev-web'") && survivors.is_empty(),
        "stop must reap every owned descendant; output: {stopped}; survivors: {survivors:?}");
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "server_stop must release the owned port");

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

#[tokio::test]
async fn cas_7694_start_uses_explicit_name() {
    let env = TestEnv::new();
    let started = env.call(serde_json::json!({"action":"server_start", "name":"explicit server", "command":"sleep 300", "cwd":env.workdir})).await;
    if started.contains("Started server") {
        let id = extract_id(&started);
        env.call(serde_json::json!({"action":"server_stop", "id":id}))
            .await;
    }
    assert!(
        started.contains("Started server 'explicit server'"),
        "{started}"
    );
}

#[tokio::test]
async fn cas_7694_immediate_exit_reports_failure_and_log_tail() {
    let env = TestEnv::new();
    let result = env.call(serde_json::json!({"action":"server_start", "command":"printf 'startup error\\n' >&2; exit 7", "cwd":env.workdir})).await;
    assert!(
        result.contains("MCP_ERROR") && result.contains("startup error"),
        "must report failure with diagnostic: {result}"
    );
    let listed = env.call(serde_json::json!({"action":"server_list"})).await;
    assert!(
        !listed.contains("Running servers (1)"),
        "failed launch must not stay running: {listed}"
    );
}

/// Registered identity controls both factory and legacy coordination routes.
#[tokio::test]
async fn cas_9723_worker_stops_owned_server_but_not_another_workers() {
    use cas::types::{Agent, AgentRole, Task, TaskStatus};
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

    // A context recycle preserves the name/session/task binding, releases
    // the old process's lease, and task start claims it for the new UUID.
    let agents = cas::store::open_agent_store(&env.cas_root).unwrap();
    let tasks = cas::store::open_task_store(&env.cas_root).unwrap();
    let mut task = Task::new("cas-recycle".into(), "Resume server work".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(alice.name.clone());
    tasks.add(&task).unwrap();
    assert!(
        agents
            .try_claim(&task.id, &alice.id, 600, None)
            .unwrap()
            .is_success()
    );
    let started = call(
        service(&alice),
        serde_json::json!({
            "action":"server_start", "id":"recycled", "command":"sleep 300",
            "cwd":env.workdir, "task_id":task.id, "shared":true,
        }),
    )
    .await;
    let id = extract_id(&started);
    agents.graceful_shutdown(&alice.id).unwrap();

    let mut replacement = Agent::new_with_role(
        "alice-recycled-id".into(),
        alice.name.clone(),
        AgentRole::Worker,
    );
    // Each identity gets the same live lease where indicated. Names alone,
    // another factory session, or another task must not authorize a stop.
    for (label, name, session, assigned, leased, allowed) in [
        (
            "different worker",
            "bob",
            Some("session-a"),
            "bob",
            true,
            false,
        ),
        (
            "different session",
            "alice",
            Some("session-b"),
            "alice",
            true,
            false,
        ),
        ("missing session", "alice", None, "alice", true, false),
        (
            "different task binding",
            "alice",
            Some("session-a"),
            "bob",
            true,
            false,
        ),
        (
            "no resumed lease",
            "alice",
            Some("session-a"),
            "alice",
            false,
            false,
        ),
        (
            "resumed owner",
            "alice",
            Some("session-a"),
            "alice",
            true,
            true,
        ),
    ] {
        replacement.name = name.into();
        replacement.factory_session = session.map(str::to_string);
        let svc = service(&replacement);
        task.assignee = Some(assigned.into());
        tasks.update(&task).unwrap();
        if leased {
            assert!(
                agents
                    .try_claim(&task.id, &replacement.id, 600, None)
                    .unwrap()
                    .is_success()
            );
        }
        let result = call(svc, serde_json::json!({"action":"server_stop", "id":id})).await;
        if leased {
            agents.release_lease(&task.id, &replacement.id).unwrap();
        }
        if allowed {
            // Clean up on the red path only. Re-stopping an already reaped
            // wrapper without a cgroup correctly refuses unverifiable legacy
            // descendants (as on macOS); it is not an ownership failure.
            let cleanup = if result.contains("Stopped server") {
                String::new()
            } else {
                env.call(serde_json::json!({"action":"server_stop", "id":id})).await
            };
            assert!(result.contains("Stopped server"), "{label}: {result}; cleanup: {cleanup}");
            assert!(!pid_alive(extract_pid(&started)), "{label} left its owned server alive");
        } else {
            assert!(
                result.contains("workers may stop only servers they started"),
                "{label}: {result}"
            );
            assert!(
                pid_alive(extract_pid(&started)),
                "{label} killed the server"
            );
        }
    }
}
