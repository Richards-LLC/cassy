//! Hook-only fixtures: never send a real signal while testing this guard.
use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::HookInput;

fn decision(command: &str, role: &str) -> Option<String> {
    let input = HookInput {
        session_id: "process-signal-fixture".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Bash".into()),
        tool_input: Some(serde_json::json!({"command": command})),
        agent_role: Some(role.into()),
        ..HookInput::default()
    };
    let output = handle_pre_tool_use(&input, None).unwrap();
    let specific = serde_json::to_value(output.hook_specific_output?).unwrap();
    (specific["permissionDecision"] == "deny")
        .then(|| specific["permissionDecisionReason"].as_str().unwrap().to_string())
}

fn assert_refused(command: &str) {
    let reason = decision(command, "worker").unwrap_or_else(|| panic!("expected refusal: {command}"));
    assert!(reason.contains("kill"), "{reason}");
    assert!(reason.contains("PID"), "{reason}");
    assert!(reason.contains("$!") && reason.contains("PID file"), "{reason}");
}

#[test]
fn cas_51c7_worker_pattern_kill_utilities_are_refused() {
    let _env = TestEnvGuard::temp_home();
    for command in [
        "pkill -f scripts/test-release-gate.sh",
        "killall cargo",
        "/usr/bin/pkill -TERM -f cargo",
        "env TEST_FLAG=1 /usr/bin/killall cargo",
        "command pkill -f cargo",
        "bash -lc 'pkill -f cargo'",
        "sh -c 'killall cargo'",
        "nice -n 10 pkill -f cargo",
        "env -S 'pkill -f cargo'",
    ] {
        assert_refused(command);
    }
}

#[test]
fn cas_51c7_worker_pattern_kill_substitutions_are_refused() {
    let _env = TestEnvGuard::temp_home();
    for command in [
        "kill $(pgrep -f scripts/test-release-gate.sh)",
        "kill -TERM \"$(pgrep -f cargo)\"",
        "kill $(/usr/bin/pgrep -f cargo)",
        "kill `pgrep -f cargo`",
    ] {
        assert_refused(command);
    }
}

#[test]
fn cas_51c7_worker_pattern_kill_pipelines_are_refused() {
    let _env = TestEnvGuard::temp_home();
    for command in [
        "pgrep -f cargo | kill",
        "pgrep -f cargo | xargs kill",
        "pgrep -f cargo | xargs -r kill -TERM",
        "pgrep -f cargo | xargs -I{} /bin/kill {}",
        "pgrep -f cargo | sort -u | xargs kill",
    ] {
        assert_refused(command);
    }
}

#[test]
fn cas_51c7_worker_owned_pid_signals_stay_allowed() {
    let _env = TestEnvGuard::temp_home();
    for command in [
        "kill 12345",
        "kill -TERM 12345",
        "kill -0 12345",
        "kill \"$pid\"",
        "sleep 30 & pid=$!; kill \"$pid\"",
        "kill $(cat target/own.pid)",
        "pgrep -f cargo",
        "pgrep -f cargo; kill 12345",
    ] {
        assert!(decision(command, "worker").is_none(), "{command}");
    }
}

#[test]
fn cas_51c7_supervisor_signals_are_out_of_scope() {
    let _env = TestEnvGuard::temp_home();
    for command in ["pkill -f cargo", "killall cargo", "kill $(pgrep -f cargo)", "pgrep -f cargo | xargs kill"] {
        assert!(decision(command, "supervisor").is_none(), "{command}");
    }
}

#[test]
fn cas_51c7_process_signal_documentation_stays_allowed() {
    let _env = TestEnvGuard::temp_home();
    for command in [
        "echo 'pkill -f cargo'",
        "printf '%s' 'kill $(pgrep -f cargo)'",
        "# pkill -f cargo\nkill 12345",
        "cat <<'EOF'\npkill -f cargo\nEOF",
    ] {
        assert!(decision(command, "worker").is_none(), "{command}");
    }
}
