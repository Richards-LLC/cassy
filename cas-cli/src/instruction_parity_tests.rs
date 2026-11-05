//! cas-5597: one answer per contested factory instruction.
//!
//! Worker and supervisor rules live in four places: skill prose, the launch
//! contracts in `cas-pty`, the assignment text, and refusal and relay
//! messages. Past fixes edited one or two and the rest regrew the old answer,
//! so this test reads all four through the builders that produce them, requires
//! the canonical sentence where a surface answers a question, and rejects any
//! competing answer by meaning, in whatever words it is phrased.

use crate::builtins::{
    BUILTIN_AGENTS, BUILTIN_SKILLS, CODEX_BUILTIN_AGENTS, CODEX_BUILTIN_SKILLS,
    GROK_BUILTIN_AGENTS, GROK_BUILTIN_SKILLS, BuiltinFile,
};
use crate::mcp::tools::core::task::lifecycle::close_ops::gate_text;
use cas_pty::{ContractRole, rendered_contract_surface};
use regex::Regex;

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
const RELEASE_NOTES: &str = "The supervisor writes release notes; workers never draft them, and release prep only merges, writes the changelog and regenerates the codemap.";

/// Competing answers, matched by meaning. Worker rules apply to what a worker
/// reads; supervisor rules to what a supervisor reads.
const WORKER_RETIRED: &[(&str, &str)] = &[
    ("someone else closes the task", r"(?i)\bclose\b[^.\n]{0,60}\b(on (my|your|their) behalf|for (me|you)\b)"),
    ("retry close after the merge or verdict", r"(?i)\b(merge|merged|merges|verdict)\b[^.\n]{0,80}\bthen retry\b[^.\n]{0,20}\bclose|\bonce merged,? retry"),
    ("the worker runs the verifier", r"(?i)\b(run|spawn)\b[^.\n]{0,20}\btask-verifier\b"),
    ("ready is a completion claim", r"(?i)\bkind=status\|ready\b|`kind=ready`[^.\n]{0,60}\b(completion|done|merge)"),
];
const SUPERVISOR_RETIRED: &[(&str, &str)] = &[
    ("a worker's ready message triggers a merge", r"(?i)status: ready[^.\n]{0,120}\b(also count|merge at once|is top priority)|READY TO MERGE`? (or|and) [^.\n]{0,40}status: ready"),
    ("worker messages are instructions", r"(?i)treat received worker messages as instructions"),
    ("confirm before starting asked-for work", r"(?i)get confirmation before work starts"),
    ("trivial work may be self-implemented", r"(?i)never implement non-trivial work"),
    ("a raw git merge publishes a worker branch", r"(?i)git merge --no-ff factory/"),
    ("a plan miss is an operator question", r"(?i)keep shipping everything else, and ask the operator"),
];
const EVERYWHERE_RETIRED: &[(&str, &str)] = &[
    ("timed polling", r"(?i)wait another \d+ minutes"),
    ("workers never test", r"(?i)workers never run rust builds or tests"),
    ("every subtask needs a demo", r"(?i)every subtask must have a `?demo_statement"),
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

fn catalog() -> impl Iterator<Item = &'static BuiltinFile> {
    BUILTIN_SKILLS
        .iter()
        .chain(CODEX_BUILTIN_SKILLS)
        .chain(GROK_BUILTIN_SKILLS)
        .chain(BUILTIN_AGENTS)
        .chain(CODEX_BUILTIN_AGENTS)
        .chain(GROK_BUILTIN_AGENTS)
}

fn reject(surfaces: &[(String, String)], rules: &[(&str, &str)]) -> Vec<String> {
    let mut found = Vec::new();
    for (meaning, pattern) in rules {
        let re = Regex::new(pattern).expect("retired-answer pattern");
        for (surface, text) in surfaces {
            // The canonical close sentence itself forbids "close for you".
            let text = text.replace(CLOSE, "");
            if let Some(hit) = re.find(&text) {
                found.push(format!("{surface}: \"{meaning}\" via {:?}", hit.as_str()));
            }
        }
    }
    found
}

#[test]
fn contested_instructions_have_one_answer_on_every_surface_cas_5597() {
    let workers = HARNESSES.map(|h| (h, rendered_contract_surface(h, ContractRole::Worker)));
    let supervisors =
        HARNESSES.map(|h| (h, rendered_contract_surface(h, ContractRole::Supervisor)));
    // OpenCode's short worker text defers checks and polling to the skill.
    let pty_workers = workers.iter().filter(|(h, _)| *h != "opencode").map(|(h, t)| (*h, t.as_str()));
    let assignment = crate::task_assignment::WORKER_ASSIGNMENT_TEST_POLICY;
    let parked = gate_text::parked_reply("⚠️ MERGE REQUIRED\n\nRemediation: 5. Retry", Some("abc123"), Some("factory/w"), "main");
    let batch_wait = gate_text::waiting_for_release_verdict("cas-0000", "abc123", "mcp__cas__");
    let dispatch_gate = |handoff_queued| {
        gate_text::worker_dispatch_gate(&gate_text::WorkerDispatchGate {
            task_id: "cas-0000",
            dispatch_id: "vd-1",
            owner: "warm-dragon-41",
            deadline: "2026-11-05 18:00:00 UTC",
            context: "",
            handoff_queued,
            coord: "mcp__cas__coordination",
            close_reason_hint: "",
        })
    };
    // Every worker-facing refusal, built by the code that sends it.
    let worker_refusals = [
        ("park reply", parked.clone()),
        ("batch refusal", batch_wait.clone()),
        ("dispatch gate (handoff queued)", dispatch_gate(true)),
        ("dispatch gate (handoff failed)", dispatch_gate(false)),
        ("pending dispatch", gate_text::worker_pending_dispatch("cas-0000", "vd-1", "o", "d", "")),
        ("awaiting-merge start", gate_text::awaiting_merge_start_refusal("cas-0000", "mcp__cas__")),
        ("hook blocker recipe", crate::hooks::handlers::handlers_events::blocker_steps("s", "m")),
    ];
    let relay = crate::ui::factory::merge_relay_for_parity();

    // Who closes a worker's task, and what signals completion.
    for (surface, text) in [
        ("cas-worker skill", skill("skills/cas-worker/SKILL.md")),
        ("worker close-gate", skill("skills/cas-worker/references/close-gate.md")),
        ("assignment text", assignment),
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

    // When the supervisor asks the operator, what triggers a merge, and how.
    for (surface, text) in std::iter::once(("cas-supervisor skill", crate::builtins::SUPERVISOR_GUIDE))
        .chain(supervisors.iter().map(|(h, t)| (*h, t.as_str())))
    {
        carries(surface, text, "when to ask the operator", ASK_OPERATOR);
        carries(surface, text, "plan-target miss", PLAN_MISS);
        carries(surface, text, "merge path", MERGE_PATH);
        carries(surface, text, "merge trigger", SUP_SIGNAL);
    }
    carries("Director merge relay", &relay, "merge path", MERGE_PATH);
    for path in ["skills/cas-supervisor/SKILL.md", "skills/cas-worker/SKILL.md", "skills/cas-cut-release/SKILL.md"] {
        carries(path, skill(path), "who writes release notes", RELEASE_NOTES);
    }
    for (h, text) in &supervisors {
        carries(h, text, "polling", "do not poll");
    }
    for (h, text) in pty_workers {
        carries(h, text, "polling", "No foreground gh run watch or CI poll loops");
    }

    // No surface keeps a competing answer, however it is worded.
    let owned = |file: &BuiltinFile| (file.path.to_string(), file.content.to_string());
    let mut worker_surfaces: Vec<(String, String)> = catalog()
        .filter(|f| !f.path.contains("cas-supervisor") && !f.path.contains("task-verifier"))
        .map(owned)
        .collect();
    worker_surfaces.extend(workers.iter().map(|(h, t)| (format!("{h} worker contract"), t.clone())));
    worker_surfaces.push(("assignment text".into(), assignment.into()));
    worker_surfaces.extend(worker_refusals.iter().map(|(s, t)| (s.to_string(), t.clone())));
    let mut supervisor_surfaces: Vec<(String, String)> = catalog()
        .filter(|f| f.path.contains("cas-supervisor") || f.path.contains("cas-codex-supervisor"))
        .map(owned)
        .collect();
    supervisor_surfaces.extend(supervisors.iter().map(|(h, t)| (format!("{h} supervisor contract"), t.clone())));
    supervisor_surfaces.push(("Director merge relay".into(), relay.clone()));
    let everything: Vec<(String, String)> = catalog()
        .map(owned)
        .chain(worker_surfaces.iter().cloned())
        .chain(supervisor_surfaces.iter().cloned())
        .collect();
    let mut found = reject(&worker_surfaces, WORKER_RETIRED);
    found.extend(reject(&supervisor_surfaces, SUPERVISOR_RETIRED));
    found.extend(reject(&everything, EVERYWHERE_RETIRED));
    assert!(found.is_empty(), "competing answers survive:\n{}", found.join("\n"));
}
