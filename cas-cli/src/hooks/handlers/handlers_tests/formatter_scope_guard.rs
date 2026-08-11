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
fn worker_mutating_cargo_fmt_is_denied() {
    for command in [
        "cargo fmt",
        "cargo fmt --all && git diff --check",
        "cargo +nightly fmt -p cas-store",
        "/usr/bin/cargo fmt -p cas",
        "cargo fmt --check || cargo fmt",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        let reason = deny_reason(&out).unwrap_or_else(|| panic!("expected deny for {command:?}"));
        assert!(reason.contains("UNSCOPED WORKER FORMAT RUN"), "{reason}");
        assert!(reason.contains("skip_children=true"), "{reason}");
        assert!(reason.contains("operator approval"), "{reason}");
    }
}

#[test]
fn worker_recursive_rustfmt_is_denied() {
    for command in [
        "rustfmt --edition 2024 crates/cas-store/src/lib.rs",
        "/usr/bin/rustfmt --edition 2024 cas-cli/src/cli/mod.rs",
        "git status --short; rustfmt task.rs",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        assert!(
            deny_reason(&out).is_some(),
            "recursive formatter must be denied: {command:?}"
        );
    }
}

#[test]
fn checks_and_non_recursive_rustfmt_remain_available() {
    for command in [
        "cargo fmt --check",
        "cargo fmt --all -- --check",
        "cargo fmt --version",
        "rustfmt --edition 2024 --check crates/cas-store/src/lib.rs",
        "rustfmt --edition 2024 --emit stdout task.rs",
        "rustfmt --edition 2024 --emit=stdout task.rs",
        "rustfmt --edition 2024 --config skip_children=true crates/cas-store/src/lib.rs",
        "rustfmt --edition 2024 --config=skip_children=true task.rs",
        "rustfmt --version",
        "rustfmt --print-config default",
        "echo 'cargo fmt --all'",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        assert!(
            deny_reason(&out).is_none(),
            "safe formatter command must remain available: {command:?}"
        );
    }
}

#[test]
fn supervisor_retains_normalization_authority() {
    for command in [
        "cargo fmt --all",
        "rustfmt --edition 2024 crates/cas-store/src/lib.rs",
    ] {
        let out = handle_pre_tool_use(&input(command, "supervisor"), None).expect("handler ok");
        assert!(
            deny_reason(&out).is_none(),
            "supervisor command: {command:?}"
        );
    }
}

#[test]
fn codex_routes_unified_exec_through_the_pre_tool_hook() {
    let hooks: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../.codex/hooks.json"))
            .expect("valid project Codex hooks JSON");
    let pre_tool = hooks["hooks"]["PreToolUse"]
        .as_array()
        .expect("PreToolUse hook list");

    assert!(pre_tool.iter().any(|entry| {
        entry["matcher"] == "^Bash$"
            && entry["hooks"].as_array().is_some_and(|handlers| {
                handlers
                    .iter()
                    .any(|handler| {
                        handler["command"] == "CAS_HOOK_HARNESS=codex cas hook PreToolUse"
                    })
            })
    }));
}
