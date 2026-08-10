use crate::hooks::handlers::handle_pre_tool_use;
use cas_core::hooks::types::{HookInput, HookOutput};

fn input(command: &str, role: &str) -> HookInput {
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Bash".into()),
        tool_input: Some(serde_json::json!({"command": command})),
        agent_role: Some(role.into()),
        ..HookInput::default()
    }
}

fn deny_reason(out: &HookOutput) -> Option<String> {
    let value = serde_json::to_value(out.hook_specific_output.as_ref()?).ok()?;
    if value.get("permissionDecision")?.as_str()? != "deny" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|reason| reason.as_str())
        .map(str::to_string)
}

#[test]
fn worker_unscoped_cargo_test_and_nextest_are_denied_with_scoped_recipe() {
    for command in [
        "cargo test",
        "cargo test -p cas --no-fail-fast",
        "RUSTC_WRAPPER=sccache cargo nextest run -p cas",
        "cargo check -p cas && cargo test store::tests",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        let reason = deny_reason(&out).unwrap_or_else(|| panic!("expected deny for {command:?}"));
        assert!(reason.contains("UNSCOPED WORKER TEST RUN"), "{reason}");
        assert!(reason.contains("scripts/run-scoped-tests.sh"), "{reason}");
        assert!(
            reason.contains("cargo check -p cas --lib --tests"),
            "{reason}"
        );
        assert!(reason.contains("supervisor integration merge"), "{reason}");
    }
}

#[test]
fn worker_target_scopes_and_non_test_commands_are_not_denied() {
    for command in [
        "cargo nextest run -p cas --lib hooks::",
        "cargo test -p cas --test cli_test",
        "cargo test -p cas --doc",
        "cargo check -p cas --lib --tests",
        "scripts/run-scoped-tests.sh -p cas --lib hooks::",
        "echo 'cargo test'",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        assert!(
            deny_reason(&out).is_none(),
            "target-scoped/non-test command must not be denied: {command:?}"
        );
    }
}

#[test]
fn supervisor_retains_full_suite_authority() {
    let out = handle_pre_tool_use(&input("cargo nextest run -p cas", "supervisor"), None)
        .expect("handler ok");
    assert!(deny_reason(&out).is_none());
}
