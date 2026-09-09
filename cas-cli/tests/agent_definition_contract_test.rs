//! Contract tests for the built-in verifier and learning-reviewer agents.
//!
//! These tests intentionally read the checked-in source files. The files are
//! embedded into the runtime catalog, so a test that only exercises the
//! catalog can miss an unregistered or stale mirror.

use std::fs;
use std::path::Path;
use std::process::Command;

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

fn load(relative: &str) -> &'static str {
    match relative {
        "cas-cli/src/hooks/handlers/handlers_session.rs" => {
            include_str!("../src/hooks/handlers/handlers_session.rs")
        }
        "cas-cli/src/hooks/handlers/handlers_middle/session_stop/mod.rs" => {
            include_str!("../src/hooks/handlers/handlers_middle/session_stop/mod.rs")
        }
        "cas-cli/src/builtins.rs" => include_str!("../src/builtins.rs"),
        _ => builtin_catalog::find_source_path(relative),
    }
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

const RULE_REVIEWER_PATHS: [&str; 3] = [
    "cas-cli/src/builtins/agents/rule-reviewer.md",
    "cas-cli/src/builtins/codex/agents/rule-reviewer.md",
    "cas-cli/src/builtins/grok/agents/rule-reviewer.md",
];

const DUPLICATE_DETECTOR_PATHS: [&str; 3] = [
    "cas-cli/src/builtins/agents/duplicate-detector.md",
    "cas-cli/src/builtins/codex/agents/duplicate-detector.md",
    "cas-cli/src/builtins/grok/agents/duplicate-detector.md",
];

/// Every shipped agent definition, in all three flavors.
///
/// cas-ef87a retired `git-history-analyzer` and `issue-intelligence-analyst`,
/// which were the only two agents that carried date guidance at all. The
/// positive half of the original contract (they must defer to the host date)
/// therefore has nothing left to assert; the durable half — no agent may
/// hard-code a calendar year — now sweeps the whole catalog instead, so it
/// still catches the next agent that tries.
fn every_agent_definition() -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    for (flavor, label) in builtin_catalog::FLAVORS {
        for builtin in builtin_catalog::agents(*flavor) {
            found.push((format!("{label}/{}", builtin.path), builtin.content));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(
        found.len() >= 15,
        "expected the three agent catalogs to be discovered, found {}",
        found.len()
    );
    found
}

#[test]
fn verifier_mirrors_document_the_current_close_contract() {
    for path in VERIFIER_PATHS {
        let body = load(path);
        assert!(
            body.contains("model: inherit"),
            "{path} must inherit the caller model"
        );
        assert!(
            body.contains("files_reviewed=\"file1,file2\""),
            "{path} must record files_reviewed in every verdict template"
        );
        assert!(
            !body.contains(" files=\""),
            "{path} must not use the unknown verification field files="
        );
        assert!(
            body.contains("git diff --name-status HEAD~10 | grep -E "),
            "{path} must use POSIX grep's extended regexp option in its test-first check"
        );
        assert!(
            !body.contains("| rg -e ") && !body.contains("| rg -E "),
            "{path} retains a ripgrep-only test-first command"
        );
        assert!(
            !body.contains("VERIFICATION JAIL"),
            "{path} retains stale jail wording"
        );
        for marker in [
            "⚠️ VERIFICATION REQUIRED",
            "⚠️ VERIFICATION FAILED",
            "ast-grep",
            "stranded_branch_override",
            "epic_verification_owner",
        ] {
            assert!(
                body.contains(marker),
                "{path} is missing close-gate marker {marker:?}"
            );
        }
    }
}

/// An epic's own demo is often empty. Pin the child-discovery route ahead of
/// the shared evidence gate so per-task success cannot bypass combined QA.
#[test]
fn epic_child_demos_require_evidence_before_close_reason() {
    for path in VERIFIER_PATHS {
        let body = load(path);
        let prerequisites = body.find("### Epic evidence prerequisites").unwrap();
        let table = body.find("| Check | REJECT when |").unwrap();
        let close_reason = body.find("### Step 0B: Check Close Reason").unwrap();
        assert!(prerequisites < table && table < close_reason, "{path}");
        assert_eq!(body.matches("| Check | REJECT when |").count(), 1, "{path}");
        let epic_gate = &body[prerequisites..body.find("### Step 0A:").unwrap()];
        for required in [
            "verification_type=epic",
            "exactly one `Epic flow walk` note",
            "~/.cas/artifacts/<epic-id>/LEDGER.md",
            "QA evidence required: epic has child demo_statement but no Epic flow walk note or LEDGER.md",
            "current assembled epic tip",
            "60-minute budget",
            "coverage mapping for every child demo",
            "missing, duplicate",
            "running, stale-tip, or incomplete-coverage",
            "counts and label split must match",
            "same Step 0A REJECT table",
            "Contradictions across",
            "Do not rerun QA",
        ] {
            assert!(
                epic_gate
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(required),
                "{path}: {required}"
            );
        }
        assert!(body[..prerequisites].contains("**any child**"), "{path}");
        assert!(body[..prerequisites].contains("closed children"), "{path}");
        for check in [
            "Required cells",
            "Source inference",
            "Failed-cell ownership",
            "Forbidden verdict",
            "Matrix breadth",
            "Headline counts",
            "PASS evidence",
        ] {
            assert!(body[table..close_reason].contains(check), "{path}: {check}");
        }
    }
}

/// Exercise the installed catalog routes, including the deliberately renamed
/// Codex checklist, rather than accepting unregistered source-only guidance.
#[test]
fn epic_walk_is_one_concurrent_pass_in_every_harness() {
    for (flavor, label) in builtin_catalog::FLAVORS {
        let get = |path| builtin_catalog::find(*flavor, path);
        let supervisor = get("skills/cas-supervisor.md");
        let checklist = get(if *flavor == builtin_catalog::Flavor::Codex {
            "skills/cas-codex-supervisor-checklist.md"
        } else {
            "skills/cas-supervisor-checklist.md"
        });
        let route = "cas-supervisor/references/epic-driving.md#epic-flow-walk";
        assert!(supervisor.contains(route), "{label} supervisor route");
        assert!(checklist.contains(route), "{label} checklist route");
        assert!(checklist.contains("release gate detached"), "{label}");
        assert!(checklist.contains("verification_type=epic"), "{label}");

        let walk = get("skills/cas-supervisor/references/epic-driving.md");
        let launch = walk.find("Launch the release").unwrap();
        let spawn = walk.find("Spawn exactly one").unwrap();
        assert!(launch < spawn, "{label}: launch the gate before QA");
        for required in [
            "one pass per epic",
            "Include closed children",
            "if none exist, skip",
            "dedicated worktree",
            "**before** dispatching",
            "gate launch or monitoring",
            "never spawn a duplicate",
            "verifier-class evidence agent",
            "not the sealed task-verifier close dispatch",
            "**60-minute**",
            "one combined matrix",
            "at least three unmentioned conditions",
            "at least one adjacent surface",
            "zero replay cells after row 1",
            "cap **8 cells**",
            "every child demo",
            "across children",
            "Contradictions",
            "Richards-LLC/cassy/issues/759",
            "NOT EXERCISED",
            "one task per defect",
            "exactly one epic note",
            "receipt is stale",
            "do not silently rerun",
        ] {
            assert!(
                walk.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(required),
                "{label}: {required}"
            );
        }
        let qa = get("skills/cas-qa-craft/SKILL.md");
        assert!(qa.contains(route), "{label}: QA routes to epic procedure");
        assert!(qa.contains("empty and no child has one"), "{label}");
        assert!(qa.contains("**60-minute** box overrides"), "{label}");
        let example = walk.split("### Example epic note").nth(1).unwrap();
        for field in [
            "Epic flow walk",
            "Tip:",
            "Children:",
            "Ledger:",
            "cells=4; PASS=3; FAIL=0; NOT EXERCISED=1",
            "labels:",
            "Owed:",
        ] {
            assert!(example.contains(field), "{label}: example lacks {field}");
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
fn agent_hygiene_instructions_match_available_actions_and_runtime_context() {
    for path in RULE_REVIEWER_PATHS {
        let body = load(path);
        assert!(
            body.contains("Retire (tombstone)"),
            "{path} must describe rule deletion as a tombstone retirement"
        );
        assert!(
            body.contains("rule action=delete"),
            "{path} must use the available rule delete action"
        );
        assert!(
            !body.contains("**Archive**"),
            "{path} must not describe an unavailable rule archive action"
        );
    }

    for path in DUPLICATE_DETECTOR_PATHS {
        let body = load(path);
        assert!(
            body.contains("task action=notes id=<task-id> note_type=question"),
            "{path} must name the task-note channel for uncertain cases"
        );
    }

    for (path, body) in every_agent_definition() {
        assert!(
            !body.contains("Current year: 2026"),
            "{path} must not hard-code a calendar year"
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
        run_git(
            repo,
            [
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                &format!("history-{index}"),
            ],
        );
    }
    fs::write(repo.join("contract_test.rs"), "#[test] fn contract() {}\n").unwrap();
    run_git(repo, ["add", "contract_test.rs"]);
    run_git(repo, ["commit", "--quiet", "-m", "add test"]);

    // This is the exact command shape documented by task-verifier.md after
    // the fix: HEAD~10 is valid because the fixture has eleven commits, and
    // POSIX grep's `-E` selects extended regular expressions.
    let output = Command::new("sh")
        .current_dir(repo)
        .arg("-c")
        .arg(
            "git diff --name-status HEAD~10 | grep -E '^A[[:space:]]+.*(_test\\.rs|tests/.*\\.rs)'",
        )
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
