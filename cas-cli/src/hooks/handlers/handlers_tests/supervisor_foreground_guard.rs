//! cas-803f: a supervisor's long foreground Bash call is flagged (H-49) so
//! operator and Commander messages can always reach it. Commands replay the
//! Nov 20 incidents; assertions read the gate code, never the wording.
use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::HookInput;

fn refusal_codes(tool_input: serde_json::Value) -> Option<Vec<&'static str>> {
    refusal_codes_as(tool_input, None)
}

fn refusal_codes_as(tool_input: serde_json::Value, agent_id: Option<&str>) -> Option<Vec<&'static str>> {
    let input = HookInput {
        session_id: "supervisor-foreground-fixture".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Bash".into()),
        tool_input: Some(tool_input),
        agent_role: Some("supervisor".into()),
        agent_id: agent_id.map(Into::into),
        ..HookInput::default()
    };
    let output = handle_pre_tool_use(&input, None).unwrap();
    assert!(!super::blocked(&output), "a demoted guard never blocks");
    Some(super::warned(&output)).filter(|codes| !codes.is_empty())
}

const LONG: &[&str] = &[
    "scripts/release.sh 9.99.0",
    "bash -lc 'scripts/release-gate.sh'",
    "cd cas-cli && timeout 300 cargo nextest run -p cas --lib -E 'test(gates::registry_tests)'",
    "cargo build --release",
    "npm --prefix cas-hub run build",
    "pnpm test",
    "for i in $(seq 12); do ssh prowl true && break; sleep 10; done",
    "ssh prowl 'cd ~/cassy && cargo build --release'",
    "timeout 590 bash -c 'until grep -q Summary target/run.log; do sleep 10; done'",
    "gh run watch 123",
];

#[test]
fn cas_803f_supervisor_long_foreground_bash_is_flagged() {
    let mut env = TestEnvGuard::temp_home();
    env.remove("CAS_FACTORY_SESSION");
    // cas-4acdf: an Agent Teams supervisor carries its own agent_id on every hook.
    for agent_id in [None, Some("supervisor@fixture-team")] {
        for command in LONG {
            let codes = refusal_codes_as(serde_json::json!({"command": command}), agent_id);
            assert!(codes.as_deref().is_some_and(|codes| codes.contains(&"H-49")), "{agent_id:?} {command}: {codes:?}");
        }
    }
    let codes = refusal_codes(serde_json::json!({"command": "git fetch origin", "timeout": 600000}));
    assert!(codes.is_some_and(|codes| codes.contains(&"H-49")), "explicit timeout above 60 s");
}

#[test]
fn cas_803f_supervisor_backgrounded_long_bash_is_allowed() {
    let _env = TestEnvGuard::temp_home();
    for command in LONG {
        let codes = refusal_codes(serde_json::json!({"command": command, "run_in_background": true}));
        assert!(!codes.unwrap_or_default().contains(&"H-49"), "{command}");
    }
}

#[test]
fn cas_803f_supervisor_short_bash_is_allowed() {
    let _env = TestEnvGuard::temp_home();
    for (command, timeout) in [
        ("git status", None),
        ("cargo --version", None),
        ("ssh prowl uptime", None),
        ("timeout 30 git fetch origin", None),
        ("git commit -m 'run cargo build later'", None),
        ("grep -n 'npm run build' docs/notes.md", None),
        ("git log --oneline -5", Some(60000)),
    ] {
        let mut tool_input = serde_json::json!({"command": command});
        if let Some(timeout) = timeout {
            tool_input["timeout"] = timeout.into();
        }
        assert!(!refusal_codes(tool_input).unwrap_or_default().contains(&"H-49"), "{command}");
    }
}
