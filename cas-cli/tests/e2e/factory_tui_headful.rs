//! Factory TUI headful e2e tests using cas-tui-test

#![cfg(target_os = "macos")]

use cas::store::{open_agent_store, open_prompt_queue_store};
use cas::ui::factory::{SessionInfo, SessionManager};
use cas_tui_test::{PtyRunner, PtyRunnerConfig, WaitConfig, WaitExt, screen_with_size};
use cas_types::AgentRole;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::fixtures::new_cas_instance;

fn headful_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard;
impl EnvGuard {
    fn set(vars: &[(&str, String)]) -> crate::test_env_guard::TestEnvGuard {
        let mut env = crate::test_env_guard::TestEnvGuard::new();
        for (key, value) in vars { env.set(key, value); }
        env
    }
}

fn cas_binary() -> String {
    cas::test_paths::binary("cas", option_env!("CARGO_BIN_EXE_cas").map(Into::into))
        .to_string_lossy()
        .into_owned()
}

fn wait_for_screen_text(
    runner: &PtyRunner,
    text: &str,
    timeout: Duration,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let output = runner.get_output();
        let scr = screen_with_size(&output.as_str(), cols, rows);
        if scr.text().contains(text) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("Timed out waiting for screen text: {text}"))
}

fn terminal_window_ids(title: &str) -> Vec<String> {
    let script = format!(
        "tell application \"Terminal\"\n\
            set targetTitle to \"{title}\"\n\
            set wids to {{}}\n\
            repeat with w in windows\n\
                repeat with t in tabs of w\n\
                    try\n\
                        if custom title of t is targetTitle then\n\
                            set end of wids to id of w\n\
                        end if\n\
                    end try\n\
                end repeat\n\
            end repeat\n\
            return wids\n\
        end tell"
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .expect("failed to run osascript");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn wait_for_terminal_window(title: &str, timeout: Duration) -> Vec<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let ids = terminal_window_ids(title);
        if !ids.is_empty() {
            return ids;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    vec![]
}

fn close_terminal_window(window_id: &str) {
    let script = format!(
        "tell application \"Terminal\"\n\
            try\n\
                set targetWindow to window id {window_id}\n\
                close targetWindow\n\
            end try\n\
        end tell"
    );
    let _ = Command::new("osascript").arg("-e").arg(script).output();
}

fn wait_for_session_info(
    project_dir: &std::path::Path,
    timeout: Duration,
) -> Result<SessionInfo, String> {
    let manager = SessionManager::new();
    let project = project_dir.to_string_lossy();
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(Some(info)) = manager.find_session_for_project(None, &project) {
            if info.can_attach() {
                return Ok(info);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("Timed out waiting for factory session metadata".to_string())
}

fn wait_for_worker_count(
    cas_root: &std::path::Path,
    expected: usize,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(store) = open_agent_store(cas_root) {
            if let Ok(agents) = store.list(None) {
                let workers = agents
                    .iter()
                    .filter(|agent| agent.role == AgentRole::Worker)
                    .count();
                if workers >= expected {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "Timed out waiting for {expected} workers to register"
    ))
}

fn count_worker_replies(output: &str, token: &str) -> usize {
    output
        .lines()
        .filter(|line| line.contains("<message from=") && line.contains(token))
        .count()
}

#[test]
fn explicit_headful_missing_claude_fails_cas_7e12() {
    let empty_path = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "e2e::factory_tui_headful::factory_tui_headful_reuse_window", "--nocapture"])
        .env("CAS_HEADFUL_E2E", "1")
        .env("PATH", empty_path.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "explicit live opt-in must not pass without Claude");
    assert!(String::from_utf8_lossy(&output.stderr).contains("headful dependency unavailable"));
}

#[test]
fn headful_fixture_preserves_selected_claude_credentials_cas_7e12() {
    let keys = ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];
    let _ambient = keys.map(|key| crate::test_env_guard::AmbientEnvRestore::set(key, "headful-fixture-value"));
    let _unrelated = crate::test_env_guard::AmbientEnvRestore::set("VIKTOR_API_KEY", "unrelated-fixture");
    {
        let _guard = EnvGuard::set(&[("TUI_TEST_HEADFUL", "1".into())]);
        let child = Command::new("/bin/sh").args(["-c",
            "[ \"$CLAUDE_CONFIG_DIR\" = headful-fixture-value ] && \
             [ \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" = headful-fixture-value ] && \
             [ \"$CLAUDE_CODE_OAUTH_TOKEN\" = headful-fixture-value ] && \
             [ \"$ANTHROPIC_API_KEY\" = headful-fixture-value ] && \
             [ \"$ANTHROPIC_AUTH_TOKEN\" = headful-fixture-value ] && \
             [ \"$TUI_TEST_HEADFUL\" = 1 ] && [ -z \"${VIKTOR_API_KEY+x}\" ] && [ -z \"${CAS_ROOT+x}\" ]"])
            .status().unwrap();
        assert!(child.success(), "live fixture must preserve Claude account selection and credentials in children");
    }
    for key in keys { assert_eq!(std::env::var(key).unwrap(), "headful-fixture-value"); }
    let _ordinary = crate::test_env_guard::TestEnvGuard::new();
    for key in keys { assert!(std::env::var_os(key).is_none(), "ordinary fixtures still scrub {key}"); }
}

#[test]
fn factory_tui_headful_reuse_window() {
    if std::env::var("CAS_HEADFUL_E2E").ok().as_deref() != Some("1") {
        eprintln!("Skipping: CAS_HEADFUL_E2E=1 not set");
        return;
    }
    let _guard = headful_lock().lock().expect("lock poisoned");
    if Command::new("claude").arg("--version").output().is_err() {
        eprintln!("Skipping: claude CLI not available");
        return;
    }

    let cas = new_cas_instance();
    let cwd = std::fs::canonicalize(cas.temp_dir.path())
        .unwrap_or_else(|_| cas.temp_dir.path().to_path_buf());
    let cas_root = cwd.join(".cas");

    let name = format!("cas-factory-e2e-{}", std::process::id());
    let title = format!("cas-tui-headful:{name}");
    let fifo_path = std::env::temp_dir().join(format!("cas-tui-headful-{name}.fifo"));
    let window_id_path = std::env::temp_dir().join(format!("cas-tui-headful-{name}.windowid"));
    let _ = std::fs::remove_file(&fifo_path);
    let _ = std::fs::remove_file(&window_id_path);

    let _env_guard = EnvGuard::set(&[
        ("TUI_TEST_HEADFUL", "1".to_string()),
        ("TUI_TEST_HEADFUL_REUSE", "1".to_string()),
        ("TUI_TEST_HEADFUL_NAME", name.clone()),
    ]);

    let config = PtyRunnerConfig::with_size(120, 40)
        .env("CAS_FACTORY_BUILD_GUARD", "off")
        .env("CAS_ROOT", cas_root.to_string_lossy())
        .cwd(&cwd);

    let wait_config = WaitConfig::with_timeout(Duration::from_secs(6))
        .stable_duration(Duration::from_millis(200));

    for run_idx in 0..2 {
        let mut runner = PtyRunner::with_config(config.clone());
        runner
            .spawn(
                &cas_binary(),
                &["factory", "--supervisor-cli", "claude", "--workers", "0"],
            )
            .expect("failed to spawn cas factory");

        let header_result =
            wait_for_screen_text(&runner, "CAS Factory", Duration::from_secs(15), 120, 40);
        if let Err(err) = header_result {
            let output = runner.get_output();
            let raw = output.as_str();
            if raw.contains("Error:") || raw.contains("CAS is not initialized") {
                let scr = screen_with_size(&output.as_str(), 120, 40);
                panic!(
                    "factory header missing: {err}\n--- raw output ---\n{}\n--- screen text ---\n{}",
                    raw,
                    scr.text()
                );
            }
            if !runner.is_running() {
                let status = runner.wait().ok().flatten();
                panic!(
                    "factory process exited before rendering header (status: {status:?})\n--- raw output ---\n{raw}"
                );
            }
            eprintln!("warning: factory header not detected: {err}");
        }
        let _ = runner.wait_stable_config(&wait_config);

        if run_idx == 0 {
            let session = match wait_for_session_info(&cwd, Duration::from_secs(30)) {
                Ok(info) => info,
                Err(err) => {
                    let output = runner.get_output();
                    let raw = output.as_str();
                    panic!("failed to load session metadata: {err}\n--- raw output ---\n{raw}");
                }
            };
            let supervisor_name = session.metadata.supervisor.name.clone();

            let token = {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                format!("CAS_FACTORY_E2E_HI_{}_{}", std::process::id(), nanos)
            };

            let prompt = format!(
                "Use MCP tools only. Run exactly:\n\
mcp__cas__factory action=spawn_workers count=3\n\
Then send to all workers:\n\
mcp__cas__coordination action=message target=all_workers message=\"Reply with: mcp__cas__coordination action=message target={supervisor_name} message=\\\"{token}\\\"\"\n\
Do not add extra text."
            );

            let queue = open_prompt_queue_store(&cas_root).expect("open prompt queue");
            queue
                .enqueue("cas", &supervisor_name, &prompt)
                .expect("enqueue supervisor prompt");

            wait_for_worker_count(&cas_root, 3, Duration::from_secs(120))
                .expect("workers did not register (is claude logged in?)");

            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(120) {
                let output = runner.get_output();
                let output_str = output.as_str_lossy();
                let count = count_worker_replies(output_str.as_ref(), &token);
                if count >= 3 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }

            let output = runner.get_output();
            let output_str = output.as_str_lossy();
            let reply_count = count_worker_replies(output_str.as_ref(), &token);
            assert!(
                reply_count >= 3,
                "expected 3 worker replies, saw {reply_count}"
            );
        }

        runner.send_bytes(b"\x11").expect("failed to send Ctrl+Q");

        let start = Instant::now();
        while runner.is_running() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if runner.is_running() {
            let _ = runner.kill();
        }
    }

    let ids = wait_for_terminal_window(&title, Duration::from_secs(6));
    assert_eq!(ids.len(), 1, "expected one headful window, got {ids:?}");

    let ids_after = wait_for_terminal_window(&title, Duration::from_secs(2));
    assert_eq!(
        ids_after.len(),
        1,
        "expected one headful window after reuse"
    );
    assert_eq!(ids_after[0], ids[0], "expected same window id on reuse");

    close_terminal_window(&ids[0]);
    let _ = std::fs::remove_file(&fifo_path);
    let _ = std::fs::remove_file(&window_id_path);
}
