//! Contract tests for the built-in verifier agent and the Stop-hook
//! maintenance job bodies.
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
        "cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs" => {
            include_str!("../src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs")
        }
        "cas-cli/src/builtins.rs" => include_str!("../src/builtins.rs"),
        _ => builtin_catalog::find_source_path(relative),
    }
}

/// Codex ships no `.md` agents: it ignores them (audit D6, cas-6b97).
const VERIFIER_PATHS: [&str; 2] = [
    "cas-cli/src/builtins/agents/task-verifier.md",
    "cas-cli/src/builtins/grok/agents/task-verifier.md",
];

/// Stop-hook maintenance job bodies: one per job, not agent definitions
/// (cas-228e, audit D12).
fn job(name: &str) -> &'static str {
    cas::maintenance_jobs::job_body(name).unwrap_or_else(|| panic!("no {name} job body"))
}

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
        found.len() >= 2,
        "expected the Claude and Grok agent catalogs to be discovered, found {}",
        found.len()
    );
    for maintenance in cas::maintenance_jobs::MAINTENANCE_JOBS {
        found.push((format!("jobs/{}.md", maintenance.name), maintenance.body));
    }
    found
}

const EVIDENCE_GATE: &str = "skills/cas-qa-craft/references/verifier-evidence-gate.md";

#[test]
fn verifier_mirrors_document_the_current_close_contract() {
    for path in VERIFIER_PATHS {
        let body = load(path);
        assert!(
            body.contains("model: inherit"),
            "{path} must inherit the caller model"
        );
        // Audit M47 / L2 P2-60 (cas-228e): one verifier spawn costs this body.
        assert!(
            body.len() < 15_000,
            "{path} is {} bytes; keep it under 15 KB and move procedure to references",
            body.len()
        );
        // Audit L2 P1-59: the verifier reads and records; it never edits.
        let frontmatter = &body[..body[3..].find("---").unwrap() + 3];
        let tools = frontmatter
            .lines()
            .find_map(|line| line.strip_prefix("tools: "))
            .unwrap_or_else(|| panic!("{path} must restrict its tools"));
        for denied in ["Edit", "Write", "Agent", "NotebookEdit"] {
            assert!(
                !tools.split(", ").any(|tool| tool == denied),
                "{path} must not grant {denied}"
            );
        }
        for needed in ["Read", "Bash", "verification", "task"] {
            assert!(tools.contains(needed), "{path} tools lack {needed}: {tools}");
        }
        // Audit M10: `files` is the field; `files_reviewed` was dropped silently.
        assert!(
            body.contains("files=\"file1,file2\""),
            "{path} must record files in its verdict template"
        );
        assert!(!body.contains("files_reviewed"), "{path} uses the old field name");
        // Audit L2 P1-50: diff against the task's delivery base.
        assert!(
            body.contains("git diff --name-status \"$BASE\" HEAD | grep -E "),
            "{path} must use POSIX grep's extended regexp option in its test-first check"
        );
        assert!(body.contains("git merge-base HEAD <target-branch>"), "{path}");
        assert!(!body.contains("HEAD~10"), "{path} keeps a fixed commit-count base");
        assert!(
            !body.contains("| rg -e ") && !body.contains("| rg -E "),
            "{path} retains a ripgrep-only test-first command"
        );
        assert!(
            !body.contains("VERIFICATION JAIL"),
            "{path} retains stale jail wording"
        );
        // Audit L2 P1-49: the close-path section addressed the closer.
        assert!(!body.contains("Close-Path Error Detection"), "{path}");
        for marker in [
            "Verifier handoff rejected",
            "Verifier capability rejected",
            "Verification authority rejected",
            "ast-grep",
            "stranded_branch_override",
            "epic_verification_owner",
            "verifier-evidence-gate.md",
            "SUPERVISOR CALL",
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
/// The gate moved out of the verifier body into a cas-qa-craft reference
/// (cas-228e); the verifier routes to it before it reads the close reason.
#[test]
fn epic_child_demos_require_evidence_before_close_reason() {
    for path in VERIFIER_PATHS {
        let body = load(path);
        let route = body.find("verifier-evidence-gate.md").unwrap();
        let close_reason = body.find("### Step 1: Check the close reason").unwrap();
        assert!(route < close_reason, "{path}: evidence gate must come first");
        assert!(body[..route].contains("**any child**"), "{path}");
        assert!(body[..close_reason].contains("closed children"), "{path}");
    }
    for (flavor, label) in builtin_catalog::FLAVORS {
        let gate = builtin_catalog::find(*flavor, EVIDENCE_GATE);
        let prerequisites = gate.find("### Epic evidence prerequisites").unwrap();
        let table = gate.find("| Check | REJECT when |").unwrap();
        let close_reason = gate
            .find("Only after this evidence gate passes may you read the close reason")
            .unwrap();
        assert!(prerequisites < table && table < close_reason, "{label}");
        assert_eq!(gate.matches("| Check | REJECT when |").count(), 1, "{label}");
        let epic_gate = &gate[prerequisites..gate.find("### Step 0A:").unwrap()];
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
                "{label}: {required}"
            );
        }
        assert!(gate[..prerequisites].contains("**any child**"), "{label}");
        assert!(gate[..prerequisites].contains("closed children"), "{label}");
        for check in [
            "Required cells",
            "Source inference",
            "Failed-cell ownership",
            "Forbidden verdict",
            "Matrix breadth",
            "Headline counts",
            "PASS evidence",
        ] {
            assert!(gate[table..close_reason].contains(check), "{label}: {check}");
        }
        // Audit decision D9: NOT EXERCISED rows are the supervisor's call.
        let policy = &gate[gate.find("### NOT EXERCISED rows").unwrap()..];
        for required in ["Neither approve nor reject", "status=error", "SUPERVISOR CALL"] {
            assert!(policy.contains(required), "{label}: {required}");
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
        // Links resolve in the installed layout: the supervisor body sits
        // beside its own `references/`, the checklist is a sibling skill dir.
        assert!(
            supervisor.contains("](references/epic-flow-walk.md)"),
            "{label} supervisor route"
        );
        let route = "](../cas-supervisor/references/epic-flow-walk.md)";
        assert!(checklist.contains(route), "{label} checklist route");
        assert!(checklist.contains("release gate detached"), "{label}");
        assert!(checklist.contains("verification_type=epic"), "{label}");

        let walk = get("skills/cas-supervisor/references/epic-flow-walk.md");
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
        stop_handler.contains("Review exactly these unreviewed learning IDs: {unreviewed_ids}"),
        "Stop context must pass explicit IDs to learning-reviewer"
    );
    let stop_flow = load("cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs");
    assert!(stop_flow.contains("build_learning_review_context(store.as_ref(), &config)"));
    assert!(stop_flow.contains("jobs.push((\"learning-reviewer\", context));"));
    assert!(stop_flow.contains("{body}\\n\\nRun this maintenance job"));
    assert!(
        stop_flow.contains("{context}"),
        "queued prompt must carry explicit-ID context"
    );
    assert!(
        job("learning-reviewer").contains("learning ID from the queued prompt"),
        "learning-reviewer must consume IDs supplied by the queued prompt"
    );
}

/// Audit D12 / L2 P1-58 (cas-228e): each Stop job has one body, built into
/// the prompt with the light lane's own tool prefix. No harness installs the
/// jobs as subagents, and stop_flow never embeds a per-harness copy.
#[test]
fn stop_jobs_use_one_body_remapped_to_the_light_lane_prefix() {
    let stop_flow = load("cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs");
    assert!(stop_flow.contains("crate::light_lane::tool_prefix()"));
    assert!(stop_flow.contains("crate::maintenance_jobs::render_job_prompt_body("));
    assert!(
        !stop_flow.contains("builtins/codex/agents/"),
        "stop_flow must not embed the Codex agent copies"
    );
    for maintenance in cas::maintenance_jobs::MAINTENANCE_JOBS {
        let rel = format!("agents/{}.md", maintenance.name);
        for (flavor, label) in builtin_catalog::FLAVORS {
            assert!(
                !builtin_catalog::agents(*flavor)
                    .iter()
                    .any(|builtin| builtin.path == rel),
                "{label} catalog still installs {rel} as a subagent"
            );
        }
        let rendered =
            cas::maintenance_jobs::render_job_prompt_body(maintenance.body, "mcp__cs__");
        assert!(
            !rendered.contains("mcp__cas__"),
            "{} keeps a Claude tool name after the Codex remap",
            maintenance.name
        );
    }
}

/// Audit M16 / L2 P0-07, P0-08 (cas-228e).
#[test]
fn maintenance_jobs_call_tools_the_way_the_tools_accept() {
    let learning = job("learning-reviewer");
    let skill_create = learning
        .lines()
        .find(|line| line.contains("skill action=create"))
        .expect("learning-reviewer documents skill creation");
    for field in ["invocation=", "scope=project", "draft=true", "source_ids="] {
        assert!(skill_create.contains(field), "skill create lacks {field}: {skill_create}");
    }
    assert_eq!(
        learning.matches("skill action=list_all").count(),
        1,
        "learning-reviewer lists skills once, not once per learning"
    );

    let rules = job("rule-reviewer");
    assert!(rules.contains("rule action=promote id=<id> change_note="));
    assert!(
        !rules.contains("rule action=helpful"),
        "rule-reviewer must promote by decision, not by voting"
    );
    assert!(rules.contains("rule action=show id=<id>"));
    assert!(!rules.contains("30+ days"), "an uncheckable criterion was dropped");

    let summarizer = job("session-summarizer");
    assert!(!summarizer.contains("task action=mine"), "the job is not the session's caller");
    assert!(summarizer.contains("transcript path"));
    assert!(summarizer.contains("task action=list status=in_progress"));

    let detector = job("duplicate-detector");
    assert!(!detector.contains("action=recent"), "process exactly the supplied IDs");
    assert!(detector.contains("UNCERTAIN <keep-id> <dup-id>"));
}

#[test]
fn agent_hygiene_instructions_match_available_actions_and_runtime_context() {
    let rules = job("rule-reviewer");
    assert!(
        rules.contains("Retire (tombstone)"),
        "rule-reviewer must describe rule deletion as a tombstone retirement"
    );
    assert!(
        rules.contains("rule action=delete"),
        "rule-reviewer must use the available rule delete action"
    );
    assert!(
        !rules.contains("**Archive**"),
        "rule-reviewer must not describe an unavailable rule archive action"
    );

    let detector = job("duplicate-detector");
    assert!(
        !detector.contains("task action=notes"),
        "duplicate-detector has no task to note; it reports UNCERTAIN lines"
    );

    for (path, body) in every_agent_definition() {
        assert!(
            !body.contains("Current year: 2026"),
            "{path} must not hard-code a calendar year"
        );
    }
}

/// Audit M09 (cas-228e): the Stop-hook classifier is a single-turn,
/// tool-less call, so it has a dedicated prompt and is handed duplicate
/// candidates. It no longer embeds the human session-learn skill.
#[test]
fn session_learn_stop_hook_uses_a_dedicated_classifier_prompt() {
    let handler = load("cas-cli/src/hooks/handlers/handlers_session.rs");
    assert!(
        handler.contains("const SESSION_LEARN_CLASSIFIER_PROMPT: &str"),
        "Stop hook must carry its own classifier prompt"
    );
    assert!(
        !handler.contains("include_str!(\"../../builtins/skills/session-learn/SKILL.md\")"),
        "Stop hook must not send the human skill body to a tool-less call"
    );
    assert!(
        handler.contains("## Existing memories (duplicate candidates)"),
        "Stop hook must pass duplicate candidates in"
    );
    let stop_flow = load("cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs");
    assert!(stop_flow.contains("session_learn_dedup_candidates(store.as_ref())"));
}

#[test]
fn verifier_test_first_command_runs_on_a_fixture_repo() {
    let temp = tempfile::tempdir().expect("temporary git fixture");
    let repo = temp.path();
    run_git(repo, ["init", "--quiet", "--initial-branch=main"]);
    run_git(repo, ["config", "user.name", "Cassy Test"]);
    run_git(repo, ["config", "user.email", "cassy@example.invalid"]);

    // History on the target branch that is not part of the delivery.
    fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    fs::write(repo.join("old_test.rs"), "#[test] fn old() {}\n").unwrap();
    run_git(repo, ["add", "seed.txt", "old_test.rs"]);
    run_git(repo, ["commit", "--quiet", "-m", "seed"]);

    // The delivery: one branch commit adding a test.
    run_git(repo, ["checkout", "--quiet", "-b", "factory/worker"]);
    fs::write(repo.join("contract_test.rs"), "#[test] fn contract() {}\n").unwrap();
    run_git(repo, ["add", "contract_test.rs"]);
    run_git(repo, ["commit", "--quiet", "-m", "add test"]);

    // This is the exact command shape documented by task-verifier.md: the
    // delivery base is the merge-base with the task's target branch (never a
    // fixed commit count), and POSIX grep's `-E` selects extended regexps.
    let output = Command::new("sh")
        .current_dir(repo)
        .arg("-c")
        .arg(
            "BASE=$(git merge-base HEAD main) && git diff --name-status \"$BASE\" HEAD | grep -E '^A[[:space:]]+.*(_test\\.rs|tests/.*\\.rs)'",
        )
        .output()
        .expect("run documented verifier command");
    assert!(
        output.status.success(),
        "documented test-first command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("contract_test.rs"),
        "the delivered test file is found: {stdout}"
    );
    assert!(
        !stdout.contains("old_test.rs"),
        "target-branch history is not attributed to the delivery: {stdout}"
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
