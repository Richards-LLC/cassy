//! Live conformance probe for the Grok Build factory launch contract.
//!
//! This uses the real `PtyConfig::grok` production path, a real Grok CLI, a
//! disposable repository/CAS store, and the authenticated host Grok home.
//! Claude/Cursor-compatible hooks are explicitly disabled for the process while
//! persistent MCP discovery remains enabled.
//!
//! Run from a tree whose freshly-built `cas` is first on `PATH`:
//! `cargo test -p cas-mux --test grok_factory_contract_runtime -- --ignored --nocapture`

#[path = "support/real_pty_serial.rs"]
mod real_pty_serial;

use cas_mux::{Mux, Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PANE: &str = "cas-9bd9-grok-contract";
const MODEL: &str = "grok-4.5";
const EFFORT: &str = "medium";
const PROBE_FILE: &str = "GROK_FACTORY_PROBE.md";
const PROBE_MARKER: &str = "CAS-9BD9-GROK-FACTORY-PASS";

fn is_grok_02114(binary: &Path) -> bool {
    Command::new(binary)
        .arg("--version")
        .output()
        .is_ok_and(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout)
                    .contains("grok 0.2.114 (0c78503879) [stable]")
        })
}

fn grok_02114_binary() -> Option<PathBuf> {
    let path_binary = PathBuf::from("grok");
    if is_grok_02114(&path_binary) {
        return Some(path_binary);
    }

    let grok_home = PathBuf::from(std::env::var("HOME").ok()?).join(".grok");
    std::fs::read_dir(grok_home.join("downloads"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("grok-0.2.114-"))
                && is_grok_02114(path)
        })
}

fn run(mut command: Command, purpose: &str) -> std::process::Output {
    let output = command.output().unwrap_or_else(|error| {
        panic!("{purpose}: failed to execute command: {error}");
    });
    assert!(
        output.status.success(),
        "{purpose}: exit={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new("git");
    command.args(args).current_dir(root);
    run(command, &format!("git {}", args.join(" ")))
}

fn initialize_probe(root: &Path, cas_root: &Path) {
    std::fs::create_dir_all(root).expect("create isolated probe repository");
    git(
        root,
        &["init", "-q", "-b", "factory/cas-9bd9-grok-contract"],
    );
    git(
        root,
        &["config", "user.email", "grok-contract@example.invalid"],
    );
    git(root, &["config", "user.name", "Grok Contract Probe"]);
    std::fs::write(
        root.join("README.md"),
        "# Isolated Grok factory contract probe\n",
    )
    .expect("write probe readme");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-q", "-m", "chore: initialize probe"]);

    let mut init = Command::new("cas");
    init.args(["init", "-y", "--no-integrations"])
        .current_dir(root)
        .env("CAS_ROOT", cas_root);
    run(init, "initialize isolated CAS store");
    git(root, &["add", "-A"]);
    git(
        root,
        &["commit", "-q", "-m", "chore: initialize isolated cas"],
    );

    let hook_dir = root.join(".claude");
    std::fs::create_dir_all(&hook_dir).expect("create hook canary directory");
    let hook_canary = root.join("CLAUDE_COMPAT_HOOK_RAN");
    std::fs::write(
        hook_dir.join("settings.json"),
        serde_json::to_vec_pretty(&json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": format!("touch {}", hook_canary.display())
                    }]
                }]
            }
        }))
        .expect("serialize hook canary"),
    )
    .expect("write disabled Claude hook canary");
    git(root, &["add", ".claude/settings.json"]);
    git(
        root,
        &["commit", "-q", "-m", "test: add disabled hook canary"],
    );
}

struct McpClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(root: &Path, cas_root: &Path, name: &str, role: &str) -> Self {
        let mut child = Command::new("cas")
            .arg("serve")
            .current_dir(root)
            .env("CAS_ROOT", cas_root)
            .env("CAS_AGENT_NAME", name)
            .env("CAS_AGENT_ROLE", role)
            .env("CAS_FACTORY_MODE", "1")
            .env(
                "CAS_SESSION_ID",
                format!("00000000-0000-4000-8000-{:012x}", std::process::id()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn isolated cas serve");
        let stdin = child.stdin.take().expect("cas stdin");
        let stdout = BufReader::new(child.stdout.take().expect("cas stdout"));
        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let response = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "grok-contract-seeder", "version": "1"}
            }),
        );
        assert!(
            response.get("error").is_none(),
            "MCP initialize: {response}"
        );
        writeln!(
            client.stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            })
        )
        .expect("write initialized notification");
        client
            .stdin
            .flush()
            .expect("flush initialized notification");
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .expect("write MCP request");
        self.stdin.flush().expect("flush MCP request");
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read MCP response");
            assert!(!line.is_empty(), "cas serve closed before response {id}");
            let response: Value = serde_json::from_str(&line).expect("parse MCP response");
            if response["id"] == id {
                return response;
            }
        }
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn result_text(response: &Value) -> String {
    assert!(
        response.get("error").is_none(),
        "MCP request failed: {response}"
    );
    response["result"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_task_id(text: &str) -> String {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .find(|word| {
            word.len() == 8
                && word.starts_with("cas-")
                && word[4..].chars().all(|ch| ch.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| panic!("task create response lacks an id: {text}"))
        .to_string()
}

fn seed_assigned_task(root: &Path, cas_root: &Path) -> String {
    let mut client = McpClient::spawn(root, cas_root, "cas-9bd9-live-supervisor", "supervisor");
    let response = client.tool(
        "task",
        json!({
            "action": "create",
            "title": "Grok 0.2.114 isolated worker lifecycle probe",
            "description": "Create and commit the requested probe file, then leave a progress note.",
            "acceptance_criteria": "Task is started; probe file is committed; progress note is recorded.",
            "priority": 2,
            "task_type": "chore",
            "depth": "light",
            "assignee": PANE
        }),
    );
    extract_task_id(&result_text(&response))
}

fn find_session_dir(
    mux: &mut Mux,
    grok_home: &Path,
    session_id: &str,
    deadline: Instant,
) -> Option<PathBuf> {
    while Instant::now() < deadline {
        let _ = mux.poll_batch();
        let sessions = grok_home.join("sessions");
        if let Ok(entries) = std::fs::read_dir(&sessions) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(session_id);
                if candidate.join("events.jsonl").is_file() {
                    return Some(candidate);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn pane_text(pane: &Pane) -> String {
    let Ok(snapshot) = pane.get_full_snapshot() else {
        return "<terminal snapshot unavailable>".to_string();
    };
    snapshot
        .cells
        .chunks(snapshot.cols as usize)
        .map(|row| {
            row.iter()
                .filter_map(|cell| char::from_u32(cell.codepoint))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for_turn_end(mux: &mut Mux, events: &Path, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut body = String::new();
    while Instant::now() < deadline {
        let _ = mux.poll_batch();
        body = std::fs::read_to_string(events).unwrap_or_default();
        if body.lines().any(|line| {
            serde_json::from_str::<Value>(line)
                .is_ok_and(|event| event["type"] == "turn_ended" && event["outcome"] == "completed")
        }) {
            return body;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!(
        "timed out waiting for Grok turn completion; events tail:\n{}",
        body.lines().rev().take(30).collect::<Vec<_>>().join("\n")
    );
}

fn event_exists(body: &str, predicate: impl Fn(&Value) -> bool) -> bool {
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|event| predicate(&event))
}

#[test]
#[ignore = "requires real Grok Build 0.2.114, authentication, and model traffic"]
fn grok_02114_factory_launch_contract_passes_live_matrix() {
    let _serial = real_pty_serial::lock();
    let grok_binary = grok_02114_binary()
        .expect("this receipt is valid only when an exact Grok Build 0.2.114 binary is installed");

    let scratch =
        std::env::temp_dir().join(format!("cas-9bd9-grok-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let cas_root = scratch.join(".cas");
    let grok_home = PathBuf::from(std::env::var("HOME").expect("HOME")).join(".grok");
    initialize_probe(&scratch, &cas_root);
    let task_id = seed_assigned_task(&scratch, &cas_root);
    let probe_bin = scratch.join(".git/cas-probe-bin");
    std::fs::create_dir_all(&probe_bin).expect("create test-local binary directory");
    std::os::unix::fs::symlink(
        std::fs::canonicalize(&grok_binary).expect("canonical retained Grok binary"),
        probe_bin.join("grok"),
    )
    .expect("link exact Grok 0.2.114 into test-local PATH");

    let mut inspect = Command::new(&grok_binary);
    inspect
        .arg("inspect")
        .arg("--json")
        .current_dir(&scratch)
        .env("GROK_CLAUDE_HOOKS_ENABLED", "false")
        .env("GROK_CURSOR_HOOKS_ENABLED", "false");
    let inspect: Value =
        serde_json::from_slice(&run(inspect, "inspect isolated Grok config").stdout)
            .expect("parse grok inspect JSON");
    assert!(
        inspect["hooks"].as_array().is_none_or(|hooks| {
            hooks
                .iter()
                .all(|hook| hook["disabled"] == true && hook["compatibilityStatus"] == "disabled")
        }),
        "every discovered Claude/Cursor-compatible hook must be explicitly disabled: {}",
        inspect["hooks"]
    );
    let cas_server = inspect["mcpServers"]
        .as_array()
        .expect("inspect MCP servers")
        .iter()
        .find(|server| server["name"] == "cas")
        .expect("native persistent cas server");
    assert_eq!(cas_server["target"], "cas");
    assert!(
        ["claudeJson", "configToml"].contains(
            &cas_server["source"]["type"]
                .as_str()
                .expect("persistent MCP source type")
        ),
        "cas must come from a persistent user/project config: {cas_server}"
    );

    let mut config = PtyConfig::grok(
        PANE,
        "worker",
        scratch.clone(),
        Some(&cas_root),
        Some("cas-9bd9-live-supervisor"),
        Some("grok"),
        Some(MODEL),
        Some(EFFORT),
        None,
    );
    assert!(
        config.command == "grok"
            || (config.command == "nice" && config.args.iter().any(|arg| arg == "grok")),
        "production launch contract must continue to select Grok by name: command={} args={:?}",
        config.command,
        config.args
    );
    config.env.extend([
        (
            "PATH".to_string(),
            format!(
                "{}:{}",
                probe_bin.display(),
                std::env::var("PATH").expect("PATH")
            ),
        ),
        ("GROK_CLAUDE_HOOKS_ENABLED".to_string(), "false".to_string()),
        ("GROK_CURSOR_HOOKS_ENABLED".to_string(), "false".to_string()),
    ]);
    let session_id = config
        .env
        .iter()
        .find(|(key, _)| key == "CAS_SESSION_ID")
        .map(|(_, value)| value.clone())
        .expect("production config exports session id");
    assert!(
        session_id.len() == 36
            && [8, 13, 18, 23]
                .into_iter()
                .all(|index| session_id.as_bytes()[index] == b'-')
            && session_id
                .chars()
                .enumerate()
                .all(|(index, ch)| [8, 13, 18, 23].contains(&index) || ch.is_ascii_hexdigit()),
        "production session id must be a bare UUID: {session_id}"
    );
    for pair in [
        ["--permission-mode", "bypassPermissions"],
        ["--session-id", session_id.as_str()],
        ["--cwd", scratch.to_str().expect("UTF-8 scratch")],
        ["-m", MODEL],
        ["--reasoning-effort", EFFORT],
    ] {
        assert!(
            config
                .args
                .windows(2)
                .any(|args| args[0] == pair[0] && args[1] == pair[1]),
            "production argv must contain {} {}",
            pair[0],
            pair[1]
        );
    }
    let rules = config
        .args
        .windows(2)
        .find(|args| args[0] == "--rules")
        .map(|args| &args[1])
        .expect("production config exports worker rules");
    assert!(rules.contains("CAS Factory Worker"));
    assert!(rules.contains("cas__task") && rules.contains("cas__coordination"));
    for (key, expected) in [
        ("CAS_AGENT_NAME", PANE),
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_FACTORY_MODE", "1"),
        ("CAS_FACTORY_WORKER_CLI", "grok"),
        ("CAS_FACTORY_WORKER_MODEL", MODEL),
        ("CAS_FACTORY_WORKER_EFFORT", EFFORT),
    ] {
        assert!(
            config
                .env
                .iter()
                .any(|(actual_key, value)| actual_key == key && value == expected),
            "production launch must export {key}={expected}"
        );
    }

    let pty = Pty::spawn(PANE, config).expect("spawn real production Grok PTY");
    let pane = Pane::with_pty(PANE, PaneKind::Worker, pty, 24, 80, SupervisorCli::Grok)
        .expect("wrap Grok PTY");
    pane.set_harness_session_id(session_id.clone());
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    let trust_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < trust_deadline {
        let _ = mux.poll_batch();
        if pane_text(mux.get(PANE).expect("pane"))
            .contains("Do you trust the contents of this directory?")
        {
            runtime
                .block_on(mux.get(PANE).expect("pane").write(b"y\r"))
                .expect("accept isolated disposable directory");
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let session_dir = find_session_dir(
        &mut mux,
        &grok_home,
        &session_id,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap_or_else(|| {
        let pane = mux.get(PANE).expect("pane");
        panic!(
            "Grok transcript must use GROK_HOME/sessions/*/<session-id>; \
             exited={} exit_code={:?}; terminal:\n{}",
            pane.has_exited(),
            pane.exit_code(),
            pane_text(pane)
        )
    });
    runtime
        .block_on(mux.inject(
            PANE,
            &format!(
                "Execute the assigned isolated validation task {task_id}. Use the CAS tools \
                 exactly as your worker rules require: coordination whoami, task mine, task \
                 show, task start, then add a progress note. Create {PROBE_FILE} containing \
                 exactly `{PROBE_MARKER}` followed by a newline; git add and commit it with \
                 message `test: grok 0.2.114 worker lifecycle`. Do not push or close the task. \
                 Finish with the marker {PROBE_MARKER}."
            ),
        ))
        .expect("inject isolated lifecycle assignment");
    let events = wait_for_turn_end(
        &mut mux,
        &session_dir.join("events.jsonl"),
        Duration::from_secs(180),
    );
    mux.get(PANE).expect("pane").refresh_harness_turn_state();
    assert!(
        !mux.get(PANE).expect("pane").is_turn_in_flight(),
        "Grok turn_ended transcript signal must clear liveness"
    );

    assert!(event_exists(&events, |event| {
        event["type"] == "turn_started"
            && event["session_id"] == session_id
            && event["model_id"] == MODEL
            && event["yolo_mode"] == true
    }));
    assert!(event_exists(&events, |event| {
        event["type"] == "mcp_config_resolved"
            && event["servers"]
                .as_array()
                .is_some_and(|servers| servers.iter().any(|server| server["name"] == "cas"))
    }));
    assert!(event_exists(&events, |event| {
        event["type"] == "mcp_server_connected"
            && event["server_name"] == "cas"
            && event["tool_count"]
                .as_u64()
                .is_some_and(|count| count >= 11)
            && event["tools"].as_array().is_some_and(|tools| {
                ["coordination", "task"]
                    .iter()
                    .all(|name| tools.iter().any(|tool| tool == name))
            })
    }));
    assert!(event_exists(&events, |event| {
        event["type"] == "mcp_tool_call_completed"
            && event["server_name"] == "cas"
            && event["success"] == true
    }));

    let prompt_context: Value =
        serde_json::from_slice(&std::fs::read(session_dir.join("prompt_context.json")).unwrap())
            .expect("parse Grok prompt context");
    assert_eq!(
        prompt_context["working_directory"],
        scratch.to_string_lossy().as_ref()
    );
    let system_prompt = std::fs::read_to_string(session_dir.join("system_prompt.txt"))
        .expect("read Grok system prompt");
    assert!(system_prompt.contains("CAS Factory Worker"));
    assert!(system_prompt.contains("cas__task") && system_prompt.contains("cas__coordination"));

    let chat = std::fs::read_to_string(session_dir.join("chat_history.jsonl"))
        .expect("read Grok chat history");
    for action in ["whoami", "mine", "show", "start", "notes"] {
        assert!(
            chat.contains(&format!("\\\"action\\\":\\\"{action}\\\"")),
            "chat transcript must prove CAS action={action}"
        );
    }
    assert!(chat.contains("\\\"tool_name\\\":\\\"cas__coordination\\\""));
    assert!(chat.contains("\\\"tool_name\\\":\\\"cas__task\\\""));
    assert!(chat.contains(PROBE_MARKER));
    assert!(
        chat.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|entry| entry["type"] == "assistant"
                && entry["model_id"]
                    .as_str()
                    .is_some_and(|model| model.starts_with("grok-4.5"))
                && entry["reasoning_effort"] == EFFORT)
    );

    assert_eq!(
        std::fs::read_to_string(scratch.join(PROBE_FILE)).expect("probe file"),
        format!("{PROBE_MARKER}\n")
    );
    let log = String::from_utf8(git(&scratch, &["log", "-1", "--pretty=%s"]).stdout).unwrap();
    assert_eq!(log.trim(), "test: grok 0.2.114 worker lifecycle");
    assert!(
        String::from_utf8(git(&scratch, &["status", "--porcelain"]).stdout)
            .unwrap()
            .trim()
            .is_empty(),
        "worker edit/commit lifecycle must leave the isolated repo clean"
    );
    assert!(
        !scratch.join("CLAUDE_COMPAT_HOOK_RAN").exists(),
        "disabled Claude-compatible SessionStart hook must never execute"
    );

    let mut observer = McpClient::spawn(
        &scratch,
        &cas_root,
        "cas-9bd9-live-supervisor",
        "supervisor",
    );
    let task = result_text(&observer.tool("task", json!({"action": "show", "id": task_id})));
    assert!(task.contains("Status: InProgress"));
    assert!(
        task.contains("live isolated Grok 0.2.114")
            || task.contains("isolated validation")
            || task.contains("progress"),
        "worker must persist a progress note through cas__task: {task}"
    );

    eprintln!(
        "PASS Grok Build 0.2.114 factory contract; task={task_id}; session={session_id}; \
         isolated transcript={}",
        session_dir.display()
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
