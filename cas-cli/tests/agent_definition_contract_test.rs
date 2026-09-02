//! Contract tests for the built-in verifier and learning-reviewer agents.
//!
//! These tests intentionally read the checked-in source files. The files are
//! embedded into the runtime catalog, so a test that only exercises the
//! catalog can miss an unregistered or stale mirror.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

fn load(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

const VERIFIER_PATHS: [&str; 3] = [
    "cas-cli/src/builtins/agents/task-verifier.md",
    "cas-cli/src/builtins/codex/agents/task-verifier.md",
    "cas-cli/src/builtins/grok/agents/task-verifier.md",
];

const REVIEWER_PATHS: [&str; 3] = [
    "cas-cli/src/builtins/agents/learning-reviewer.md",
    "cas-cli/src/builtins/codex/agents/learning-reviewer.md",
    "cas-cli/src/builtins/grok/agents/learning-reviewer.md",
];

#[test]
fn verifier_mirrors_document_the_current_close_contract() {
    for path in VERIFIER_PATHS {
        let body = load(path);
        assert!(body.contains("model: inherit"), "{path} must inherit the caller model");
        assert!(
            body.contains("files_reviewed=\"file1,file2\""),
            "{path} must record files_reviewed in every verdict template"
        );
        assert!(
            !body.contains(" files=\""),
            "{path} must not use the unknown verification field files="
        );
        assert!(
            body.contains("git diff --name-status HEAD~10 | rg -e "),
            "{path} must use ripgrep's regexp option in its test-first check"
        );
        assert!(!body.contains("| rg -E "), "{path} retains invalid rg -E syntax");
        assert!(!body.contains("VERIFICATION JAIL"), "{path} retains stale jail wording");
        for marker in [
            "⚠️ VERIFICATION REQUIRED",
            "⚠️ VERIFICATION FAILED",
            "ast-grep",
            "stranded_branch_override",
            "epic_verification_owner",
        ] {
            assert!(body.contains(marker), "{path} is missing close-gate marker {marker:?}");
        }
    }
}

#[test]
fn learning_reviewer_receives_and_consumes_explicit_ids() {
    let stop_handler = load("cas-cli/src/hooks/handlers/handlers_middle/session_stop/mod.rs");
    assert!(
        stop_handler.contains("unreviewed_ids"),
        "Stop prompt builder must construct the complete unreviewed ID list"
    );
    assert!(
        stop_handler.contains("Review these unreviewed learning IDs"),
        "Stop prompt must pass IDs to learning-reviewer"
    );

    for path in REVIEWER_PATHS {
        let body = load(path);
        assert!(
            body.contains("learning ID from the parent prompt"),
            "{path} must consume IDs supplied by the parent prompt"
        );
    }
}

#[test]
fn session_learn_stop_hook_uses_the_skill_as_its_prompt_source() {
    let handler = load("cas-cli/src/hooks/handlers/handlers_session.rs");
    assert!(
        handler.contains("include_str!(\"../../builtins/skills/session-learn/SKILL.md\")"),
        "Stop hook must embed the canonical session-learn skill body"
    );
    assert!(
        !handler.contains("You are analyzing a Claude Code session transcript"),
        "Stop hook must not retain a second inline session-learn prompt"
    );
}

#[test]
fn verifier_test_first_command_runs_on_a_fixture_repo() {
    let temp = tempfile::tempdir().expect("temporary git fixture");
    let repo = temp.path();
    run_git(repo, ["init", "--quiet"]);
    run_git(repo, ["config", "user.name", "Cassy Test"]);
    run_git(repo, ["config", "user.email", "cassy@example.invalid"]);

    fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    run_git(repo, ["add", "seed.txt"]);
    run_git(repo, ["commit", "--quiet", "-m", "seed"]);
    for index in 0..9 {
        run_git(repo, ["commit", "--quiet", "--allow-empty", "-m", &format!("history-{index}")]);
    }
    fs::write(repo.join("contract_test.rs"), "#[test] fn contract() {}\n").unwrap();
    run_git(repo, ["add", "contract_test.rs"]);
    run_git(repo, ["commit", "--quiet", "-m", "add test"]);

    // This is the exact command shape documented by task-verifier.md after
    // the fix: HEAD~10 is valid because the fixture has eleven commits, and
    // `-e` is ripgrep's pattern option (unlike the invalid `-E` encoding flag).
    let output = Command::new("sh")
        .current_dir(repo)
        .arg("-c")
        .arg("git diff --name-status HEAD~10 | rg -e '^A\\s+.*(_test\\.rs|tests/.*\\.rs)'")
        .output()
        .expect("run documented verifier command");
    assert!(
        output.status.success(),
        "documented test-first command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("contract_test.rs"),
        "fixture test file should be found by the documented command"
    );
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
