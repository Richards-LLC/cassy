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

/// Only the literal package-scoped check shape is exempt from assembly.
#[test]
fn worker_rust_builds_are_denied_naming_the_assembly_rule() {
    for command in [
        "cargo test",
        "cargo test -p cas --no-fail-fast",
        "cargo check -p cas --lib --tests",
        "cargo check -p cas --tests --workspace",
        "cargo check -p '*' --tests",
        "cargo +nightly check -p cas --tests",
        "cargo check -p cas --tests && cargo test",
        "cargo check -p cas --tests; cargo build",
        "cargo check -p cas --tests | tee check.log",
        "cargo check -p cas --tests --config build.jobs=64",
        "CARGO_BUILD_JOBS=64 cargo check -p cas --tests",
        "bash -c 'cargo check -p cas --tests'",
        "nice -n 10 cargo check -p cas --tests",
        "/usr/bin/cargo check -p cas --tests",
        "cargo build --release",
        "cargo +nightly clippy -p cas",
        "cargo -C cas-cli check",
        "RUSTC_WRAPPER=sccache cargo nextest run -p cas",
        "cd cas-cli && cargo check",
        "timeout 580 cargo check -p cas --lib --tests",
        "timeout -s KILL 60 cargo test",
        "nice -n 10 cargo build",
        "cargo test -p cas --test cli_test --no-run",
        "scripts/run-scoped-tests.sh -p cas --lib hooks::",
        "bash scripts/run-scoped-tests.sh -p cas --lib hooks::",
        "SCOPED_PROOF_BASE=abc scripts/run-scoped-tests.sh --proof -p cas --lib",
        "scripts/run-verified-tests.sh test -p cas --doc",
        "rustc src/main.rs",
        "make -C cas-cli test-scoped SCOPED_ARGS='-p cas --lib x'",
        "make test-release-panic",
        "bash -c 'cargo check -p cas'",
        "sh -lc \"cd cas-cli && cargo test\"",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        let reason = deny_reason(&out).unwrap_or_else(|| panic!("expected deny for {command:?}"));
        assert!(
            reason.contains("NO WORKER RUST BUILDS"),
            "{command:?}: {reason}"
        );
        assert!(reason.contains("cas-4cbb"), "{reason}");
        assert!(reason.contains("assembly"), "{reason}");
    }
}

#[test]
fn literal_worker_check_is_rewritten_to_the_capped_runner_for_both_harnesses() {
    use crate::test_support::TestEnvGuard;
    for harness in ["claude", "codex"] {
        let _env = TestEnvGuard::with_vars(&[("CAS_HOOK_HARNESS", harness)]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        for command in [
            "cargo check -p cas --tests",
            "cargo check -p cas -p cas-pty --tests",
            "cargo check --tests -p cas > /tmp/worker-check.log 2>&1 &",
        ] {
            let out = handle_pre_tool_use(&input(command, "worker"), Some(&root)).unwrap();
            assert!(deny_reason(&out).is_none(), "{command}: {out:?}");
            let value = serde_json::to_value(&out).unwrap();
            let rewritten = value
                .pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|v| v.as_str())
                .expect("updated command");
            assert!(
                rewritten.contains("factory worker-check --cas-root"),
                "{rewritten}"
            );
            assert!(
                rewritten.contains("-- -p cas --tests")
                    || rewritten.contains("-- --tests -p cas")
                    || rewritten.contains("-- -p cas -p cas-pty --tests")
            );
            if command.ends_with('&') {
                assert!(rewritten.ends_with("2>&1 &"));
            }
            if harness == "codex" {
                assert!(
                    value
                        .pointer("/hookSpecificOutput/permissionDecision")
                        .is_none()
                );
            }
        }
    }
}

#[test]
fn check_without_a_shared_root_fails_closed() {
    let out = handle_pre_tool_use(&input("cargo check -p cas --tests", "worker"), None).unwrap();
    assert!(deny_reason(&out).unwrap().contains("Cassy root"));
}

#[test]
fn worker_read_only_and_non_build_commands_are_not_denied() {
    for command in [
        "cargo fmt --all -- --check",
        "rustfmt --edition 2024 --check --config skip_children=true src/lib.rs",
        "cargo metadata --format-version 1",
        "cargo tree -p cas",
        "cargo --version",
        "rustc --version",
        "echo 'cargo test'",
        "git commit -m \"skip cargo check in the brief\"",
        "grep -rn 'cargo build' docs",
        "make -C cas-cli install-tools",
        "npm test",
        "npx vitest run",
    ] {
        let out = handle_pre_tool_use(&input(command, "worker"), None).expect("handler ok");
        assert!(
            deny_reason(&out).is_none(),
            "read-only/non-Rust command must not be denied: {command:?}"
        );
    }
}

#[test]
fn supervisor_retains_full_suite_authority() {
    let out = handle_pre_tool_use(&input("cargo nextest run -p cas", "supervisor"), None)
        .expect("handler ok");
    assert!(deny_reason(&out).is_none());
}
