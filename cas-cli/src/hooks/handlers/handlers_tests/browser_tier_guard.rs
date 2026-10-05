use crate::hooks::handlers::handle_pre_tool_use;
use cas_core::hooks::types::HookInput;

fn request(command: &str, role: &str) -> HookInput {
    HookInput {
        session_id: "browser-tier-fixture".into(),
        cwd: std::env::var("CAS_CLONE_PATH").unwrap_or_else(|_| "/fixture".into()),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Bash".into()),
        tool_input: Some(serde_json::json!({"command": command})),
        agent_role: Some(role.into()),
        ..HookInput::default()
    }
}
fn denial(command: &str, role: &str) -> Option<String> {
    let out = handle_pre_tool_use(&request(command, role), None).unwrap();
    let value = serde_json::to_value(out.hook_specific_output?).unwrap();
    (value["permissionDecision"] == "deny").then(|| {
        value["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .to_string()
    })
}

#[test]
fn cas_c082_worker_and_qa_unfiltered_browser_runs_are_denied() {
    for role in ["worker", "qa"] {
        for command in [
            "scripts/journey-eval.sh /artifacts/cas-fixture --full",
            "bash scripts/journey-eval.sh /artifacts/cas-fixture --full --workers=4",
            "npx playwright test",
            "npm run journeys -- --workers=4",
            "npm run test:journeys",
            "npm exec playwright test -- --project=journeys",
            "node scripts/run-verified-tests.mjs playwright --project=journeys",
            "bash -c 'npx playwright test'",
            "npx playwright test --grep='.*'",
            "npx playwright test --config config.spec.ts",
            "python3 scripts/journey-receipt.py plan /artifacts --full",
            "cd hub-web && npx playwright test --workers=4",
            "echo ready\nnpx playwright test",
            "env -u CAS_AGENT_ROLE npx playwright test",
            "nice -n 10 npx playwright test",
            "cat > README.md <<'EOF'\nexample\nEOF\nnpx playwright test",
            "cat > README.md <<EOF\nexample\nEOF\nnpx playwright test",
            "cat > README.md <<\"EOF\"\nexample\nEOF\nnpx playwright test",
        ] {
            let reason = denial(command, role).expect(command);
            assert!(reason.contains("scripts/journey-eval.sh"), "{reason}");
            assert!(reason.contains("affected"), "{reason}");
        }
    }
}

#[test]
fn cas_c082_affected_and_targeted_runs_and_supervisor_full_are_allowed() {
    // The documentation heredoc is a write. Bind it to a real fixture root so
    // the independent workspace guard cannot mask the browser-tier decision.
    let directory = tempfile::tempdir().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[(
        "CAS_CLONE_PATH",
        directory.path().to_str().unwrap(),
    )]);
    for command in [
        "scripts/journey-eval.sh /artifacts/cas-fixture",
        "scripts/journey-eval.sh /artifacts/cas-fixture --affected abc --workers=4",
        "npx playwright test e2e/journeys/answer-ask.journey.ts --workers=1",
        "npm run journeys -- --grep=HUB-J7 --workers=4",
        "npx playwright test --grep 'HUB-J7|HUB-J12'",
        "npx playwright --version",
        "npx vitest run",
        "npx tsc --noEmit",
        "cat > README.md <<'EOF'\nnpx playwright test\nEOF",
        "cat > README.md <<EOF\nnpx playwright test\nEOF",
        "cat > README.md <<\"EOF\"\nnpx playwright test\nEOF",
        "cat > README.md <<-'EOF'\n\tnpx playwright test\n\tEOF",
        "printf '%s\\n' 'npx playwright test'",
        "# npx playwright test",
        "printf '%s\\n' scripts/journey-eval.sh --full",
    ] {
        let reason = denial(command, "worker");
        assert!(reason.is_none(), "{command}: {reason:?}");
    }
    for command in [
        "scripts/journey-eval.sh /artifacts/cas-fixture --full",
        "npx playwright test",
    ] {
        assert!(denial(command, "supervisor").is_none(), "{command}");
    }
}
