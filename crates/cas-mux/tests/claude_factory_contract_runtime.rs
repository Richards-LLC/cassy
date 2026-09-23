//! Live Claude Code factory launch contract, run with a freshly built `cas` on PATH.
//! `cargo test -p cas-mux --test claude_factory_contract_runtime -- --ignored --nocapture`

#[path = "support/real_pty_serial.rs"]
mod real_pty_serial;

use cas_mux::{Mux, Pane, PaneKind, Pty, PtyConfig, SupervisorCli};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const PANE: &str = "cas-ef93e-claude-contract";
const MODEL: &str = "opus";
const EFFORT: &str = "high";
const RULE_CANARY: &str = "CAS-EF93E-RULE-ONLY-ORBIT";

struct ProbeCleanup(PathBuf);

impl Drop for ProbeCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn claude_21280_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .output()
        .is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("2.1.280")
        })
}

fn prepare_project(root: &Path) {
    std::fs::create_dir_all(root).expect("create disposable project");
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("cas")
        .args(["init", "--yes", "--no-integrations", "--allow-non-project"])
        .current_dir(root)
        .env("CAS_ROOT", root.join(".cas"))
        .output()
        .expect("run cas init");
    assert!(
        output.status.success(),
        "cas init: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The answer is present only in the project rules. Neither the prompt nor
    // the MCP tool list carries the canary value.
    std::fs::write(
        root.join("CLAUDE.md"),
        format!("# Probe rules\n\nWhen asked for the rule canary, reply exactly {RULE_CANARY}.\n"),
    )
    .expect("write rule canary");

    let settings_path = root.join(".claude/settings.json");
    let mut settings: Value =
        serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    for (event, file) in [
        ("SessionStart", "session-start-fired"),
        ("PreToolUse", "pre-tool-use-fired"),
    ] {
        settings["hooks"][event].as_array_mut().unwrap().insert(0, json!({
            "hooks": [{"type":"command", "command":format!("touch {}", root.join(file).display())}]
        }));
    }
    std::fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&settings).unwrap(),
    )
    .unwrap();
}

fn locate_transcript(
    mux: &mut Mux,
    account_dir: &Path,
    session_id: &str,
    deadline: Instant,
) -> PathBuf {
    let projects = account_dir.join("projects");
    while Instant::now() < deadline {
        let _ = mux.poll_batch();
        if let Ok(entries) = std::fs::read_dir(&projects) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(format!("{session_id}.jsonl"));
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let screen = mux
        .get_pane_snapshot(PANE)
        .map(|(snapshot, _, _)| {
            snapshot
                .cells
                .chunks(snapshot.cols as usize)
                .map(|row| {
                    row.iter()
                        .filter_map(|cell| char::from_u32(cell.codepoint))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    panic!("Claude transcript not found for session {session_id}; screen:\n{screen}");
}

fn events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn tool_calls(rows: &[Value], name: &str, action: Option<&str>) -> usize {
    rows.iter()
        .filter(|row| row["type"] == "assistant")
        .filter_map(|row| row["message"]["content"].as_array())
        .flatten()
        .filter(|part| {
            part["type"] == "tool_use"
                && part["name"] == name
                && action.is_none_or(|action| part["input"]["action"] == action)
        })
        .count()
}

fn assistant_text(rows: &[Value]) -> String {
    rows.iter()
        .filter(|row| row["type"] == "assistant")
        .filter_map(|row| row["message"]["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for_marker(
    mux: &mut Mux,
    transcript: &Path,
    marker: &str,
    timeout: Duration,
) -> Vec<Value> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let _ = mux.poll_batch();
        let rows = events(transcript);
        if assistant_text(&rows).contains(marker) {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!("timed out waiting for assistant marker {marker}");
}

#[test]
fn assistant_text_ignores_user_prompt_canary() {
    let rows = vec![
        json!({"type":"user", "message":{"content":"USER-CANARY"}}),
        json!({"type":"assistant", "message":{"content":[{"type":"text", "text":"ASSISTANT-CANARY"}]}}),
    ];
    assert!(!assistant_text(&rows).contains("USER-CANARY"));
    assert!(assistant_text(&rows).contains("ASSISTANT-CANARY"));
}

#[test]
#[ignore = "requires Claude Code 2.1.280, authentication, and model traffic"]
fn claude_21280_factory_launch_contract_passes_live_matrix() {
    let _serial = real_pty_serial::lock();
    assert!(
        claude_21280_available(),
        "live receipt requires Claude Code 2.1.280"
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/claude-factory-contract-probe");
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    let _cleanup = ProbeCleanup(root.clone());
    prepare_project(&root);
    let cas_root = root.join(".cas");
    let account_dir = root.join("claude-account");
    std::fs::create_dir_all(&account_dir).unwrap();
    let host_account = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".claude"));
    std::os::unix::fs::symlink(
        host_account.join(".credentials.json"),
        account_dir.join(".credentials.json"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        host_account.join("settings.json"),
        account_dir.join("settings.json"),
    )
    .unwrap();
    std::fs::write(account_dir.join(".claude.json"), serde_json::to_vec(&json!({"projects": {root.to_string_lossy().to_string(): {"hasTrustDialogAccepted": true}}})).unwrap()).unwrap();
    let mut config = PtyConfig::claude(
        PANE,
        "worker",
        root.clone(),
        Some(&cas_root),
        Some("cas-ef93e-supervisor"),
        Some("claude"),
        Some(MODEL),
        Some(EFFORT),
        None,
    );
    // A disposable, previously unseen repository is not in Claude's trusted
    // project list. Load its generated hook settings explicitly, as production
    // factory team launches do through their role settings path.
    config.env.push((
        "CLAUDE_CONFIG_DIR".into(),
        account_dir.to_string_lossy().into_owned(),
    ));
    config.args.push("--settings".into());
    config.args.push(
        root.join(".claude/settings.json")
            .to_string_lossy()
            .into_owned(),
    );
    config.args.push("--debug-file".into());
    config
        .args
        .push(root.join("claude-debug.log").to_string_lossy().into_owned());
    let session_id = config
        .env
        .iter()
        .find(|(key, _)| key == "CAS_SESSION_ID")
        .unwrap()
        .1
        .clone();
    for (key, expected) in [
        ("CAS_AGENT_NAME", PANE),
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_FACTORY_MODE", "1"),
        ("CAS_FACTORY_WORKER_CLI", "claude"),
        ("CAS_FACTORY_WORKER_MODEL", MODEL),
        ("CAS_FACTORY_WORKER_EFFORT", EFFORT),
    ] {
        assert!(
            config.env.iter().any(|(k, v)| k == key && v == expected),
            "missing {key}={expected}"
        );
    }
    assert!(
        config
            .env
            .iter()
            .any(|(k, v)| k == "CAS_ROOT" && v == cas_root.to_string_lossy().as_ref())
    );
    assert!(
        config
            .env
            .iter()
            .any(|(k, v)| k == "CLAUDE_PROJECT_DIR" && v == root.to_string_lossy().as_ref())
    );
    assert_eq!(config.cwd.as_deref(), Some(root.as_path()));
    for flag in [
        "--dangerously-skip-permissions",
        "--permission-mode",
        "bypassPermissions",
        "--session-id",
        "--model",
        MODEL,
        "--effort",
        EFFORT,
    ] {
        assert!(
            config.args.iter().any(|arg| arg == flag),
            "missing launch flag/value {flag}"
        );
    }

    let pty = Pty::spawn(PANE, config).expect("spawn production Claude PTY");
    let pane = Pane::with_pty(PANE, PaneKind::Worker, pty, 24, 80, SupervisorCli::Claude).unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    std::thread::sleep(Duration::from_secs(5));
    runtime.block_on(mux.inject(PANE,
        "You are a Cassy Factory Worker. Use the CAS MCP tools. Call mcp__cas__coordination action=whoami and mcp__cas__task action=mine, then reply CAS-EF93E-ROOT."
    )).expect("inject worker startup");
    let transcript = locate_transcript(
        &mut mux,
        &account_dir,
        &session_id,
        Instant::now() + Duration::from_secs(30),
    );
    let first = wait_for_marker(
        &mut mux,
        &transcript,
        "CAS-EF93E-ROOT",
        Duration::from_secs(90),
    );
    assert!(
        tool_calls(&first, "mcp__cas__coordination", Some("whoami")) >= 1,
        "root coordination discovery/call"
    );
    assert!(
        tool_calls(&first, "mcp__cas__task", Some("mine")) >= 1,
        "root task discovery/call"
    );
    assert!(assistant_text(&first).contains("CAS-EF93E-ROOT"));
    assert!(
        root.join("session-start-fired").is_file(),
        "SessionStart hook did not fire"
    );
    assert!(
        root.join("pre-tool-use-fired").is_file(),
        "PreToolUse hook did not fire"
    );

    runtime.block_on(mux.inject(PANE,
        "Call mcp__cas__coordination action=whoami and mcp__cas__task action=mine again. What is the rule canary from CLAUDE.md? Also run pwd through Bash. Reply CAS-EF93E-FOLLOWUP."
    )).expect("inject follow-up");
    let second = wait_for_marker(
        &mut mux,
        &transcript,
        "CAS-EF93E-FOLLOWUP",
        Duration::from_secs(90),
    );
    assert!(
        tool_calls(&second, "mcp__cas__coordination", Some("whoami")) >= 2,
        "follow-up MCP coordination"
    );
    assert!(
        tool_calls(&second, "mcp__cas__task", Some("mine")) >= 2,
        "follow-up MCP task"
    );
    assert!(
        tool_calls(&second, "Bash", None) >= 1,
        "follow-up Bash cwd check"
    );
    let text = assistant_text(&second);
    for marker in [RULE_CANARY, "CAS-EF93E-FOLLOWUP"] {
        assert!(text.contains(marker), "missing {marker}");
    }
    assert!(
        text.contains(root.to_string_lossy().as_ref()),
        "assistant must report worktree cwd"
    );

    runtime
        .block_on(mux.inject(
            PANE,
            "Write a lengthy essay on terminal multiplexers; keep working until interrupted.",
        ))
        .unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    runtime
        .block_on(mux.interrupt_and_inject(
            PANE,
            "Message from supervisor: stop and reply CAS-EF93E-RESUMED.",
            Duration::from_millis(1200),
        ))
        .expect("urgent Esc interrupt and inject");
    let third = wait_for_marker(
        &mut mux,
        &transcript,
        "CAS-EF93E-RESUMED",
        Duration::from_secs(90),
    );
    assert!(
        assistant_text(&third).contains("CAS-EF93E-RESUMED"),
        "urgent redirect did not complete"
    );
    assert!(
        third
            .iter()
            .filter(|row| row["type"] == "assistant")
            .all(|row| row["message"]["model"]
                .as_str()
                .is_some_and(|model| model.starts_with("claude-opus"))),
        "model drifted"
    );
    eprintln!(
        "PASS Claude Code 2.1.280 factory contract; root={}; transcript={}",
        root.display(),
        transcript.display()
    );
}
