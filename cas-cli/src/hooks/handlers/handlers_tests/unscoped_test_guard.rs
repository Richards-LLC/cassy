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

fn init_ignored_log_repo(dir: &std::path::Path) {
    assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(dir).status().unwrap().success());
    std::fs::write(dir.join(".gitignore"), "/target/\n").unwrap();
}

#[test]
fn cas_c0ec_rewritten_worker_logs_require_ignored_or_external_paths() {
    use crate::test_support::TestEnvGuard;
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().to_str().unwrap();
    init_ignored_log_repo(dir.path());
    let root = dir.path().join(".cas");
    std::fs::create_dir(&root).unwrap();
    let _env = TestEnvGuard::with_vars(&[("CAS_HOOK_HARNESS", "claude"), ("CAS_CLONE_PATH", worktree)]);
    for command in [
        "cargo check -p cas --lib > worker-check.log 2>&1 &",
        "cargo check -p cas --tests >> worker-check.log 2>&1 &",
        "cargo nextest run -p cas --lib -E 'test(one)' > worker-tests.log 2>&1 &",
    ] {
        let mut request = input(command, "worker");
        request.cwd = worktree.into();
        let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
        let reason = deny_reason(&out).expect("unignored log would dirty the admitted check");
        assert!(reason.contains("WORKER CHECK LOG"), "{reason}");
        assert!(reason.contains("target/worker-check.log"), "{reason}");
    }
    #[cfg(unix)]
    {
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("source.rs"), "// source").unwrap();
        std::os::unix::fs::symlink("../source.rs", dir.path().join("target/source.log")).unwrap();
        let mut request = input("cargo check -p cas --lib > target/source.log 2>&1 &", "worker");
        request.cwd = worktree.into();
        let reason = deny_reason(&handle_pre_tool_use(&request, Some(&root)).unwrap()).unwrap();
        assert!(reason.contains("WORKER CHECK LOG"));
    }
    let artifacts = tempfile::tempdir().unwrap();
    let mut config = crate::config::Config::default();
    let mut factory = config.factory();
    factory.artifacts_root = Some(artifacts.path().to_string_lossy().into_owned());
    config.factory = Some(factory);
    config.save(&root).unwrap();
    let task_artifacts = crate::config::project_factory_artifacts_root(&root, artifacts.path()).join("cas-c0ec");
    let mut request = input(&format!("cargo check -p cas --tests > {}/worker-check.log 2>&1 &", task_artifacts.display()), "worker");
    request.cwd = worktree.into();
    let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
    assert!(deny_reason(&out).is_none(), "sanctioned artifact log: {out:?}");
}

/// Only the literal package-scoped check shape is exempt from assembly.
#[test]
fn worker_rust_builds_are_denied_naming_the_assembly_rule() {
    for command in [
        "cargo nextest run -p cas -E 'all()'",
        "cargo nextest run -p cas -E ''",
        "cargo nextest run -p cas -E 'test()'",
        "cargo nextest run -p cas -E 'test(one) | all()'",
        "cargo nextest run -p cas -p cas --lib -E 'test(one)'",
        "cargo nextest run --workspace -E 'test(one)'",
        "cargo nextest run -p cas --release -E 'test(one)'",
        "cargo nextest run -p cas --test one --test two -E 'test(one)'",
        "cargo nextest run -p cas --lib --test one -E 'test(one)'",
        "cargo nextest run -p cas --no-fail-fast",
        "cargo nextest run -p cas --no-fail-fast -E 'test(one)'",
        "cargo nextest run -p cas -E test(one)",
        "cargo nextest run -p cas -E \"test($NAME)\"",
        "env -u X cargo nextest run -p cas --test integration_cli -E 'test(x)'",
        // cas-cfd6: env/sudo options that take a value must not hide cargo.
        "env -u CAS_AGENT_NAME -u CLAUDE_X cargo nextest run -p cas --lib -E 'test(x)'",
        "env --unset CAS_AGENT_NAME cargo check -p cas --lib",
        "env -C cas-cli cargo nextest run -p cas -E 'test(one)'",
        "env --chdir cas-cli cargo check -p cas --tests",
        "env -i PATH=/usr/bin cargo check -p cas --lib",
        "env -S 'cargo nextest run -p cas -E test(one)'",
        "env --split-string='cargo check -p cas --lib'",
        "/usr/bin/env -u X cargo-nextest nextest run -p cas -E 'test(one)'",
        "sudo -u root cargo nextest run -p cas -E 'test(one)'",
        "timeout 60 env -u X cargo check -p cas --lib",
        "CARGO_BUILD_JOBS=64 cargo nextest run -p cas -E 'test(one)'",
        "cargo nextest run -p cas -E 'test(one)' && cargo build",
        "cargo nextest run -p cas -E 'test(one)'; cargo build",
        "cargo nextest run -p cas -E 'test(one)' | tee check.log",
        "cargo nextest run\n-p cas -E 'test(one)'",
        "cargo nextest run -p cas -E 'test(one)'\ncargo build",
        "cargo test",
        "cargo test -p cas --no-fail-fast",
        "cargo check -p cas --lib --tests",
        "cargo check -p cas --tests --lib",
        "cargo check -p cas --lib --lib",
        "cargo check -p cas --lib --all-targets",
        "cargo check -p cas --lib --workspace",
        "cargo check -p cas --lib --config build.jobs=64",
        "CARGO_BUILD_JOBS=64 cargo check -p cas --lib",
        "cargo check -p cas --lib && cargo test",
        "cargo check -p cas --lib; cargo build",
        "cargo check -p cas --lib | tee check.log",
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
        let dir = tempfile::tempdir().unwrap();
        init_ignored_log_repo(dir.path());
        let worktree = dir.path().to_str().unwrap();
        let _env =
            TestEnvGuard::with_vars(&[("CAS_HOOK_HARNESS", harness), ("CAS_CLONE_PATH", worktree)]);
        let root = dir.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        for command in [
            "cargo check -p cas --lib",
            "cargo check -p cas -p cas-pty --lib",
            "cargo check --lib -p cas > target/worker-check.log 2>&1 &",
            "cargo check -p cas --tests",
            "cargo check -p cas -p cas-pty --tests",
            "cargo check --tests -p cas > target/worker-check.log 2>&1 &",
        ] {
            let mut request = input(command, "worker");
            request.cwd = worktree.into();
            let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
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
            let args = command.strip_prefix("cargo check ").unwrap();
            assert!(rewritten.contains(&format!("-- {args}")), "{rewritten}");
            if command.ends_with('&') {
                assert!(rewritten.ends_with("2>&1 &"));
            }
            // cas-980d: Codex applies updatedInput only with an allow
            // decision; without it Codex runs the original command.
            if harness == "codex" {
                assert_eq!(
                    value
                        .pointer("/hookSpecificOutput/permissionDecision")
                        .and_then(|v| v.as_str()),
                    Some("allow"),
                    "{value}"
                );
            }
        }
        // Rewriting through the capped runner must preserve the workspace
        // contract, rather than granting the check a bare /tmp escape hatch.
        for command in [
            "cargo check --lib -p cas > /tmp/worker-check.log 2>&1 &",
            "cargo check --tests -p cas > /tmp/worker-check.log 2>&1 &",
        ] {
            let mut request = input(command, "worker");
            request.cwd = worktree.into();
            let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
            let reason = deny_reason(&out).expect("bare /tmp must remain denied after rewrite");
            assert!(reason.contains("FACTORY WORKSPACE CONTRACT"), "{reason}");
            assert!(reason.contains("/tmp/worker-check.log"), "{reason}");
        }
    }
}

#[test]
fn targeted_nextest_rewrite_preserves_filter_literals_and_workspace_guard() {
    use crate::test_support::TestEnvGuard;
    for harness in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        init_ignored_log_repo(dir.path());
        let cwd = dir.path().to_str().unwrap();
        let _env =
            TestEnvGuard::with_vars(&[("CAS_HOOK_HARNESS", harness), ("CAS_CLONE_PATH", cwd)]);
        let root = dir.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        for command in [
            "cargo nextest run -p cas -E 'test(hooks::handlers)'",
            "cargo nextest run -p cas --lib -E 'test(=module::name)'",
            "cargo nextest run -p cas --test integration_factory -E 'test(worker)'",
            "cargo nextest run -p cas --lib -E 'test(one) | test(two)' > target/tests.log 2>&1 &",
            "cargo nextest run -p cas --lib -E 'test(one) & test(two)' > target/tests.log 2>&1 &",
        ] {
            let mut request = input(command, "worker");
            request.cwd = cwd.into();
            let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
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
                rewritten.contains(command.strip_prefix("cargo ").unwrap()),
                "{rewritten}"
            );
        }
        let mut request = input(
            "cargo nextest run -p cas --lib -E 'test(worker)' > /tmp/tests.log 2>&1 &",
            "worker",
        );
        request.cwd = cwd.into();
        let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
        assert!(
            deny_reason(&out)
                .unwrap()
                .contains("FACTORY WORKSPACE CONTRACT")
        );
    }
    let out = handle_pre_tool_use(
        &input("cargo nextest run -p cas -E 'test(worker)'", "worker"),
        None,
    )
    .unwrap();
    assert!(deny_reason(&out).unwrap().contains("Cassy root"));
}

#[test]
fn check_without_a_shared_root_fails_closed() {
    let out = handle_pre_tool_use(&input("cargo check -p cas --tests", "worker"), None).unwrap();
    assert!(deny_reason(&out).unwrap().contains("Cassy root"));
}

#[test]
fn worker_read_only_and_non_build_commands_are_not_denied() {
    // The explicit HookInput role exercises the Rust command guard. Ambient
    // factory identity would additionally reject this fixture's /test Git cwd.
    let _env = crate::test_support::TestEnvGuard::new();
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
            "read-only/non-Rust command must not be denied: {command:?}: {out:?}"
        );
    }
}

#[test]
fn worker_suite_admission_warns_without_helper_and_rewrites_with_helper_cas_61dc() {
    use crate::test_support::TestEnvGuard;
    for harness in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let _env = TestEnvGuard::with_vars(&[("CAS_HOOK_HARNESS", harness), ("CAS_CLONE_PATH", cwd)]);
        let root = dir.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        for has_helper in [false, true] {
            if has_helper {
                std::fs::create_dir(dir.path().join("scripts")).unwrap();
                std::fs::write(dir.path().join("scripts/worker-memory.py"), "# fixture, never executed\n").unwrap();
            }
            // cas-3ae7: an unfiltered Playwright suite is denied to workers by the
            // browser tier guard first, so admission is exercised on a named spec.
            for command in ["npm test", "npm run test", "npx playwright test e2e/journeys/answer-ask.journey.ts", "vitest run", "bash scripts/journey-eval.sh", "npm run typecheck", "npm run build", "tsc --noEmit", "vite build", "vitest run --maxWorkers=2"] {
                let mut request = input(command, "worker");
                request.cwd = cwd.into();
                let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
                assert!(deny_reason(&out).is_none(), "{command}: {out:?}");
                let value = serde_json::to_value(&out).unwrap();
                let rewritten = value.pointer("/hookSpecificOutput/updatedInput/command").and_then(|value| value.as_str());
                if has_helper {
                    let rewritten = rewritten.expect("suite routed through host admission");
                    assert!(rewritten.contains("worker-memory.py") && rewritten.contains(command), "{rewritten}");
                    // cas-4cb9: pass the actual payload for weighted admission;
                    // a caller-supplied light hint must never hide a heavy suite.
                    assert!(rewritten.contains(" --shell-command "), "{rewritten}");
                    if harness == "codex" {
                        assert_eq!(value.pointer("/hookSpecificOutput/permissionDecision").and_then(|value| value.as_str()), Some("allow"));
                    }
                    assert!(out.system_message.is_none(), "helper present: {out:?}");
                } else {
                    assert!(rewritten.is_none(), "no unavailable helper rewrite: {out:?}");
                    assert!(out.system_message.as_deref().is_some_and(|message| message.contains("shared host memory admission is unavailable")), "{out:?}");
                }
            }
            for command in ["npm config get registry", "npm view vitest version", "node -e 'require(\"fs\").readFileSync(\"secrets.json\")'", "node scripts/generate-tokens.mjs"] {
                let mut request = input(command, "worker");
                request.cwd = cwd.into();
                let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
                assert!(deny_reason(&out).is_none(), "{command}: {out:?}");
                let value = serde_json::to_value(&out).unwrap();
                assert!(value.pointer("/hookSpecificOutput/updatedInput").is_none(), "plain read/script: {out:?}");
                assert!(out.system_message.is_none(), "plain read/script: {out:?}");
            }
        }
        // Admission preserves the unwrapped command's credential decision.
        let mut request = input("npm test; node -e 'require(\"fs\").writeFileSync(\"secrets.json\", \"FIXTURE\")'", "worker");
        request.cwd = cwd.into();
        let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
        let reason = deny_reason(&out).expect("credential write remains denied");
        assert!(reason.contains("credentials"), "the credential guard's own reason survives admission: {reason}");
        let value = serde_json::to_value(&out).unwrap();
        assert!(value.pointer("/hookSpecificOutput/updatedInput").is_none(), "deny cannot be rewritten into allow: {out:?}");
    }
}

#[test]
fn worker_suite_rewrite_preserves_original_denials_cas_61dc() {
    use crate::test_support::TestEnvGuard;
    for harness in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let _env = TestEnvGuard::with_vars(&[("CAS_HOOK_HARNESS", harness), ("CAS_CLONE_PATH", cwd)]);
        let root = dir.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/worker-memory.py"), "# never executed\n").unwrap();
        for forbidden in [
            "node -e 'require(\"fs\").writeFileSync(\"secrets.json\", \"FIXTURE\")'",
            "printf x > .env",
            "cargo test",
        ] {
            let mut direct = input(forbidden, "worker");
            direct.cwd = cwd.into();
            let expected = deny_reason(&handle_pre_tool_use(&direct, Some(&root)).unwrap())
                .expect("original command must be refused");
            for command in [format!("npm test; {forbidden}"), format!("{forbidden}; npx vitest run")] {
                let mut request = input(&command, "worker");
                request.cwd = cwd.into();
                let out = handle_pre_tool_use(&request, Some(&root)).unwrap();
                assert_eq!(deny_reason(&out).as_deref(), Some(expected.as_str()), "{harness}: {command}: {out:?}");
                let value = serde_json::to_value(&out).unwrap();
                assert!(value.pointer("/hookSpecificOutput/updatedInput").is_none(), "refused command cannot acquire a rewrite: {out:?}");
            }
        }
    }
}

#[test]
fn supervisor_retains_full_suite_authority() {
    let out = handle_pre_tool_use(&input("cargo nextest run -p cas", "supervisor"), None)
        .expect("handler ok");
    assert!(deny_reason(&out).is_none());
}

/// cas-cf70 fixture: a Makefile with one script-only target and the shapes
/// that must stay refused.
fn cf70_makefile_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("cas-cli")).unwrap();
    std::fs::write(
        dir.path().join("cas-cli/Makefile"),
        "CARGO ?= cargo\n\
         .PHONY: test-ci-tiers test-rust\n\
         \n\
         # Script-only CI fixtures.\n\
         test-ci-tiers:\n\
         \tcd .. && bash scripts/test-a.sh\n\
         \tcd .. && python3 scripts/test-b.py\n\
         \tcd .. && ./scripts/test-c.sh\n\
         \n\
         test-rust:\n\
         \t$(CARGO) nextest run -p cas\n\
         \n\
         test-literal-cargo:\n\
         \tcargo test -p cas\n\
         \n\
         test-needs-rust: test-rust\n\
         \tcd .. && bash scripts/test-a.sh\n\
         \n\
         test-submake:\n\
         \tmake test-rust\n\
         \n\
         test-variable:\n\
         \t$(RUNNER) scripts/test-a.sh\n\
         \n\
         check:\n\
         \t$(CARGO) check -p cas\n",
    )
    .unwrap();
    dir
}

/// cas-cf70: `make -C cas-cli test-ci-tiers` runs only Python and Bash CI
/// fixtures, which the Makefile shows; the build guard admits it, read from
/// the target's own recipe.
#[test]
fn script_only_make_target_is_admitted_cas_cf70() {
    let dir = cf70_makefile_dir();
    let mut request = input("make -C cas-cli test-ci-tiers", "worker");
    request.cwd = dir.path().to_str().unwrap().into();
    let out = handle_pre_tool_use(&request, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none_or(|reason| !reason.contains("NO WORKER RUST BUILDS")),
        "a script-only target is not a Rust build: {out:?}"
    );

    // The real target in this repository, as reported.
    let root = crate::test_paths::workspace_root();
    let mut request = input("make -C cas-cli test-ci-tiers", "worker");
    request.cwd = root.to_str().unwrap().into();
    let out = handle_pre_tool_use(&request, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none_or(|reason| !reason.contains("NO WORKER RUST BUILDS")),
        "cas-cli/Makefile test-ci-tiers runs only CI fixtures: {out:?}"
    );
}

/// cas-cf70: the admission comes from the target, not from `make`: Rust
/// recipes, Rust prerequisites, sub-makes, unexpanded variables, unknown
/// targets, a mixed target list and an unreadable Makefile stay refused.
#[test]
fn rust_or_unprovable_make_targets_stay_refused_cas_cf70() {
    let dir = cf70_makefile_dir();
    for command in [
        "make -C cas-cli test-rust",
        "make -C cas-cli test-literal-cargo",
        "make -C cas-cli test-needs-rust",
        "make -C cas-cli test-submake",
        "make -C cas-cli test-variable",
        "make -C cas-cli test-unknown",
        "make -C cas-cli check",
        "make -C cas-cli test-ci-tiers test-rust",
        "make -C missing test-ci-tiers",
        "bash -c 'make -C cas-cli test-rust'",
    ] {
        let mut request = input(command, "worker");
        request.cwd = dir.path().to_str().unwrap().into();
        let out = handle_pre_tool_use(&request, None).expect("handler ok");
        let reason = deny_reason(&out).unwrap_or_else(|| panic!("expected deny for {command:?}"));
        assert!(reason.contains("NO WORKER RUST BUILDS"), "{command:?}: {reason}");
    }
}

/// What Codex runs for a Bash call given a PreToolUse hook's stdout. This
/// mirrors codex-rs/hooks/src/engine/output_parser.rs (`parse_pre_tool_use`
/// and `unsupported_pre_tool_use_hook_specific_output`):
/// - `updatedInput` without `permissionDecision: "allow"` is invalid;
/// - `allow` without `updatedInput` is invalid;
/// - an invalid output fails open, so the original command runs;
/// - a deny with a reason blocks;
/// - only `allow` plus `updatedInput.command` replaces the command.
/// `None` means Codex blocks the call.
fn codex_effective_command(hook_stdout: &serde_json::Value, original: &str) -> Option<String> {
    let Some(specific) = hook_stdout.get("hookSpecificOutput") else {
        return Some(original.to_string());
    };
    assert_eq!(specific["hookEventName"], "PreToolUse", "Codex requires hookEventName");
    let decision = specific.get("permissionDecision").and_then(|v| v.as_str());
    let updated = specific.get("updatedInput");
    let invalid = (updated.is_some() && decision != Some("allow"))
        || (decision == Some("allow") && updated.is_none())
        || decision == Some("ask");
    if invalid {
        return Some(original.to_string());
    }
    if decision == Some("deny") {
        return None;
    }
    match (decision, updated) {
        (Some("allow"), Some(updated)) => Some(
            updated["command"]
                .as_str()
                .expect("Codex maps updatedInput.command back to exec_command's cmd")
                .to_string(),
        ),
        _ => Some(original.to_string()),
    }
}

/// cas-980d: a Codex worker's capped `cargo check`, issued through
/// `functions.exec` → `tools.exec_command` (code mode), must run through
/// `cas factory worker-check`. Observed: it ran as raw cargo with no runner and
/// no slot lock, because the hook returned `updatedInput` without
/// `permissionDecision: "allow"`, which Codex treats as invalid and fails open.
///
/// Codex sends a nested code-mode call through the same registry hook path as
/// a direct one (code_mode/mod.rs `handle_tool_call_with_source` with
/// `ToolCallSource::CodeMode` → registry.rs `dispatch_any_with_state` →
/// `run_pre_tool_use_hooks`). `exec_command`'s payload is `tool_name: "Bash"`
/// and `tool_input: {"command": <cmd>}` (unified_exec/exec_command.rs
/// `pre_tool_use_payload`). The payloads below follow Codex's PreToolUse input
/// schema field for field, so the hook is fed exactly what Codex sends.
#[test]
fn codex_nested_exec_command_check_runs_through_the_capped_runner_cas_980d() {
    use crate::test_support::TestEnvGuard;
    let dir = tempfile::tempdir().unwrap();
    init_ignored_log_repo(dir.path());
    let worktree = dir.path().to_str().unwrap();
    let root = dir.path().join(".cas");
    std::fs::create_dir(&root).unwrap();
    // A Codex worker's hook process: the harness wrapper and the worker's own
    // environment, with no agent_role field in the payload.
    let _env = TestEnvGuard::with_vars(&[
        ("CAS_HOOK_HARNESS", "codex"),
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_CLONE_PATH", worktree),
    ]);
    let codex_payload = |tool_use_id: &str, command: &str| -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "codex-worker-session",
            "turn_id": "turn-7",
            "transcript_path": null,
            "cwd": worktree,
            "hook_event_name": "PreToolUse",
            "model": "gpt-codex",
            "permission_mode": "default",
            "tool_name": "Bash",
            "tool_use_id": tool_use_id,
            "tool_input": { "command": command },
        }))
        .expect("Codex's PreToolUse payload deserializes")
    };

    for (dispatch, tool_use_id) in [("direct exec_command", "call_direct"), ("functions.exec → exec_command", "call_nested")] {
        for command in [
            "cargo check -p cas --tests",
            "cargo check -p cas --tests > target/worker-check.log 2>&1 &",
        ] {
            let out = handle_pre_tool_use(&codex_payload(tool_use_id, command), Some(&root)).unwrap();
            let stdout = serde_json::to_value(&out).unwrap();
            let runs = codex_effective_command(&stdout, command)
                .unwrap_or_else(|| panic!("{dispatch}: {command} was blocked: {stdout}"));
            assert!(
                runs.contains("factory worker-check --cas-root"),
                "{dispatch}: Codex would run `{runs}` for `{command}`, not the capped runner: {stdout}"
            );
            assert!(runs.contains("-- -p cas --tests"), "{runs}");
            if command.ends_with('&') {
                assert!(runs.ends_with("> target/worker-check.log 2>&1 &"), "{runs}");
            }
        }
    }

    // A guard that denies the rewritten command still blocks it in Codex.
    let command = "cargo check -p cas --tests > /tmp/worker-check.log 2>&1 &";
    let out = handle_pre_tool_use(&codex_payload("call_nested", command), Some(&root)).unwrap();
    let stdout = serde_json::to_value(&out).unwrap();
    assert_eq!(codex_effective_command(&stdout, command), None, "{stdout}");
}
