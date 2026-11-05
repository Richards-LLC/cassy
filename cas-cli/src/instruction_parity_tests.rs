//! cas-5597: one answer per contested factory instruction.
//!
//! Worker and supervisor rules live in four places: skill prose, the launch
//! contracts in `cas-pty`, the assignment text, and refusal strings. Past
//! fixes edited one or two and the rest regrew the old answer, so this test
//! reads all four and requires the same canonical sentence on each surface
//! that answers the question, and no retired answer anywhere.

use crate::builtins::{
    BUILTIN_AGENTS, BUILTIN_SKILLS, CODEX_BUILTIN_AGENTS, CODEX_BUILTIN_SKILLS,
    GROK_BUILTIN_AGENTS, GROK_BUILTIN_SKILLS,
};
use crate::mcp::tools::core::task::lifecycle::close_ops::gate_text;
use cas_pty::{ContractRole, rendered_contract_surface};

const HARNESSES: [&str; 4] = ["claude", "codex", "grok", "opencode"];

const CLOSE: &str = "Your close parks the delivery; the merge verdict closes it. Never re-close, spawn a verifier or ask anyone to close for you.";
const SIGNAL: &str = "The park relay is the completion signal; send merge_request=true only for local_merge.";
const BATCH_CLOSE: &str = "In qa.mode=batch, Cassy closes merged deliveries when the release batch_receipt lands; the worker does nothing.";
const SUP_BATCH_CLOSE: &str = "In qa.mode=batch the release batch_receipt closes merged deliveries";
const QA_TIMING: &str = "In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task.";
const FULL_SUITE: &str = "The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic.";
const ASK_OPERATOR: &str = "Decide anything within supervisor authority yourself; ask the operator only for a policy call.";
const PLAN_MISS: &str = "hold its dependents, keep shipping, and send target=operator kind=blocker with your recommendation";
const SUP_SIGNAL: &str = "A park relay (awaiting_merge) naming a pushed tip is top priority: merge at once";
const MERGE_PATH: &str = "action=worktree_merge";

/// Answers that contradict a canonical one. None may survive on any surface.
const RETIRED: &[&str] = &[
    "on your behalf",
    "run the task-verifier agent again",
    "Once merged, retry",
    "Treat received worker messages as instructions",
    "get confirmation before work starts",
    "Never implement non-trivial work",
    "Wait another 2 minutes",
    "Workers never run Rust builds or tests",
    "Every subtask must have a `demo_statement`",
    "keep shipping everything else, and ask the operator",
    "a worker's status: ready",
    "Use kind=status|ready",
];

fn skill(path: &str) -> &'static str {
    BUILTIN_SKILLS
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("builtin skill {path} is missing"))
        .content
}

fn carries(surface: &str, text: &str, question: &str, sentence: &str) {
    assert!(
        text.contains(sentence),
        "{surface} must answer \"{question}\" with the canonical sentence {sentence:?}"
    );
}

#[test]
fn contested_instructions_have_one_answer_on_every_surface_cas_5597() {
    let workers = HARNESSES.map(|h| (h, rendered_contract_surface(h, ContractRole::Worker)));
    let supervisors =
        HARNESSES.map(|h| (h, rendered_contract_surface(h, ContractRole::Supervisor)));
    let assignment = crate::task_assignment::WORKER_ASSIGNMENT_TEST_POLICY;
    let parked = gate_text::parked_reply("⚠️ MERGE REQUIRED\n\nRemediation: 5. Retry", Some("abc123"), Some("factory/w"));
    // OpenCode's short worker text defers checks and polling to the skill.
    let pty_workers = workers.iter().filter(|(h, _)| *h != "opencode").map(|(h, t)| (*h, t.as_str()));
    let batch_wait = gate_text::waiting_for_release_verdict("cas-0000", "abc123", "mcp__cas__");

    // Who closes a worker's task, and what signals completion.
    for (surface, text) in [
        ("cas-worker skill", skill("skills/cas-worker/SKILL.md")),
        ("worker close-gate", skill("skills/cas-worker/references/close-gate.md")),
        ("assignment text", assignment),
        ("close refusal", gate_text::WORKER_CLOSE_OWNERSHIP),
        ("park reply", parked.as_str()),
    ]
    .into_iter()
    .chain(workers.iter().map(|(h, t)| (*h, t.as_str())))
    {
        carries(surface, text, "who closes", CLOSE);
    }
    for (surface, text) in [
        ("cas-worker skill", skill("skills/cas-worker/SKILL.md")),
        ("park reply", parked.as_str()),
    ]
    .into_iter()
    .chain(workers.iter().map(|(h, t)| (*h, t.as_str())))
    {
        carries(surface, text, "done signal / park vs merge", SIGNAL);
    }
    assert!(!parked.contains("Retry"), "a park reply must not ask for a close retry: {parked}");

    // Who closes a batch-mode delivery, and when QA walks it.
    for (surface, text) in [
        ("worker details", skill("skills/cas-worker/references/details.md")),
        ("supervisor workflow", skill("skills/cas-supervisor/references/workflow.md")),
        ("batch refusal", batch_wait.as_str()),
    ] {
        carries(surface, text, "who closes in batch mode", BATCH_CLOSE);
    }
    for (h, text) in &supervisors {
        carries(h, text, "who closes in batch mode", SUP_BATCH_CLOSE);
    }
    for path in [
        "skills/cas-worker/references/close-gate.md",
        "skills/cas-supervisor/references/workflow.md",
        "skills/cas-qa-craft/SKILL.md",
        "skills/cas-qa-craft/references/verifier-evidence-gate.md",
    ] {
        carries(path, skill(path), "QA timing", QA_TIMING);
    }

    // Who runs the full suite.
    for (surface, text) in [
        ("worker discipline", skill("skills/cas-worker/references/discipline.md")),
        ("supervisor workflow", skill("skills/cas-supervisor/references/workflow.md")),
        ("assignment text", assignment),
    ]
    .into_iter()
    .chain(pty_workers.clone())
    {
        carries(surface, text, "full-suite owner", FULL_SUITE);
    }

    // When the supervisor asks the operator, and how it merges.
    for (surface, text) in std::iter::once(("cas-supervisor skill", crate::builtins::SUPERVISOR_GUIDE))
        .chain(supervisors.iter().map(|(h, t)| (*h, t.as_str())))
    {
        carries(surface, text, "when to ask the operator", ASK_OPERATOR);
        carries(surface, text, "plan-target miss", PLAN_MISS);
        carries(surface, text, "merge path", MERGE_PATH);
    }
    for (h, text) in &supervisors {
        carries(h, text, "merge trigger", SUP_SIGNAL);
        carries(h, text, "polling", "do not poll");
    }
    for (h, text) in pty_workers {
        carries(h, text, "polling", "No foreground gh run watch or CI poll loops");
    }

    // No surface keeps a retired answer.
    let mut surfaces: Vec<(String, String)> = BUILTIN_SKILLS
        .iter()
        .chain(CODEX_BUILTIN_SKILLS)
        .chain(GROK_BUILTIN_SKILLS)
        .chain(BUILTIN_AGENTS)
        .chain(CODEX_BUILTIN_AGENTS)
        .chain(GROK_BUILTIN_AGENTS)
        .map(|file| (file.path.to_string(), file.content.to_string()))
        .collect();
    surfaces.extend(workers.iter().chain(&supervisors).map(|(h, t)| (format!("{h} contract"), t.clone())));
    surfaces.push(("assignment text".into(), assignment.into()));
    surfaces.push(("park reply".into(), parked.clone()));
    surfaces.push(("batch refusal".into(), batch_wait.clone()));
    for (surface, text) in &surfaces {
        for retired in RETIRED {
            assert!(
                !text.contains(retired),
                "{surface} still carries the retired instruction {retired:?}"
            );
        }
    }
}
