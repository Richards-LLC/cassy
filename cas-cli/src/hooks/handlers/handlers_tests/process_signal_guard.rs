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
fn cas_a290_every_factory_role_refuses_pattern_signals() {
    let _env = TestEnvGuard::temp_home();
    // Native subagents inherit the supervisor role. The incident command was
    // auto-approved there even though the identical worker command was denied.
    for role in ["worker", "supervisor", "reviewer", "qa"] {
        for command in [
            "pkill -f \"nuxt prepare\" ; pkill -f \"pnpm install\"",
            "/usr/bin/pkill -TERM -f cargo",
            "killall cargo",
            "bash -lc 'pkill -f cargo'",
            "kill $(pgrep -f cargo)",
            "pgrep -f cargo | xargs kill",
            "for p in $(pgrep -f x); do kill $p; done",
            "ps aux | grep nuxt | awk '{print $2}' | xargs kill",
            "kill $(ps -ef | grep nuxt | awk '{print $2}')",
            "ps -eo pid,args | grep \"pnpm install\" | awk '{print $1}' | xargs kill -9",
            "xargs kill < <(pgrep -f x)",
            "eval \"pkill -f x\"",
            "P=pkill; $P -f x",
        ] {
            let reason = decision(command, role)
                .unwrap_or_else(|| panic!("{role} must refuse {command}"));

        }
        for command in [
            "kill 12345", "kill -0 12345", "kill \"$(cat .pid)\"", "kill $!",
            "pgrep -f x; kill 123", "cas factory shutdown",
        ] {
            assert!(decision(command, role).is_none(), "{role} must allow {command}");
        }
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
