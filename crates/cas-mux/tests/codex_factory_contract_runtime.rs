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

fn standard_codex_recipe() -> (String, String) {
    let registry: toml::Value =
        toml::from_str(include_str!("../../cas-factory/policy/lane-registry.toml"))
            .expect("parse factory lane registry");
    let candidates = registry["lanes"]["standard"]["candidates"]
        .as_array()
        .expect("standard lane candidates");
    let recipe = candidates
        .iter()
        .filter_map(toml::Value::as_str)
        .map(|name| &registry["recipes"][name])
        .find(|recipe| {
            recipe["harness"].as_str() == Some("codex")
                && recipe["status"].as_str() == Some("active")
        })
        .expect("standard lane must have an active Codex recipe");
    (
        recipe["model"].as_str().expect("Codex model").to_owned(),
        recipe["default_effort"]
            .as_str()
            .expect("Codex effort")
            .to_owned(),
    )
}

#[test]
fn live_matrix_uses_active_standard_codex_recipe() {
    let (model, effort) = standard_codex_recipe();
    assert!(!model.is_empty());
    assert!(!effort.is_empty());
}

fn codex_0156_available() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .output()
        .map(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout).contains("codex-cli 0.156.0")
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

fn tool_call_outputs(body: &str, name: &str, action: &str) -> Vec<String> {
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let call_ids: Vec<&str> = events
        .iter()
        .filter(|event| {
            event["type"] == "response_item"
                && event["payload"]["type"] == "function_call"
                && event["payload"]["name"] == name
                && event["payload"]["arguments"]
                    .as_str()
                    .and_then(|args| serde_json::from_str::<Value>(args).ok())
                    .is_some_and(|args| args["action"] == action)
        })
        .filter_map(|event| event["payload"]["call_id"].as_str())
        .collect();
    events
        .iter()
        .filter(|event| {
            event["type"] == "response_item"
                && event["payload"]["type"] == "function_call_output"
                && event["payload"]["call_id"]
                    .as_str()
                    .is_some_and(|id| call_ids.contains(&id))
        })
        .filter_map(|event| event["payload"]["output"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn tool_call_outputs_only_match_server_responses_for_requested_action() {
    let body = [
        serde_json::json!({"type":"response_item","payload":{
            "type":"function_call","name":"task","call_id":"create-1",
            "arguments":"{\"action\":\"create\"}"}}),
        serde_json::json!({"type":"response_item","payload":{
            "type":"function_call_output","call_id":"create-1","output":"created"}}),
        serde_json::json!({"type":"response_item","payload":{
            "type":"function_call_output","call_id":"other","output":"unrelated"}}),
    ]
    .into_iter()
    .map(|event| event.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    assert_eq!(tool_call_outputs(&body, "task", "create"), ["created"]);
    assert!(tool_call_outputs(&body, "task", "show").is_empty());
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

fn assert_turn_context(body: &str, scratch: &Path, model: &str, effort: &str) {
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
        assert_eq!(context["model"], model);
        assert_eq!(context["effort"], effort);
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
#[ignore = "requires real Codex 0.156.0, authentication, and model traffic"]
fn codex_0156_factory_launch_contract_passes_live_matrix() {
    let _serial = real_pty_serial::lock();
    let (model, effort) = standard_codex_recipe();
    assert!(
        codex_0156_available(),
        "this receipt is valid only when run against codex-cli 0.156.0"
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
        Some(&model),
        Some(&effort),
        None,
    );
    assert!(config.args.iter().any(|arg| arg == "--yolo"));
    assert!(config.args.iter().any(|arg| arg == "--no-alt-screen"));
    assert!(
        config
            .args
            .windows(2)
            .any(|pair| pair == ["--model", model.as_str()])
    );
    let effort_config = format!("model_reasoning_effort={effort}");
    assert!(
        config
            .args
            .windows(2)
            .any(|pair| pair == ["-c", effort_config.as_str()])
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
        ("CAS_FACTORY_WORKER_MODEL", model.as_str()),
        ("CAS_FACTORY_WORKER_EFFORT", effort.as_str()),
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
             In this disposable CAS root, call task create with title=CAS-1C66-SCHEMA-TITLE, \
             description=CAS-1C66-SCHEMA-DESCRIPTION, design=CAS-1C66-SCHEMA-DESIGN, \
             acceptance_criteria=CAS-1C66-SCHEMA-ACCEPTANCE, labels=probe,schema, \
             priority=3, task_type=chore, risk=none. Then call task show with the new task ID. \
             Then reply with CAS-1C66-FOLLOWUP, CAS-1C66-AGENTS, CAS-1C66-SKILL, \
             the calculation result, and whether .codex/agents/cas-1c66-probe.md exists.",
        ))
        .expect("inject follow-up");
    let second = wait_for_completions(&mut mux, &rollout, 2, Duration::from_secs(60));
    assert!(matching_tool_calls(&second, "coordination", "whoami") >= 2);
    assert!(matching_tool_calls(&second, "task", "mine") >= 2);
    let created_with_full_schema = second
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["type"] == "response_item"
                && event["payload"]["type"] == "function_call"
                && event["payload"]["name"] == "task"
        })
        .filter_map(|event| {
            event["payload"]["arguments"]
                .as_str()
                .and_then(|args| serde_json::from_str::<Value>(args).ok())
        })
        .any(|args| {
            args["action"] == "create"
                && args["title"] == "CAS-1C66-SCHEMA-TITLE"
                && args["description"] == "CAS-1C66-SCHEMA-DESCRIPTION"
                && args["design"] == "CAS-1C66-SCHEMA-DESIGN"
                && args["acceptance_criteria"] == "CAS-1C66-SCHEMA-ACCEPTANCE"
                && args["labels"] == "probe,schema"
                && args["priority"] == 3
                && args["task_type"] == "chore"
                && args["risk"] == "none"
        });
    assert!(
        created_with_full_schema,
        "Codex must send all task create arguments intact"
    );
    assert!(
        tool_call_outputs(&second, "task", "show")
            .iter()
            .any(|output| {
                [
                    "Title: CAS-1C66-SCHEMA-TITLE",
                    "Priority: P3",
                    "Type: chore",
                    "CAS-1C66-SCHEMA-DESCRIPTION",
                    "CAS-1C66-SCHEMA-DESIGN",
                    "CAS-1C66-SCHEMA-ACCEPTANCE",
                    "Labels: probe, schema",
                    "Risk: none",
                ]
                .iter()
                .all(|marker| output.contains(marker))
            }),
        "task show must return every persisted complex-schema field from CAS"
    );
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
    assert_turn_context(&final_body, &scratch, &model, &effort);

    eprintln!(
        "PASS codex-cli 0.156.0 factory contract; model={model}; effort={effort}; complex_schema=task_create_show; isolated_root={}; rollout={}",
        cas_root.display(),
        rollout.display()
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
