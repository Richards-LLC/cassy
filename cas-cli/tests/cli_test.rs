//! CLI integration tests

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_cmd(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    // Clear CAS_ROOT to prevent env pollution from parent shell
    // Tests should use current_dir() for isolation, not inherit env vars
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

#[test]
fn test_help() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Cassy — a multi-agent coding factory with persistent memory and task coordination",
        ));
}

#[test]
fn test_version() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("cas"));
}

#[test]
fn test_init_yes_flag() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cassy initialized"))
        .stdout(predicate::str::contains("cas hub service install"));

    assert!(temp.path().join(".cas").exists());
    assert!(temp.path().join(".cas/cas.db").exists());
    // Config is now saved as TOML (preferred format)
    assert!(temp.path().join(".cas/config.toml").exists());
}

#[test]
fn test_init_json() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized"));
}

#[test]
fn test_init_already_initialized() {
    let temp = TempDir::new().unwrap();

    // First init
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Second init without force should inform user
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already initialized"));
}

/// Regression test for cas-bf06: a reinit of `cas init` (interactive, no
/// --yes/--json) with an EOF'd stdin must not hang at 100% CPU. Prior to
/// the fix in cas-cli/src/cli/interactive.rs, `select()` looped forever
/// when stdin returned EOF, burning a CPU core and leaving a 0-byte log.
#[test]
fn test_init_reinit_with_closed_stdin_does_not_hang() {
    use std::io::Write;
    use std::process::{Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    let temp = TempDir::new().unwrap();

    // First init to make the reinit branch active.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Now run `cas init` interactively (no --yes / --json / --force) with
    // stdin closed. This is the exact call shape that hung in production:
    // `cas init 2>&1 | tail -5` where stdin is the tty but gets EOF'd.
    let bin = cas::test_paths::cas_binary();
    let started = Instant::now();
    let mut child = StdCommand::new(bin)
        .current_dir(&temp)
        .arg("init")
        .env("HOME", temp.path().join(".test-home"))
        .env("XDG_CONFIG_HOME", temp.path().join(".test-xdg-config"))
        .env(
            "CAS_TEST_PROTECTED_HOME",
            std::env::var_os("HOME").unwrap_or_default(),
        )
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn cas");

    // Close stdin immediately — the child will see EOF on its first read.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"")
        .expect("close stdin");

    // Poll for exit. Fix causes exit within ~1s. Regression would hang
    // until our watchdog fires (5min) or this loop times out (20s).
    let deadline = started + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                panic!(
                    "cas init hung for more than 20s with closed stdin — \
                     regression of cas-bf06 (select() infinite loop on EOF)"
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    // The process must terminate with an exit code (i.e., ran to completion
    // and returned), not by signal (SIGKILL/SIGSEGV). Bounded termination
    // is already proven by the poll loop's deadline panic above; this asserts
    // that the termination path is clean rather than a crash or external kill.
    assert!(
        status.code().is_some(),
        "cas init was terminated by signal rather than exiting: {:?}",
        status
    );
}

#[test]
fn test_init_force_reinit() {
    let temp = TempDir::new().unwrap();

    // First init
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Force reinit should succeed
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cassy initialized"));
}

/// EPIC cas-8888 (cas-6f46, Phase 5): a pre-existing `.grok/` dir (the
/// opt-in signal `detect_agent_defaults` looks for) must cause `cas init`
/// to sync the Grok builtin skill twins, using the `cas__` tool prefix —
/// end-to-end proof that the config.agents.grok wiring actually runs,
/// not just that the underlying sync function works in isolation.
#[test]
fn test_init_json_syncs_grok_builtins_when_grok_dir_present() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join(".grok")).unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"grok\":true"));

    let worker_skill = temp.path().join(".grok/skills/cas-worker/SKILL.md");
    assert!(
        worker_skill.exists(),
        "cas init with .grok/ present must sync the Grok cas-worker skill twin"
    );
    let content = std::fs::read_to_string(&worker_skill).unwrap();
    assert!(
        content.contains("cas__task"),
        "grok cas-worker skill must reference the cas__ tool prefix"
    );
    assert!(
        !content.contains("mcp__"),
        "grok cas-worker skill must not reference any mcp__ wrapped tool name"
    );

    // .mcp.json is reused (no separate Grok config writer) — confirm it
    // was still created so Grok's own MCP doctor can find it.
    assert!(temp.path().join(".mcp.json").exists());
}

#[test]
fn test_init_json_already_initialized() {
    let temp = TempDir::new().unwrap();

    // First init
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success();

    // Second init in JSON mode
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already_initialized"));
}

#[test]
fn test_status() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("entries"));
}

#[test]
fn test_status_verbose() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["status", "-v"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configuration"));
}

#[test]
fn test_config() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // List config
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.enabled"));

    // Get specific value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));

    // Set value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.min_helpful", "5"])
        .assert()
        .success();

    // Verify
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.min_helpful"])
        .assert()
        .success()
        .stdout(predicate::str::contains("5"));
}

#[test]
fn test_doctor() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("cas directory"));
}

#[test]
fn test_not_initialized_error() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not initialized"));
}

#[test]
fn test_config_list_offline_no_auth_required() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Ensure local config command remains available without login state.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.enabled"));
}

#[test]
fn test_status_offline_no_auth_required() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Ensure local status command remains available without login state.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("entries"));
}

#[test]
fn test_cloud_command_requires_auth() {
    let temp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("HOME", home.path())
        .args(["cloud", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Not logged in"));
}

#[test]
fn test_hook_command_session_start() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test SessionStart hook with JSON input
    let input = r#"{"session_id":"test123","cwd":"/test","hook_event_name":"SessionStart"}"#;

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["hook", "SessionStart"])
        .write_stdin(input)
        .assert()
        .success();
}

#[test]
fn test_hook_config() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Check default hook config
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "hooks.capture_enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));

    // Set hook config
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.capture_enabled", "false"])
        .assert()
        .success();

    // Verify it was set
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "hooks.capture_enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}

#[test]
fn test_hook_post_tool_use() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test PostToolUse hook with Write tool
    let input = r#"{
        "session_id": "tool-use-test",
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/test/file.rs", "content": "fn main() {}"}
    }"#;

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["hook", "PostToolUse"])
        .write_stdin(input)
        .assert()
        .success();
}

#[test]
fn test_hook_user_prompt_submit() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test UserPromptSubmit hook
    let input = r#"{
        "session_id": "prompt-test",
        "cwd": "/test",
        "hook_event_name": "UserPromptSubmit",
        "user_prompt": "Help me write tests"
    }"#;

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["hook", "UserPromptSubmit"])
        .write_stdin(input)
        .assert()
        .success();
}

#[test]
fn test_config_list() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test config list
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.enabled"))
        .stdout(predicate::str::contains("hooks.token_budget"));
}

#[test]
fn test_config_list_json() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test config list with JSON output
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("hooks"));
}

#[test]
fn test_config_get_set() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Get a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));

    // Set a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.enabled", "false"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Set sync.enabled"));

    // Verify the value was set
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}

#[test]
fn test_config_get_unknown_key() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Try to get unknown key
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "unknown.key"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown config key"));
}

#[test]
fn test_config_set_validation() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Try to set invalid boolean value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.enabled", "notabool"])
        .assert()
        .failure();

    // Try to set invalid integer value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.token_budget", "notanumber"])
        .assert()
        .failure();
}

#[test]
fn test_config_list_section_filter() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Filter by section
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list", "--section", "hooks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks.capture_enabled"))
        .stdout(predicate::str::contains("hooks.token_budget"));
}

#[test]
fn test_config_list_modified() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Modify a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.token_budget", "8000"])
        .assert()
        .success();

    // List only modified values
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list", "--modified"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks.token_budget"))
        .stdout(predicate::str::contains("8000"));
}

#[test]
fn test_config_diff() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // After init --yes with defaults, there should be no differences from defaults
    // (mcp.enabled was removed - MCP is always enabled)
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No differences"));

    // The default is enabled; an explicit opt-out must remain visible as an override.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "code.enabled", "false"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("code.enabled"))
        .stdout(predicate::str::contains("false"))
        .stdout(predicate::str::contains("default: true"));

    // Modify a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.min_helpful", "5"])
        .assert()
        .success();

    // Now there should be differences
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.min_helpful"))
        .stdout(predicate::str::contains("5"))
        .stdout(predicate::str::contains("default: 1"));
}

#[test]
fn test_config_describe() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Describe a config key
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "describe", "hooks.token_budget"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Token Budget"))
        .stdout(predicate::str::contains("integer"))
        .stdout(predicate::str::contains("4000"));
}

#[test]
fn test_config_export_import() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Modify a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.token_budget", "6000"])
        .assert()
        .success();

    // Export config
    let export_output = cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "config", "list"])
        .assert()
        .success();

    // Verify exported config contains our modification
    let stdout = String::from_utf8_lossy(&export_output.get_output().stdout);
    assert!(stdout.contains("6000"));
}

#[test]
fn test_doctor_json() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""name":"cas directory""#))
        .stdout(predicate::str::contains(r#""status":"ok""#));
}

#[test]
fn test_doctor_mcp_configured() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Doctor should show MCP is configured after init
    // Use CAS_ROOT to isolate from parent project's .cas
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp config"))
        .stdout(predicate::str::contains("MCP configured"));
}

#[test]
fn test_doctor_mcp_not_configured() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    // Initialize CAS
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Delete the .mcp.json file to simulate MCP not being configured
    std::fs::remove_file(temp.path().join(".mcp.json")).unwrap();

    // Doctor should warn about MCP not being configured
    // Use CAS_ROOT to isolate from parent project's .cas
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp config"))
        .stdout(predicate::str::contains("MCP not configured"));
}

#[test]
fn test_doctor_fix_initializes_project() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("auto-fix"))
        .stdout(predicate::str::contains("Initialized Cassy at"));
}

#[test]
fn test_doctor_fix_json_before_init_errors() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--fix", "--json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "`cas doctor --fix --json` is not supported before initialization",
        ));
}

#[test]
fn test_bare_factory_flags_are_parsed() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .args(["--new", "-w", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

#[test]
fn test_noninteractive_factory_includes_preflight_hints() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .args(["--new", "-w", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Initialize Cassy first with `cas doctor --fix` (or `cas init`).",
        ));
}

#[test]
fn test_doctor_not_initialized() {
    let temp = TempDir::new().unwrap();

    // Doctor on uninitialized directory should show error
    // Use CAS_ROOT pointing to non-existent .cas to prevent finding parent project's .cas
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", temp.path().join(".cas"))
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("Not found"));
}

// ---------------------------------------------------------------------------
// `cas knowledge` — the distilled project wiki (cas-ee3d)
// ---------------------------------------------------------------------------

/// Seed one distilled page directly through the store, so the CLI round-trip
/// can be exercised without an LLM. `cas knowledge build` is the production
/// producer; this stands in for it.
fn seed_knowledge_page(cas_dir: &std::path::Path) {
    use cas_store::{IngestBatch, KnowledgePage, KnowledgeStore, PageWrite, SqliteKnowledgeStore};

    let store = SqliteKnowledgeStore::open(cas_dir).expect("knowledge store should open");
    let mut page = KnowledgePage::new(
        store.generate_id().expect("id"),
        "subsystem",
        "Worktree Manager",
    );
    page.snippet = "Creates and reaps per-worker git worktrees.".to_string();
    page.sources = vec!["docs/worktrees.md".to_string()];

    store
        .commit_ingest(&IngestBatch {
            pages: vec![PageWrite {
                page,
                body: "# Worktree Manager\n\nCreates and reaps per-worker git worktrees.\n\
                       Each factory worker gets an isolated branch.\n"
                    .to_string(),
            }],
            ..IngestBatch::default()
        })
        .expect("ingest should commit");
}

/// Replace a current initialized knowledge schema with the m219 shape that
/// existed before m226 added page-attribution columns. The CLI process must
/// repair this fixture through `SqliteKnowledgeStore::open` before any page
/// command issues a query.
fn seed_pre_m226_knowledge_store(cas_dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("open legacy fixture");
    conn.execute_batch(
        "DROP TABLE knowledge_pages_fts;
         DROP TABLE knowledge_sources;
         DROP TABLE knowledge_page_tombstones;
         DROP TABLE knowledge_pages;
         DELETE FROM cas_migrations WHERE id IN (219, 226, 227);

         CREATE TABLE knowledge_pages (
             row_id INTEGER PRIMARY KEY AUTOINCREMENT,
             id TEXT NOT NULL UNIQUE,
             page_type TEXT NOT NULL,
             title TEXT NOT NULL,
             rel_path TEXT NOT NULL,
             snippet TEXT NOT NULL DEFAULT '',
             locked INTEGER NOT NULL DEFAULT 0,
             sources_json TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             pending_embedding INTEGER NOT NULL DEFAULT 1
         );
         CREATE VIRTUAL TABLE knowledge_pages_fts USING fts5(
             title, snippet, body, content='', contentless_delete=1
         );
         INSERT INTO knowledge_pages
             (id, page_type, title, rel_path, snippet, sources_json, created_at, updated_at)
         VALUES (
             'cas-kn001', 'architecture', 'Legacy Store', 'architecture/legacy-store.md',
             'A pre-m226 page.', '[\"README.md\"]',
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
         );
         INSERT INTO knowledge_pages_fts (rowid, title, snippet, body)
         VALUES (1, 'Legacy Store', 'A pre-m226 page.', 'The legacy page is searchable.');",
    )
    .expect("create pre-m226 knowledge fixture");
    std::fs::create_dir_all(cas_dir.join("knowledge/architecture")).expect("create body dir");
    std::fs::write(
        cas_dir.join("knowledge/architecture/legacy-store.md"),
        "# Legacy Store\n\nThe legacy page is searchable.\n",
    )
    .expect("write legacy body");
}

#[test]
fn test_knowledge_commands_open_and_upgrade_pre_m226_store() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    let cas_dir = temp.path().join(".cas");
    seed_pre_m226_knowledge_store(&cas_dir);

    // Every public CLI entry point opens the shared store. `status` runs
    // first, proving the self-heal happens before its first SELECT; the rest
    // guard against future paths opening a query-capable legacy store.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pages:   1"));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("architecture/legacy-store.md"));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "searchable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Legacy Store"));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "read", "cas-kn001"])
        .assert()
        .success()
        .stdout(predicate::str::contains("The legacy page is searchable."));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "build", "--dry-run", "--max-sources", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Knowledge distillation (dry run"));

    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("open upgraded store");
    for column in ["origin", "origin_project_id"] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM pragma_table_info('knowledge_pages') WHERE name = ?1
                 )",
                [column],
                |row| row.get(0),
            )
            .expect("query column shape");
        assert_eq!(exists, 1, "m226 column {column} must be restored");
    }
}

#[test]
fn test_knowledge_search_and_read_round_trip() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    seed_knowledge_page(&temp.path().join(".cas"));

    // search surfaces the page by a body word.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "worktrees"])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"))
        .stdout(predicate::str::contains("Worktree Manager"));

    // read by path prints metadata plus the markdown body from disk.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "read", "subsystem/worktree-manager.md"])
        .assert()
        .success()
        .stdout(predicate::str::contains("locked:  false"))
        .stdout(predicate::str::contains("docs/worktrees.md"))
        .stdout(predicate::str::contains(
            "Each factory worker gets an isolated branch.",
        ));
}

#[cfg(unix)]
#[test]
fn knowledge_build_timeout_returns_nonzero_and_does_not_start_another_call() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    std::fs::write(temp.path().join("README.md"), "# Slow source\n\ncontent\n").unwrap();
    let provider = temp.path().join("provider");
    let calls = temp.path().join("provider-calls");
    std::fs::write(
        &provider,
        format!(
            "#!/bin/sh\nprintf call >> '{}'\nsleep 30\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_KNOWLEDGE_LLM_BIN", &provider)
        .args([
            "knowledge",
            "build",
            "--timeout-secs",
            "1",
            "--max-sources",
            "5",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "timeout must be a nonzero CLI result"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(calls).unwrap().lines().count(),
        1,
        "the command deadline must prevent later stage/source calls"
    );
}

/// cas-461a, through the shipped command rather than the store API.
///
/// `fts_query` joined tokens with a space, which FTS5 reads as an implicit
/// AND, so a multi-term search only matched pages containing *every* term.
/// The cas-d075 measurement
/// (`docs/migration/cas-b129-knowledge-retrieval-verdict.md`) recorded 7 of 10
/// real-vocabulary queries returning zero pages where legacy returned 4–10.
///
/// The regression is asserted at the CLI boundary because that is where it was
/// observed and because the failure was silent: `cas knowledge search` printed
/// a clean "No distilled pages match", not an error, so nothing upstream could
/// notice it.
#[test]
fn test_knowledge_multi_term_search_is_disjunctive() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    seed_knowledge_page(&temp.path().join(".cas"));

    // Every term present: worked before this fix, must keep working.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "worktrees factory branch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"));

    // The actual regression: a query where only some terms occur on the page.
    // Under the implicit-AND conjunction this printed "No distilled pages
    // match" and the page was unreachable.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args([
            "knowledge",
            "search",
            "worktrees deployment kubernetes rollback",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"))
        .stdout(predicate::str::contains("No distilled pages match").not());

    // Disjunctive must not mean "matches anything": a query sharing no term
    // with the corpus still reports no matches.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "kubernetes rollback helm"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No distilled pages match"));

    // An explicitly quoted phrase keeps adjacency: these two words both appear
    // on the page but never next to each other in this order.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "\"branch worktrees\""])
        .assert()
        .success()
        .stdout(predicate::str::contains("No distilled pages match"));

    // ...while the phrase that does appear verbatim is found, proving the
    // result above is adjacency rather than a tokenisation accident.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "\"isolated branch\""])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"));
}

#[test]
fn test_knowledge_search_with_no_match_is_not_an_error() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "nonexistent-subject"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No distilled pages match"));
}

#[test]
fn test_knowledge_read_of_unknown_page_fails_loudly() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "read", "subsystem/does-not-exist.md"])
        .assert()
        .failure();
}

/// cas-b69a (GH #157): the cas-b129 M3 incident in miniature — copy a project
/// store, `cd` into the copy, and run a MUTATING command while `CAS_ROOT` (as a
/// factory session exports it) still points at the live store.
///
/// The write must still land in the CAS_ROOT store (precedence is deliberately
/// unchanged — factory workers in clones depend on it), but the operator must be
/// told, on stderr, that the store under their feet lost.
#[test]
fn cas_root_override_of_a_differing_cwd_root_is_announced_on_stderr() {
    let temp = TempDir::new().unwrap();
    let live = temp.path().join("live");
    let copy = temp.path().join("copy");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(&copy).unwrap();

    // Two independent projects, each with its own .cas.
    cas_cmd(temp.path())
        .current_dir(&live)
        .args(["init", "--yes"])
        .assert()
        .success();
    cas_cmd(temp.path())
        .current_dir(&copy)
        .args(["init", "--yes"])
        .assert()
        .success();

    let live_root = live.join(".cas");
    let copy_root = copy.join(".cas");

    // A mutating command, run from the copy, with CAS_ROOT aimed at the live store.
    let output = cas_cmd(temp.path())
        .current_dir(&copy)
        .env("CAS_ROOT", &live_root)
        .args(["config", "set", "sync.min_helpful", "7"])
        .output()
        .expect("cas config set must run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // (1) Both roots named, winner stated.
    assert!(
        stderr.contains(&live_root.display().to_string()),
        "stderr must name the winning CAS_ROOT store.\nstderr: {stderr}"
    );
    assert!(
        stderr.contains(&copy_root.display().to_string()),
        "stderr must name the working-directory store that lost.\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("CAS_ROOT wins"),
        "stderr must state which root won.\nstderr: {stderr}"
    );

    // (3) stderr only — stdout stays parseable.
    assert!(
        !stdout.contains("CAS_ROOT override"),
        "the notice must never reach stdout.\nstdout: {stdout}"
    );

    // (2) Precedence itself is unchanged: the write really did land in the
    // CAS_ROOT store, which is exactly why the notice has to exist.
    let live_config = std::fs::read_to_string(live_root.join("config.toml")).unwrap();
    let copy_config = std::fs::read_to_string(copy_root.join("config.toml")).unwrap();
    assert!(
        live_config.contains("min_helpful = 7"),
        "CAS_ROOT must keep winning.\nlive config: {live_config}"
    );
    assert!(
        !copy_config.contains("min_helpful = 7"),
        "the working-directory store must be untouched.\ncopy config: {copy_config}"
    );
}

/// The other half of the honesty contract: when there is nothing to
/// disambiguate — CAS_ROOT pointing at the very store the working directory
/// would have resolved on its own — there must be no notice at all. A banner
/// that fires on every ordinary factory command is a banner nobody reads.
#[test]
fn cas_root_matching_the_cwd_root_produces_no_notice() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    cas_cmd(temp.path())
        .current_dir(&project)
        .args(["init", "--yes"])
        .assert()
        .success();

    let output = cas_cmd(temp.path())
        .current_dir(&project)
        .env("CAS_ROOT", project.join(".cas"))
        .args(["config", "set", "sync.min_helpful", "7"])
        .output()
        .expect("cas config set must run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("CAS_ROOT override"),
        "no conflict means no notice.\nstderr: {stderr}"
    );
}
