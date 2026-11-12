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
            "npx playwright test qa/real-walk.spec.js --full",
            // A later --help can be an option's value, and these still run the suite.
            "npx playwright test --grep-invert --help",
            // GH #1852: --list as an option value or file filter still runs.
            "npx playwright test --grep-invert --list",
            "npx playwright test -- --list",
            "npx playwright test --list --full",
            "npx playwright test --reporter --help",
            // GH #1876: the seed is consumed by --shuffle, so nothing names a spec.
            "npx playwright test --shuffle 42",
            "npx playwright test --shuffle=42",
            "npx playwright test qa/a.spec.ts --shuffle=42 --full",
            // Optional values consume spec-looking strings too (GH #1876 review).
            "pnpm exec playwright test --shuffle tests/qa-coverage/a.spec.ts --workers=1",
            "npx playwright test --debug qa/a.spec.ts",
            "npx playwright test --only-changed qa/a.spec.ts",
            "npx playwright test --update-snapshots qa/a.spec.ts",
            "npx playwright test -u qa/a.spec.ts",
            "npx playwright test --project chromium qa/a.spec.ts",
            "npx playwright test -- qa/a.spec.ts",
            "npx playwright test -- --help",
            "npx playwright test --help --full",
            "npx playwright test --help; npx playwright test",
            "node scripts/run-verified-tests.mjs playwright --help",
            "npm run journeys -- qa/real-walk.spec.js --full=true",
            "npx playwright test .journey.ts",
            "npx playwright test .spec.mjs",
            "npx playwright test .spec.ts",
            "npx playwright test qa/.spec.ts",
            "npx playwright test ..journey.ts",
            r"npx playwright test '\.journey.ts'",
            "npx playwright test '.+.journey.ts'",
            "npx playwright test qa/_.spec.ts",
            r"npx playwright test 'qa/real\walk.spec.mjs'",
            "npx playwright test 'qa/real+walk.spec.mjs'",
            "npx playwright test 'qa/(walk).spec.ts'",
            "npx playwright test 'qa/one|two.spec.ts'",
            "npx playwright test 'qa/walk{1}.spec.js'",
            "npx playwright test 'qa/^walk.spec.tsx'",
            "npx playwright test qa/.walk.spec.ts",
            "npx playwright test ./qa/real-walk.spec.mjs",
            "npx playwright test qa/real-walk.spec.mjs '.+.journey.ts'",
            "npx playwright test qa/real-walk.spec.mjs journey",
            "npx playwright test qa/real-walk.spec.mjs > target/pw.log journey",
            "npx playwright test > qa/real-walk.spec.mjs",
            "npx playwright test qa/real-walk.spec.mjs 2 > target/pw.log",
            "npm run journeys -- --workers=4",
            "npm run test:journeys",
            "npm exec playwright test -- --project=journeys",
            "node scripts/run-verified-tests.mjs playwright --project=journeys",
            "node scripts/run-verified-tests.mjs playwright test qa/real-walk.spec.ts",
            "bash -c 'npx playwright test'",
            "npx playwright test --grep='.*'",
            "npx playwright test --config config.spec.ts",
            "npx playwright test '*.spec.mjs'",
            "npx playwright test 'walk?.spec.js'",
            "npx playwright test 'walk[12].spec.tsx'",
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
    assert!(!directory.path().join("scripts/journey-eval.sh").exists());
    for command in [
        "scripts/journey-eval.sh /artifacts/cas-fixture",
        "scripts/journey-eval.sh /artifacts/cas-fixture --affected abc --workers=4",
        "npx playwright test e2e/journeys/answer-ask.journey.ts --workers=1",
        "npx playwright test qa/real-walk.spec.mjs --workers=1",
        "npx playwright test qa/2.spec.ts --workers=1",
        "npx playwright test qa/real-walk.spec.ts:12 --workers=1",
        "node scripts/run-verified-tests.mjs playwright qa/real-walk.spec.ts:12:5 --workers=1",
        "npx playwright test qa/real-walk.spec.mjs 2>&1 | tail -50",
        "npx playwright test qa/real-walk.spec.mjs > target/pw.log 2>&1",
        "PLAYWRIGHT_HTML_OPEN=never npx playwright test tests/x.spec.ts:42 --debug=cli > pw-debug.log 2>&1 &",
        "npx playwright test qa/real-walk.spec.mjs --retries 0 -j 1 --trace on --repeat-each 2 --max-failures 1 --browser chromium --global-timeout 30000 --tsconfig tsconfig.json",
        "npx playwright test qa/real-walk.spec.js --workers=1",
        // GH #1876: --shuffle takes an optional seed that is not a file filter.
        "pnpm exec playwright test tests/qa-coverage/am-hubspot-integrations.spec.ts -c tests/qa-coverage/playwright.hubspot-integrations.config.ts --trace on --workers=1 --shuffle 42 --reporter=json",
        "pnpm exec playwright test tests/qa-coverage/a.spec.ts --shuffle arbitrary-seed --workers=1",
        "pnpm exec playwright test qa/a.spec.ts --debug cli --only-changed main --shuffle arbitrary-seed --update-snapshots changed -u missing --workers=1",
        "pnpm exec playwright test --shuffle -1e-3 qa/a.spec.ts --workers=1",
        "pnpm exec playwright test --shuffle --workers=1 qa/a.spec.ts",
        "pnpm exec playwright test --project chromium --workers=1 qa/a.spec.ts",
        "npx playwright test tests/e2e/explorer-login.spec.cjs --workers=1",
        "NODE_PATH=/cached/node_modules npx --prefix /cached playwright test scripts/tests/decision-workbook.spec.js --config scripts/tests/decision-workbook.playwright.config.cjs",
        "npx --prefix=/cached playwright test scripts/tests/decision-workbook.spec.cjs --config=scripts/tests/decision-workbook.playwright.config.cjs",
        "npx playwright test /task-artifacts/independent-qa/round-1/independent.spec.cjs --config /task-artifacts/independent-qa/round-1/independent.config.cjs --workers=1 > target/independent-browser.log 2>&1",
        "npx playwright test qa/real-walk.spec.tsx --workers=1",
        "npm run journeys -- --grep=HUB-J7 --workers=4",
        "npx playwright test --grep 'HUB-J7|HUB-J12'",
        "npx playwright --version",
        "npx playwright test --help",
        "npx playwright test -h",
        "node node_modules/@playwright/test/cli.js test --help",
        "npx playwright test --help 2>&1 | head -40",
        // GH #1852: collection only; nothing launches.
        "pnpm exec playwright test --list > target/collection.log 2>&1",
        "pnpm exec playwright test --list --forbid-only --shuffle=42",
        "pnpm exec playwright test --list --quiet --shuffle 42",
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
        for role in ["worker", "qa"] {
            let reason = denial(command, role);
            assert!(reason.is_none(), "{role}: {command}: {reason:?}");
        }
    }
    for command in [
        "scripts/journey-eval.sh /artifacts/cas-fixture --full",
        "npx playwright test",
    ] {
        assert!(denial(command, "supervisor").is_none(), "{command}");
    }
}
