//! Live conformance probe for the Codex factory launch contract.
//!
//! This deliberately uses the real `PtyConfig::codex` production path and a
//! real Codex CLI, while pinning `CAS_ROOT` to a disposable directory. It is
//! ignored in normal CI because it requires Codex authentication and model
//! traffic.
//!
//! Run from a tree whose freshly-built `cas` is first on `PATH`:
//! `cargo test -p cas-mux --test codex_factory_contract_runtime -- --ignored --nocapture`

#[path = "support/real_pty_serial.rs"]
mod real_pty_serial;

use cas_mux::{Mux, Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const PANE: &str = "cas-1c66-codex-contract";
const MODEL: &str = "gpt-5.6-terra";
const EFFORT: &str = "xhigh";

fn codex_0149_available() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .output()
        .map(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout).contains("codex-cli 0.149.1")
        })
        .unwrap_or(false)
}

fn git_init(path: &Path) {
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(path)
        .status()
        .expect("run git init");
    assert!(status.success(), "initialize isolated probe repository");
}

fn cas_init(root: &Path) {
    let output = Command::new("cas")
        .args(["init", "--yes", "--no-integrations", "--allow-non-project"])
        .current_dir(root)
        .output()
        .expect("run cas init");
    assert!(
        output.status.success(),
        "initialize isolated CAS root: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_discovery_fixtures(root: &Path) {
    std::fs::write(
        root.join("AGENTS.md"),
        "# Probe instructions\n\nFactory contract marker: CAS-1C66-AGENTS.\n",
    )
    .expect("write AGENTS.md");
    let skill = root.join(".codex/skills/cas-1c66-probe");
    std::fs::create_dir_all(&skill).expect("create skill fixture");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: cas-1c66-probe\ndescription: Live factory contract marker skill.\n---\n\
         When named, include CAS-1C66-SKILL in the response.\n",
    )
    .expect("write skill fixture");
    let agents = root.join(".codex/agents");
    std::fs::create_dir_all(&agents).expect("create agent fixture");
    std::fs::write(
        agents.join("cas-1c66-probe.md"),
        "# CAS 1c66 probe agent\n\nCatalog marker: CAS-1C66-AGENT.\n",
    )
    .expect("write agent fixture");
}

fn codex_sessions_root() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME")).join(".codex/sessions")
}

fn jsonl_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            jsonl_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

fn find_rollout_containing(needle: &str, deadline: Instant) -> Option<PathBuf> {
    while Instant::now() < deadline {
        let mut candidates = Vec::new();
        jsonl_files(&codex_sessions_root(), &mut candidates);
        candidates.sort_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        for path in candidates.into_iter().rev().take(30) {
            if std::fs::read_to_string(&path).is_ok_and(|body| body.contains(needle)) {
                return Some(path);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn read_rollout(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read Codex rollout")
}

fn completed_turns(body: &str) -> usize {
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["type"] == "event_msg" && event["payload"]["type"] == "task_complete")
        .count()
}

fn matching_tool_calls(body: &str, name: &str, action: &str) -> usize {
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            if event["type"] != "response_item"
                || event["payload"]["type"] != "function_call"
                || event["payload"]["name"] != name
            {
                return false;
            }
            event["payload"]["arguments"]
                .as_str()
                .and_then(|args| serde_json::from_str::<Value>(args).ok())
                .is_some_and(|args| args["action"] == action)
        })
        .count()
}

fn matching_custom_tool_calls(body: &str, name: &str) -> usize {
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["type"] == "response_item"
                && event["payload"]["type"] == "custom_tool_call"
                && event["payload"]["name"] == name
        })
        .count()
}

fn assistant_text(body: &str) -> String {
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["type"] == "response_item"
                && event["payload"]["type"] == "message"
                && event["payload"]["role"] == "assistant"
        })
        .flat_map(|event| {
            event["payload"]["content"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|content| content["text"].as_str().map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn assistant_text_does_not_accept_markers_from_user_prompts() {
    let body = [
        serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "FALSE-POSITIVE-MARKER"}
        }),
        serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "REAL-ASSISTANT-MARKER"}]
            }
        }),
    ]
    .into_iter()
    .map(|event| event.to_string())
    .collect::<Vec<_>>()
    .join("\n");

    let text = assistant_text(&body);
    assert!(!text.contains("FALSE-POSITIVE-MARKER"));
    assert!(text.contains("REAL-ASSISTANT-MARKER"));
}

fn assert_turn_context(body: &str, scratch: &Path) {
    let contexts: Vec<Value> = body
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["type"] == "turn_context")
        .map(|event| event["payload"].clone())
        .collect();
    assert!(
        contexts.len() >= 3,
        "root, follow-up, and resumed turns must all record context"
    );
    for context in contexts.iter().take(3) {
        assert_eq!(context["cwd"], scratch.to_string_lossy().as_ref());
        assert_eq!(context["model"], MODEL);
        assert_eq!(context["effort"], EFFORT);
        assert_eq!(
            context["approval_policy"], "never",
            "--yolo approval bypass must survive every turn"
        );
        assert_eq!(context["sandbox_policy"]["type"], "danger-full-access");
    }
}

fn drain(mux: &mut Mux, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let _ = mux.poll_batch();
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_for_completions(mux: &mut Mux, rollout: &Path, wanted: usize, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut body = String::new();
    while Instant::now() < deadline {
        let _ = mux.poll_batch();
        body = read_rollout(rollout);
        if completed_turns(&body) >= wanted {
            return body;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!(
        "timed out waiting for {wanted} completed turns; rollout tail:\n{}",
        body.lines().rev().take(30).collect::<Vec<_>>().join("\n")
    );
}

#[test]
#[ignore = "requires real Codex 0.149.1, authentication, and model traffic"]
fn codex_0149_factory_launch_contract_passes_live_matrix() {
    let _serial = real_pty_serial::lock();
    assert!(
        codex_0149_available(),
        "this receipt is valid only when run against codex-cli 0.149.1"
    );

    let scratch =
        std::env::temp_dir().join(format!("cas-1c66-codex-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create isolated probe repository");
    git_init(&scratch);
    cas_init(&scratch);
    write_discovery_fixtures(&scratch);
    let cas_root = scratch.join(".cas");

    let config = PtyConfig::codex(
        PANE,
        "worker",
        scratch.clone(),
        Some(&cas_root),
        Some("cas-1c66-supervisor"),
        Some("codex"),
        Some(MODEL),
        Some(EFFORT),
        None,
    );
    assert!(config.args.iter().any(|arg| arg == "--yolo"));
    assert!(config.args.iter().any(|arg| arg == "--no-alt-screen"));
    assert!(
        config
            .args
            .windows(2)
            .any(|pair| pair == ["--model", MODEL])
    );
    assert!(
        config
            .args
            .windows(2)
            .any(|pair| pair == ["-c", "model_reasoning_effort=xhigh"])
    );
    assert!(
        config
            .args
            .iter()
            .any(|arg| arg.contains("developer_instructions=")
                && arg.contains("Cassy Factory Worker"))
    );
    for (key, expected) in [
        ("CAS_AGENT_NAME", PANE),
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_FACTORY_MODE", "1"),
        ("CAS_FACTORY_WORKER_CLI", "codex"),
        ("CAS_FACTORY_WORKER_MODEL", MODEL),
        ("CAS_FACTORY_WORKER_EFFORT", EFFORT),
    ] {
        assert!(
            config
                .env
                .iter()
                .any(|(k, value)| k == key && value == expected),
            "production launch must export {key}={expected}"
        );
    }
    assert!(
        config
            .env
            .iter()
            .any(|(key, value)| key == "CAS_ROOT" && value == cas_root.to_string_lossy().as_ref()),
        "live probe must be pinned to its disposable CAS root"
    );
    assert!(
        !config.args.iter().any(|arg| arg.contains("rollout_token")),
        "factory launch must not inherit a low rollout-token budget"
    );
    assert!(
        config
            .args
            .iter()
            .any(|arg| { arg == "features.code_mode.direct_only_tool_namespaces=[\"mcp__cs\"]" }),
        "production launch must expose CAS as direct tools under Codex code mode"
    );
    assert!(
        config.args.iter().any(|arg| {
            arg == &format!(
                "mcp_servers.cs.env.CAS_ROOT={}",
                serde_json::to_string(&cas_root.to_string_lossy()).unwrap()
            )
        }),
        "production launch must pin the restricted MCP subprocess to the disposable CAS root"
    );
    assert!(
        !config
            .args
            .iter()
            .any(|arg| arg.contains("code_mode") && arg.contains("false")),
        "production launch must not disable supported Codex code mode"
    );

    let pty = Pty::spawn(PANE, config).expect("spawn real production Codex PTY");
    let pane = Pane::with_pty(PANE, PaneKind::Worker, pty, 24, 80, SupervisorCli::Codex)
        .expect("wrap Codex PTY");
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    // Drain startup and accept a repository-trust prompt if this fresh path
    // receives one. The production startup prompt then performs whoami + mine.
    drain(&mut mux, Duration::from_secs(10));
    runtime
        .block_on(mux.get(PANE).expect("pane").write(b"\r"))
        .ok();
    let rollout = find_rollout_containing(
        scratch.to_str().expect("UTF-8 scratch path"),
        Instant::now() + Duration::from_secs(20),
    )
    .expect("find rollout for isolated production cwd");
    let first = wait_for_completions(&mut mux, &rollout, 1, Duration::from_secs(60));
    assert!(
        matching_tool_calls(&first, "coordination", "whoami") >= 1,
        "root startup turn must call the injected coordination tool"
    );
    assert!(
        matching_tool_calls(&first, "task", "mine") >= 1,
        "root startup turn must call the injected task tool"
    );

    runtime
        .block_on(mux.inject(
            PANE,
            "Use $cas-1c66-probe. Call coordination whoami and task mine again. \
             Also use the code-mode exec tool to calculate 146 + 1. \
             Then reply with CAS-1C66-FOLLOWUP, CAS-1C66-AGENTS, CAS-1C66-SKILL, \
             the calculation result, and whether .codex/agents/cas-1c66-probe.md exists.",
        ))
        .expect("inject follow-up");
    let second = wait_for_completions(&mut mux, &rollout, 2, Duration::from_secs(60));
    assert!(matching_tool_calls(&second, "coordination", "whoami") >= 2);
    assert!(matching_tool_calls(&second, "task", "mine") >= 2);
    assert!(
        matching_custom_tool_calls(&second, "exec") >= 1,
        "CAS direct tools and the Codex code-mode exec tool must coexist"
    );
    let second_assistant_text = assistant_text(&second);
    for marker in [
        "CAS-1C66-FOLLOWUP",
        "CAS-1C66-AGENTS",
        "CAS-1C66-SKILL",
        "cas-1c66-probe.md",
    ] {
        assert!(
            second_assistant_text.contains(marker),
            "assistant response must prove discovery marker {marker}"
        );
    }

    runtime
        .block_on(mux.inject(
            PANE,
            "Write a long explanation of terminal multiplexing. Keep working until interrupted.",
        ))
        .expect("start interruptible turn");
    std::thread::sleep(Duration::from_millis(1200));
    runtime
        .block_on(mux.interrupt_and_inject(
            PANE,
            "Message from supervisor: stop and reply CAS-1C66-RESUMED.",
            Duration::from_millis(1200),
        ))
        .expect("interrupt and resume through production mux path");
    let final_body = wait_for_completions(&mut mux, &rollout, 3, Duration::from_secs(60));
    assert!(
        assistant_text(&final_body).contains("CAS-1C66-RESUMED"),
        "interrupted worker must resume and complete the redirected turn"
    );
    assert!(
        !final_body
            .to_ascii_lowercase()
            .contains("rollout token budget exceeded"),
        "multi-turn worker must not abort under a low rollout-token budget"
    );
    assert_turn_context(&final_body, &scratch);

    eprintln!(
        "PASS codex-cli 0.149.1 factory contract; isolated_root={}; rollout={}",
        cas_root.display(),
        rollout.display()
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
