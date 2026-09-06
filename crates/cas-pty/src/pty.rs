//! PTY management using portable-pty
//!
//! Provides a wrapper around portable-pty with:
//! - Async read/write operations
//! - Raw byte output (terminal parsing done by ghostty_vt)
//! - Resize support

use crate::error::{Error, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

/// Instructions injected into Codex supervisor agents via `--config developer_instructions`.
const CODEX_SUPERVISOR_INSTRUCTIONS: &str = "You are the Cassy Factory Supervisor. Coordinate only: plan epics, assign tasks, monitor progress, review/merge. Never implement tasks. Use skills cas-supervisor and cas-codex-supervisor-checklist. Use MCP tools explicitly; no /cas-start, /cas-context, or /cas-end. Worker messages (status/blocker/ready) arrive asynchronously as new injected turns framed 'Message from <sender>: …'. Each is a triage trigger, not a fresh startup: read it, then assign/answer/redirect/merge as appropriate and reply via `mcp__cs__coordination action=message target=<worker>`. MERGE REQUIRED / awaiting_merge / idle-with-close-rejected is always top priority: run `mcp__cs__coordination action=epic_status` (and/or `mcp__cs__task action=list status=awaiting_merge`), merge `factory/<worker>` into the epic branch, then tell the worker to re-close — before free-form user chat. Recovery: to unblock a worker wedged by an urgent-stop/halt or a rejected close, assign it a fresh task or tell it to re-close after the merge — its legitimate `mcp__cs__task action=start` clears the urgent-stop halt. Write in facts, not narration: assignments, verdicts and merge state, not a recap of what a worker just told you and not commentary on your own process. Skip preamble and self-congratulation. In the pane, answer first, then bullets or a small table — the reader must absorb it at a glance, and a short dense paragraph fails that as badly as a long one. Do not recap the message you just received, restate the board every turn, or close with a summary of what you just said. Brevity never trims evidence: review findings, rejection reasons, measurements and merge receipts stay in full. Finishing one round does not mean you are done — remain available to coordinate the next message.";

/// Instructions injected into Codex worker agents via `--config developer_instructions`.
const CODEX_WORKER_INSTRUCTIONS: &str = "You are a Cassy Factory Worker. Always use CAS MCP tools for task lifecycle and coordination. On startup your Cassy session is already registered automatically — do NOT call session_start. Just run `mcp__cs__coordination action=whoami` then `mcp__cs__task action=mine`. Work exactly ONE task at a time: choose a single assigned task, run `mcp__cs__task action=show id=<task-id>` then `mcp__cs__task action=start id=<task-id>` before coding, implement it, commit and push your changes, then close it with `mcp__cs__task action=close id=<task-id> reason=\"...\"` (or hand it to the supervisor if close returns verification-required guidance) BEFORE starting any other task — the factory coordination policy, even though verification waits no longer block unrelated MCP work. Add progress notes frequently using `mcp__cs__task action=notes id=<task-id> note_type=progress notes=\"...\"`. For blockers, add a blocker note, set `status=blocked`, and message supervisor via `mcp__cs__coordination action=message target=supervisor message=\"...\"`. If close returns verification-required guidance, immediately ask the supervisor to verify/close on your behalf. If close returns MERGE REQUIRED, push your branch and ask the supervisor to merge `factory/<your-name>` into the epic branch, then re-close after the merge lands. Urgent-stop recovery: if an urgent redirect halts your work (WORK HALTED), do not fight it — a legitimate `mcp__cs__task action=start` on your newly-assigned task clears the urgent-stop halt and resumes you. After closing or handing off a task, stay available — you are not permanently done; the supervisor will send more work as new messages. Treat any injected turn framed 'Message from <sender>: …' as an instruction to act on, not noise, and still finish or hand off your current task before starting the next one a message assigns. NEVER foreground-block the pane: any command that can exceed ~2 minutes (builds, full test suites, deploys, servers, CI waits) must be run backgrounded (`cmd > /tmp/out.log 2>&1 &`, then read the log later), or replaced by `mcp__cs__coordination action=remind remind_delay_secs=<n>` plus ending your turn — a blocked turn cannot receive supervisor messages or stand-down orders. Foreground `gh run watch` and CI poll loops are banned; queue the run, set a reminder, end the turn, then check once with `gh run list`. Budget your context: report remaining headroom as a PERCENTAGE in every milestone progress note (a number — an adjective like ample or adequate is unactionable), prefer small pushed commits over large WIP, and when context runs low CHECKPOINT (commit + push + handoff note + ask the supervisor for a respawn) — never work into auto-compaction. Write in facts, not narration: say what is now true and what it cost, not what you are about to do, not a recap of the brief, and never narrate tool calls the reader can already see. Skip preamble and self-congratulation. Your pane output is a triage line, not a report: answer first, then one or two bullets at most. The durable record is the task note and the close reason — those are read at review and your pane prose mostly is not, so put detail there instead of saying it twice. Shape beats compression: bullets and small tables land at a glance where a short dense paragraph does not. Blocker escalations and merge requests are the exception and stay complete. Brevity never trims evidence: commit SHAs, file:line root causes, measurements, approaches you tried that failed, and anything you are still unsure of stay in full. Do not use /cas-start, /cas-context, or /cas-end. Stay within assigned task scope.";

/// Prefix for the Codex worker startup prompt. The worker name is appended at runtime.
const CODEX_WORKER_STARTUP_PREFIX: &str = "I'm initiating Cassy worker startup now: confirm identity, check assigned tasks, then start any assigned task with a progress note. My Cassy session is already registered automatically (do NOT call session_start).\n1) Run mcp__cs__coordination action=whoami";

/// Instructions injected into Grok Build supervisor agents via `--rules`
/// (EPIC cas-8888, cas-6569 Phase 2).
///
/// Grok's SessionStart hook fires but its stdout is ignored, so the
/// SessionStart-additionalContext bundle Claude relies on for context
/// injection never reaches a Grok agent (delta #2) — `--rules` (confirmed
/// via `grok --help`: "Extra rules to append to the system prompt")
/// substitutes for that at launch time. Unlike Claude/Codex, Grok
/// namespaces MCP tools as `cas__<tool>` (its own `search_tool`/`use_tool`
/// dispatch, NOT `mcp__cas__`/`mcp__cs__`), so every tool reference here
/// uses that prefix.
const GROK_SUPERVISOR_INSTRUCTIONS: &str = "You are the Cassy Factory Supervisor, running on Grok Build. Coordinate only: plan epics, assign tasks, monitor progress, review/merge. Never implement tasks. Use skills cas-supervisor and cas-supervisor-checklist. MCP tools are namespaced cas__<tool> (e.g. cas__task, cas__coordination) — not mcp__cas__ or mcp__cs__. Worker messages (status/blocker/ready) arrive asynchronously as new injected turns; each is a triage trigger, not a fresh startup — read it, then assign/answer/redirect/merge as appropriate and reply via `cas__coordination action=message target=<worker>`. MERGE REQUIRED / awaiting_merge / idle-with-close-rejected is always top priority: run `cas__coordination action=epic_status` (and/or `cas__task action=list status=awaiting_merge`), merge `factory/<worker>` into the epic branch, tell the worker to re-close — before free-form user chat. Act on the injected signal; do not poll. Recovery: to unblock a worker wedged by an urgent-stop/halt or a rejected close, assign it a fresh task or tell it to re-close after the merge — its legitimate `cas__task action=start` clears the urgent-stop halt. Write in facts, not narration: assignments, verdicts and merge state, not a recap of what a worker just told you and not commentary on your own process. Skip preamble and self-congratulation. In the pane, answer first, then bullets or a small table — the reader must absorb it at a glance, and a short dense paragraph fails that as badly as a long one. Do not recap the message you just received, restate the board every turn, or close with a summary of what you just said. Brevity never trims evidence: review findings, rejection reasons, measurements and merge receipts stay in full. Finishing one round does not mean you are done — remain available to coordinate the next message.";

/// Instructions injected into Grok Build worker agents via `--rules`
/// (EPIC cas-8888, cas-6569 Phase 2). See `GROK_SUPERVISOR_INSTRUCTIONS`
/// for the context-injection rationale.
const GROK_WORKER_INSTRUCTIONS: &str = "You are a Cassy Factory Worker, running on Grok Build. Always use CAS MCP tools for task lifecycle and coordination — they are namespaced cas__<tool> (e.g. cas__task, cas__coordination), not mcp__cas__ or mcp__cs__. On startup your Cassy session is already registered automatically. Run `cas__coordination action=whoami` then `cas__task action=mine`. Work exactly ONE task at a time: choose a single assigned task, run `cas__task action=show id=<task-id>` then `cas__task action=start id=<task-id>` before coding, implement it, commit and push your changes, then close it with `cas__task action=close id=<task-id> reason=\"...\"` (or hand it to the supervisor if close returns verification-required guidance) BEFORE starting any other task. Add progress notes frequently using `cas__task action=notes id=<task-id> note_type=progress notes=\"...\"`. For blockers, add a blocker note, set status=blocked, and message supervisor via `cas__coordination action=message target=supervisor message=\"...\"`. If close returns MERGE REQUIRED, push your branch and ask the supervisor to merge `factory/<your-name>` into the epic branch, then re-close after the merge lands. Urgent-stop recovery: if an urgent redirect halts your work (WORK HALTED), do not fight it — a legitimate `cas__task action=start` on your newly-assigned task clears the urgent-stop halt and resumes you. After closing or handing off a task, stay available — the supervisor will send you more work as new messages; treat any injected turn as an instruction to act on, not noise. NEVER foreground-block the pane: any command that can exceed ~2 minutes (builds, full test suites, deploys, servers, CI waits) must be run backgrounded (`cmd > /tmp/out.log 2>&1 &`, then read the log later), or replaced by `cas__coordination action=remind remind_delay_secs=<n>` plus ending your turn — a blocked turn cannot receive supervisor messages or stand-down orders. Foreground `gh run watch` and CI poll loops are banned; queue the run, set a reminder, end the turn, then check once with `gh run list`. Budget your context: report remaining headroom as a PERCENTAGE in every milestone progress note (a number — an adjective like ample or adequate is unactionable), prefer small pushed commits over large WIP, and when context runs low CHECKPOINT (commit + push + handoff note + ask the supervisor for a respawn) — never work into auto-compaction. Write in facts, not narration: say what is now true and what it cost, not what you are about to do, not a recap of the brief, and never narrate tool calls the reader can already see. Skip preamble and self-congratulation. Your pane output is a triage line, not a report: answer first, then one or two bullets at most. The durable record is the task note and the close reason — those are read at review and your pane prose mostly is not, so put detail there instead of saying it twice. Shape beats compression: bullets and small tables land at a glance where a short dense paragraph does not. Blocker escalations and merge requests are the exception and stay complete. Brevity never trims evidence: commit SHAs, file:line root causes, measurements, approaches you tried that failed, and anything you are still unsure of stay in full. Stay within assigned task scope.";

/// Minimal role projection for the OpenCode primary agents injected through
/// `OPENCODE_CONFIG_CONTENT`. Full plugin/lifecycle parity remains gated on
/// the later conformance work; these instructions establish only the task and
/// coordination contract needed by the launch adapter.
const OPENCODE_SUPERVISOR_INSTRUCTIONS: &str = "You are a Cassy Factory Supervisor running in OpenCode. Coordinate work; do not implement worker tasks. Use the injected CAS MCP tools with OpenCode's cas_ prefix, including cas_coordination and cas_task. Treat injected worker messages as instructions and keep merge/re-close work ahead of new assignment work.";
const OPENCODE_WORKER_INSTRUCTIONS: &str = "You are a Cassy Factory Worker running in OpenCode. Use the injected CAS MCP tools with OpenCode's cas_ prefix, including cas_coordination and cas_task. Work exactly one assigned task at a time: show and start it before editing, post progress notes, commit and push, then close or hand off before starting another task.";
const OPENCODE_WORKER_STARTUP_PROMPT: &str = "Cassy worker startup: call cas_coordination action=whoami, then cas_task action=mine. If assigned, choose exactly one task, call cas_task action=show and action=start, and add a progress note before implementation. If none is assigned, call cas_coordination action=message target=supervisor confirming readiness.";

// ===========================================================================
// Canonical CAS role-instruction contract (cas-0263).
//
// Every launched factory agent — Claude / Codex / Grok × supervisor / worker —
// must receive the SAME semantic role contract, differing only in the
// harness-correct MCP tool prefix (`mcp__cas__` for Claude, `mcp__cs__` for
// Codex, bare `cas__` for Grok) and launch syntax. The contract is carried on
// the surface each runtime actually consumes: Codex `--config
// developer_instructions`, Grok `--rules`, and Claude a launch-time queued intro
// prompt (Claude has no equivalent launch flag). The renderers are the four
// CODEX_*/GROK_* constants above plus the two `claude_*_contract` builders
// below; `*_CONTRACT_ELEMENTS` + `missing_contract_elements` are the single
// source of truth that a parity test enforces across all six rendered surfaces.
// ===========================================================================

/// Whether a launch shape carries the supervisor or worker half of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractRole {
    Supervisor,
    Worker,
}

/// One required element of the canonical role contract. A rendered surface
/// satisfies the element iff (mode = `Any`) it contains at least one marker, or
/// (mode = `All`) it contains every marker — checked after normalizing the
/// harness tool prefix so only intentional prefix differences are ignored.
pub struct ContractElement {
    pub id: &'static str,
    pub markers: &'static [&'static str],
    pub all: bool,
}

/// The canonical supervisor contract: coordinate-only, async triage, delegate
/// lifecycle, merge/re-close priority, urgent-stop recovery.
pub const SUPERVISOR_CONTRACT_ELEMENTS: &[ContractElement] = &[
    ContractElement {
        id: "coordinate-only",
        markers: &["coordinate only"],
        all: false,
    },
    ContractElement {
        id: "async-message-handling",
        markers: &["asynchron", "injected turn", "triage trigger"],
        all: false,
    },
    ContractElement {
        id: "assigns-work",
        markers: &["assign"],
        all: false,
    },
    ContractElement {
        id: "merge-reclose",
        markers: &["merge required", "re-close"],
        all: false,
    },
    ContractElement {
        id: "urgent-stop-recovery",
        markers: &["urgent-stop halt"],
        all: false,
    },
    // cas-0e4a: see the matching worker element for why both markers are
    // required together rather than a bare brevity instruction.
    ContractElement {
        id: "concise-reporting",
        markers: &["facts, not narration", "never trims evidence"],
        all: true,
    },
    // cas-a199: concise-reporting governs notes and inter-agent messages; this
    // governs the pane prose the operator actually reads. Paired with the same
    // evidence marker for the same reason.
    ContractElement {
        id: "user-response-shape",
        markers: &["at a glance", "never trims evidence"],
        all: true,
    },
];

/// The canonical worker contract: one-task ownership, async availability, full
/// task lifecycle, merge/re-close, urgent-stop recovery.
pub const WORKER_CONTRACT_ELEMENTS: &[ContractElement] = &[
    ContractElement {
        id: "one-task-ownership",
        markers: &["one task at a time", "exactly one task"],
        all: false,
    },
    ContractElement {
        id: "async-message-handling",
        markers: &["injected turn", "new messages", "stay available"],
        all: false,
    },
    ContractElement {
        id: "task-lifecycle",
        markers: &["action=start", "action=close", "action=notes"],
        all: true,
    },
    ContractElement {
        id: "merge-reclose",
        markers: &["merge required", "re-close"],
        all: false,
    },
    ContractElement {
        id: "urgent-stop-recovery",
        markers: &["urgent-stop halt"],
        all: false,
    },
    // cas-b4921 / GH #121: a worker that foreground-blocks on a long command is
    // unreachable — messages, stand-downs and urgent stops are only delivered
    // between turns. Every worker launch surface must carry the backgrounding
    // mandate, including the explicit ban on foreground `gh run watch`.
    ContractElement {
        id: "no-foreground-blocking",
        markers: &["backgrounded", "gh run watch"],
        all: true,
    },
    // cas-b4921 / GH #121 part B: two workers were killed mid-auto-compaction
    // with unpushed work. Every worker launch surface must carry the checkpoint
    // mandate — commit + push + handoff + respawn beats compacting.
    ContractElement {
        id: "context-checkpoint",
        markers: &["checkpoint", "compaction"],
        all: true,
    },
    // cas-0e4a: operator directive — agents burn tokens on narrated thinking.
    // Both markers are required together, deliberately: the measured risk of a
    // brevity rule is that it strips the evidence the factory runs on (commit
    // SHAs, file:line causes, measurements, rejected approaches, uncertainty),
    // which is exactly the context that degrades first at an agent handoff. A
    // surface cannot carry the "cut narration" half without the "keep evidence"
    // half.
    ContractElement {
        id: "concise-reporting",
        markers: &["facts, not narration", "never trims evidence"],
        all: true,
    },
    // cas-a199, operator follow-up: "90% of what they output is never read."
    // A worker's durable record is its task note and close reason, so pane
    // prose that restates them is written for nobody. Same paired-marker rule.
    ContractElement {
        id: "user-response-shape",
        markers: &["at a glance", "never trims evidence"],
        all: true,
    },
];

/// Canonical elements for a role.
pub fn contract_elements(role: ContractRole) -> &'static [ContractElement] {
    match role {
        ContractRole::Supervisor => SUPERVISOR_CONTRACT_ELEMENTS,
        ContractRole::Worker => WORKER_CONTRACT_ELEMENTS,
    }
}

/// Normalize the three harness tool prefixes to one token so parity checks only
/// ignore the intentional prefix difference, never a missing capability.
fn normalize_tool_prefix(text: &str) -> String {
    text.replace("mcp__cas__", "TOOL__")
        .replace("mcp__cs__", "TOOL__")
        .replace("cas__", "TOOL__")
        .to_lowercase()
}

/// Return the ids of any canonical contract elements MISSING from `text` for
/// `role`. Empty ⇒ the surface carries the full contract.
pub fn missing_contract_elements(text: &str, role: ContractRole) -> Vec<&'static str> {
    let norm = normalize_tool_prefix(text);
    contract_elements(role)
        .iter()
        .filter(|el| {
            let present: Vec<bool> = el
                .markers
                .iter()
                .map(|m| norm.contains(&normalize_tool_prefix(m)))
                .collect();
            if el.all {
                !present.iter().all(|p| *p)
            } else {
                !present.iter().any(|p| *p)
            }
        })
        .map(|el| el.id)
        .collect()
}

/// Render the Claude **supervisor** role contract (cas-0263). Claude has no
/// `--rules`/`developer_instructions` launch flag, so this is delivered as the
/// launch-time intro prompt CAS queues for the supervisor (see
/// `queue_supervisor_intro_prompt`). Uses Claude's `mcp__cas__` tool prefix.
pub fn claude_supervisor_contract(worker_list: &str) -> String {
    format!(
        "You are the Cassy Factory Supervisor. Coordinate only: plan epics, assign tasks, \
monitor progress, review/merge. Never implement tasks. Use skills cas-supervisor, \
cas-supervisor-checklist, and cas-codebase-design; MCP tools are namespaced mcp__cas__<tool> (e.g. mcp__cas__task, \
mcp__cas__coordination). Worker messages (status/blocker/ready) arrive asynchronously as new \
injected turns; each is a triage trigger, not a fresh startup — read it, then \
assign/answer/redirect/merge and reply via `mcp__cas__coordination action=message \
target=<worker>`. MERGE REQUIRED / awaiting_merge / idle-with-close-rejected is always top \
priority: run `mcp__cas__coordination action=epic_status` (and/or `mcp__cas__task action=list \
status=awaiting_merge`), merge `factory/<worker>` into the epic branch, then tell the worker to \
re-close — before free-form user chat. Recovery: to unblock a worker wedged by an \
urgent-stop/halt or a rejected close, assign it a fresh task or tell it to re-close after the \
merge — its legitimate `mcp__cas__task action=start` clears the urgent-stop halt. Act on the \
injected signal; do not poll. Write in facts, not narration: assignments, verdicts and merge \
state, not a recap of what a worker just told you and not commentary on your own process. Skip \
preamble and self-congratulation. In the pane, answer first, then bullets or a small table — the reader must absorb it \
at a glance, and a short dense paragraph fails that as badly as a long one. Do not recap the message \
you just received, restate the board every turn, or close with a summary of what you just said. \
Brevity never trims evidence: review findings, rejection \
reasons, measurements and merge receipts stay in full. Finishing one round does not mean you \
are done — remain available to coordinate the next message. Canonical current workers for this session: {worker_list}. First \
steps: mcp__cas__coordination action=whoami; mcp__cas__task action=list task_type=epic; \
mcp__cas__task action=ready."
    )
}

/// Render the Claude **worker** role contract (cas-0263), delivered as the
/// launch-time intro prompt CAS queues for the worker (see
/// `queue_codex_worker_intro_prompt`). Uses Claude's `mcp__cas__` tool prefix.
pub fn claude_worker_contract(worker_name: &str) -> String {
    format!(
        "You are a Cassy Factory Worker ({worker_name}). Always use CAS MCP tools for task \
lifecycle and coordination — namespaced mcp__cas__<tool> (e.g. mcp__cas__task, \
mcp__cas__coordination). On startup your Cassy session is already registered automatically — do \
NOT call session_start. Run `mcp__cas__coordination action=whoami` then `mcp__cas__task \
action=mine`. Work exactly ONE task at a time: run `mcp__cas__task action=show id=<task-id>` \
then `mcp__cas__task action=start id=<task-id>` before coding, implement it, commit and push, \
then close it with `mcp__cas__task action=close id=<task-id> reason=\"...\"` (or hand to the \
supervisor if close returns verification-required guidance) BEFORE starting any other task — this \
is the factory coordination policy even though verification waits do not block unrelated work. Add progress notes \
frequently via `mcp__cas__task action=notes id=<task-id> note_type=progress notes=\"...\"`. For \
blockers, add a blocker note, set status=blocked, and message supervisor via \
`mcp__cas__coordination action=message target=supervisor message=\"...\"`. If close returns MERGE \
REQUIRED, push your branch and ask the supervisor to merge `factory/<your-name>` into the epic \
branch, then re-close after the merge lands. Urgent-stop recovery: if an urgent redirect halts \
your work, a legitimate `mcp__cas__task action=start` on your newly-assigned task clears the \
urgent-stop halt and resumes you. After closing \
or handing off a task, stay available — the supervisor will send you more work as new messages; \
treat any injected turn as an instruction to act on, not noise. NEVER foreground-block the \
pane: any command that can exceed ~2 minutes (builds, full test suites, deploys, servers, CI \
waits) must be run backgrounded, or replaced by `mcp__cas__coordination action=remind \
remind_delay_secs=<n>` plus ending your turn — a blocked turn cannot receive supervisor \
messages or stand-down orders. Foreground `gh run watch` and CI poll loops are banned; queue \
the run, set a reminder, end the turn, then check once with `gh run list`. Budget your context: \
report remaining headroom as a PERCENTAGE in every milestone progress note (a number — an \
adjective like ample or adequate is unactionable), prefer small pushed commits over \
large WIP, and when context runs low CHECKPOINT (commit + push + handoff note + ask the \
supervisor for a respawn) — never work into auto-compaction. Write in facts, not narration: say \
what is now true and what it cost, not what you are about to do, not a recap of the brief, and \
never narrate tool calls the reader can already see. Skip preamble and self-congratulation. \
Your pane output is a triage line, not a report: answer first, then one or two bullets at most. \
The durable record is the task note and the close reason — those are read at review and your \
pane prose mostly is not, so put detail there instead of saying it twice. Shape beats \
compression: bullets and small tables land at a glance where a short dense paragraph does not. \
Blocker escalations and merge requests are the exception and stay complete. \
Brevity never trims evidence: commit SHAs, file:line root causes, measurements, approaches you \
tried that failed, and anything you are still unsure of stay in full. See the cas-worker skill \
for detailed workflow guidance. Stay within assigned task scope."
    )
}

/// The rendered contract text for a launch shape — the single accessor a parity
/// test uses to check all six surfaces uniformly (cas-0263). Keyed on the
/// harness's `--help`-stable name to avoid a dependency cycle (cas-pty is a leaf
/// crate that cannot import cas-mux's `SupervisorCli`).
pub fn rendered_contract_surface(harness: &str, role: ContractRole) -> String {
    match (harness, role) {
        ("codex", ContractRole::Supervisor) => CODEX_SUPERVISOR_INSTRUCTIONS.to_string(),
        ("codex", ContractRole::Worker) => CODEX_WORKER_INSTRUCTIONS.to_string(),
        ("grok", ContractRole::Supervisor) => GROK_SUPERVISOR_INSTRUCTIONS.to_string(),
        ("grok", ContractRole::Worker) => GROK_WORKER_INSTRUCTIONS.to_string(),
        ("claude", ContractRole::Supervisor) => claude_supervisor_contract("worker-a, worker-b"),
        ("claude", ContractRole::Worker) => claude_worker_contract("worker-a"),
        other => panic!("unknown launch shape: {other:?}"),
    }
}

/// Configuration for spawning a PTY
#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// Command to run (e.g., "claude")
    pub command: String,
    /// Arguments for the command
    pub args: Vec<String>,
    /// Working directory
    pub cwd: Option<PathBuf>,
    /// Environment variables to set
    pub env: Vec<(String, String)>,
    /// Environment variables to remove from the inherited child environment.
    pub env_remove: Vec<String>,
    /// Initial terminal size
    pub rows: u16,
    pub cols: u16,
}

fn push_factory_worker_metadata_env(
    env: &mut Vec<(String, String)>,
    role: &str,
    factory_worker_cli: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) {
    if let Some(worker_cli) = factory_worker_cli {
        env.push(("CAS_FACTORY_WORKER_CLI".to_string(), worker_cli.to_string()));
    }
    if role == "worker" {
        if let Some(model) = model {
            env.push(("CAS_FACTORY_WORKER_MODEL".to_string(), model.to_string()));
        }
        if let Some(effort) = effort {
            env.push(("CAS_FACTORY_WORKER_EFFORT".to_string(), effort.to_string()));
        }
    }
}

// Hosted OpenCode providers are declared inline so the provider/model
// selector cannot silently resolve through a different billing endpoint. The
// API key remains an environment substitution; this module never reads or
// persists its value.
const OPENCODE_TOKEN_PLAN_ENDPOINT: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";
const OPENCODE_TOKEN_PLAN_KEY_ENV: &str = "QWENCLOUD_TOKEN_PLAN_API_KEY";
const OPENCODE_PAYG_ENDPOINT: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1";
const OPENCODE_PAYG_CN_ENDPOINT: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const OPENCODE_PAYG_KEY_ENV: &str = "DASHSCOPE_API_KEY";

fn opencode_hosted_provider_config(model: &str) -> Option<serde_json::Value> {
    let (provider, model_id) = model.split_once('/')?;
    if model_id.trim() != "qwen3.8-max" {
        return None;
    }
    let provider = provider.trim();
    let (endpoint, key_env, display_name, token_plan) = match provider {
        "qwencloud" | "hosted-token-plan" => (
            OPENCODE_TOKEN_PLAN_ENDPOINT,
            OPENCODE_TOKEN_PLAN_KEY_ENV,
            "QwenCloud Token Plan",
            true,
        ),
        "alibaba" | "hosted-payg" => (
            OPENCODE_PAYG_ENDPOINT,
            OPENCODE_PAYG_KEY_ENV,
            "QwenCloud Pay-as-you-go",
            false,
        ),
        "alibaba-cn" => (
            OPENCODE_PAYG_CN_ENDPOINT,
            OPENCODE_PAYG_KEY_ENV,
            "QwenCloud Pay-as-you-go (China)",
            false,
        ),
        _ => return None,
    };
    let mut model_config = serde_json::json!({
        "name": format!("{display_name} qwen3.8-max"),
    });
    if token_plan {
        model_config["options"] = serde_json::json!({
            "extra_body": {"enable_thinking": true}
        });
    }
    Some(serde_json::json!({
        "provider": {
            provider: {
                "npm": "@ai-sdk/openai-compatible",
                "name": display_name,
                "options": {
                    "baseURL": endpoint,
                    "apiKey": format!("{{env:{key_env}}}"),
                },
                "models": {model_id: model_config},
            }
        }
    }))
}

/// Add explicit Claude config and credential-store directories to a worker.
///
/// This deliberately does nothing for omitted values so ordinary process
/// inheritance remains untouched for existing spawns.
fn expand_claude_selector(config_dir: &str) -> String {
    let config_dir = config_dir.trim();
    let config_dir = config_dir
        .strip_suffix('/')
        .filter(|value| !value.is_empty())
        .unwrap_or(config_dir);
    config_dir.strip_prefix('~').map_or_else(
        || config_dir.to_string(),
        |suffix| {
            dirs::home_dir()
                .map(|home| format!("{}{}", home.display(), suffix))
                .unwrap_or_else(|| config_dir.to_string())
        },
    )
}

/// Add Claude account selectors to a worker env.
///
/// The outer `Option` on `secure_storage_dir` distinguishes the legacy
/// config-derived behavior (`None`) from an independently captured requester
/// selector (`Some(...)`). The inner `Option` then preserves unset versus an
/// explicitly empty value.
#[cfg(test)]
fn push_claude_config_dir_env(
    env: &mut Vec<(String, String)>,
    role: &str,
    config_dir: Option<&str>,
) -> bool {
    push_claude_account_env(env, role, config_dir, None)
}

fn push_claude_account_env(
    env: &mut Vec<(String, String)>,
    role: &str,
    config_dir: Option<&str>,
    secure_storage_dir: Option<Option<&str>>,
) -> bool {
    if role != "worker" {
        return false;
    }

    let expanded_config_dir = config_dir.map(expand_claude_selector);
    if let Some(expanded) = expanded_config_dir.as_deref() {
        env.push(("CLAUDE_CONFIG_DIR".to_string(), expanded.to_string()));
    }

    let derived_main = expanded_config_dir.as_deref().is_some_and(|expanded| {
        dirs::home_dir()
            .map(|home| home.join(".claude").to_string_lossy() == expanded)
            .unwrap_or(false)
    });
    match secure_storage_dir {
        Some(Some(value)) => env.push((
            "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string(),
            expand_claude_selector(value),
        )),
        Some(None) => {}
        None if !derived_main => {
            if let Some(expanded) = expanded_config_dir.as_deref() {
                env.push((
                    "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string(),
                    expanded.to_string(),
                ));
            }
        }
        None => {}
    }

    // `true` tells the caller to remove the inherited selector. Main must use
    // the legacy unscoped credential item, not a defined-but-empty override.
    secure_storage_dir.is_some_and(|value| value.is_none())
        || (secure_storage_dir.is_none() && derived_main)
}

#[cfg(test)]
mod codex_home_contract_tests {
    use super::*;

    fn worker_config() -> PtyConfig {
        PtyConfig {
            env: vec![("CAS_AGENT_ROLE".to_string(), "worker".to_string())],
            ..Default::default()
        }
    }

    #[test]
    fn explicit_account_home_is_pushed_with_its_source_marker() {
        let mut config = worker_config();
        config.apply_codex_home(Some("/home/op/.codex-alt"), Some("explicit"));

        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CODEX_HOME" && v == "/home/op/.codex-alt")
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_FACTORY_CODEX_HOME_SOURCE" && v == "explicit")
        );
    }

    #[test]
    fn tilde_paths_are_expanded_like_the_claude_side() {
        let mut config = worker_config();
        config.apply_codex_home(Some("~/.codex-work"), Some("supervisor"));

        let value = config
            .env
            .iter()
            .find_map(|(k, v)| (k == "CODEX_HOME").then_some(v.as_str()))
            .expect("CODEX_HOME was not pushed");
        assert!(!value.starts_with('~'), "tilde was not expanded: {value}");
        assert!(value.ends_with("/.codex-work"));
    }

    #[test]
    fn omitting_the_account_home_leaves_inheritance_untouched() {
        let mut config = worker_config();
        config.apply_codex_home(None, Some("supervisor"));

        assert!(!config.env.iter().any(|(k, _)| k == "CODEX_HOME"));
        assert!(
            !config
                .env
                .iter()
                .any(|(k, _)| k == "CAS_FACTORY_CODEX_HOME_SOURCE")
        );
    }

    #[test]
    fn supervisor_panes_never_receive_a_worker_account_home() {
        let mut env = vec![("CAS_AGENT_ROLE".to_string(), "supervisor".to_string())];
        push_codex_home_env(&mut env, "supervisor", Some("/home/op/.codex-alt"));

        assert!(!env.iter().any(|(k, _)| k == "CODEX_HOME"));
    }
}

#[cfg(test)]
mod claude_config_dir_contract_tests {
    use super::*;

    fn env_value(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter()
            .rev()
            .find_map(|(candidate, value)| (candidate == key).then(|| value.clone()))
    }

    #[test]
    fn explicit_claude_config_dir_expands_tilde_into_worker_env() {
        let mut env = Vec::new();
        push_claude_config_dir_env(&mut env, "worker", Some("~/.claude-alt"));

        let home = dirs::home_dir().expect("test host has a home directory");
        assert_eq!(
            env_value(&env, "CLAUDE_CONFIG_DIR").as_deref(),
            Some(home.join(".claude-alt").to_string_lossy().as_ref())
        );
        assert_eq!(
            env_value(&env, "CLAUDE_SECURESTORAGE_CONFIG_DIR").as_deref(),
            Some(home.join(".claude-alt").to_string_lossy().as_ref())
        );
    }

    #[test]
    fn default_claude_config_dir_keeps_legacy_main_credential_store() {
        let mut env = Vec::new();
        let removes_secure_storage =
            push_claude_account_env(&mut env, "worker", Some("~/.claude"), None);

        assert!(removes_secure_storage);
        assert_eq!(env_value(&env, "CLAUDE_SECURESTORAGE_CONFIG_DIR"), None);
    }

    #[test]
    fn requester_secure_storage_preserves_unset_empty_and_set_values() {
        let cases = [
            (Some(None), None),
            (Some(Some("")), Some("")),
            (Some(Some("~/.claude-keychain")), Some("~/.claude-keychain")),
        ];
        let home = dirs::home_dir().expect("test host has a home directory");
        for (selector, expected) in cases {
            let mut config = PtyConfig::default();
            config.apply_claude_account(Some("~/.claude-work"), selector, Some("supervisor"));
            assert_eq!(
                env_value(&config.env, "CLAUDE_SECURESTORAGE_CONFIG_DIR").as_deref(),
                expected
                    .map(|value| {
                        if value.is_empty() {
                            value.to_string()
                        } else {
                            home.join(value.trim_start_matches("~/"))
                                .to_string_lossy()
                                .into_owned()
                        }
                    })
                    .as_deref()
            );
            assert_eq!(
                config
                    .env_remove
                    .iter()
                    .any(|key| key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"),
                expected.is_none()
            );
        }
    }

    #[test]
    fn omitted_claude_config_dir_adds_no_env_entry() {
        let mut env = Vec::new();
        push_claude_config_dir_env(&mut env, "worker", None);

        assert_eq!(env_value(&env, "CLAUDE_CONFIG_DIR"), None);
        assert_eq!(env_value(&env, "CLAUDE_SECURESTORAGE_CONFIG_DIR"), None);
    }

    #[test]
    fn non_worker_claude_config_dir_adds_no_env_entry() {
        let mut env = Vec::new();
        push_claude_config_dir_env(&mut env, "supervisor", Some("~/.claude-alt"));

        assert_eq!(env_value(&env, "CLAUDE_CONFIG_DIR"), None);
        assert_eq!(env_value(&env, "CLAUDE_SECURESTORAGE_CONFIG_DIR"), None);
    }
}

/// Configuration for spawning an agent with native Claude Code Agent Teams flags.
#[derive(Debug, Clone)]
pub struct TeamsSpawnConfig {
    /// Team name (factory session name)
    pub team_name: String,
    /// Agent ID (e.g., "worker-1@session-name")
    pub agent_id: String,
    /// Agent display name
    pub agent_name: String,
    /// Agent color for UI
    pub agent_color: String,
    /// Agent type (e.g., "team-lead", "general-purpose")
    pub agent_type: String,
    /// Parent session ID for analytics correlation (workers only)
    pub parent_session_id: Option<String>,
    /// Lead session ID — set for the team lead so --session-id matches leadSessionId
    pub lead_session_id: Option<String>,
    /// Optional path to a settings JSON file passed via `--settings <path>`.
    ///
    /// Populated for both the supervisor (`supervisor-settings.json`) and for
    /// every worker (`{worker-name}-settings.json`) so filesystem tool calls
    /// auto-approve from the per-role allowlist instead of escalating through
    /// the team-approval channel. Workers without this file hang on the
    /// phantom `team-lead` mailbox because Claude Code's harness misreads
    /// `agentType="team-lead"` as the lead's display name (upstream bug);
    /// shipping the allowlist eliminates the trigger even while that misread
    /// remains unfixed. See `cas-cli/src/ui/factory/daemon/runtime/teams.rs`
    /// (`supervisor_settings_contents` / `worker_settings_contents`) for the
    /// shape of each file.
    pub settings_path: Option<String>,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            command: "bash".to_string(),
            args: vec![],
            cwd: None,
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        }
    }
}

/// Add an explicit Codex account home to a worker.
///
/// The Codex analog of `push_claude_config_dir_env`: `CODEX_HOME` scopes a
/// codex account's config, credentials and session state (verified against
/// codex-cli 0.147.0 in cas-9cc3). Like the Claude version this deliberately
/// does nothing for omitted values, so ordinary inheritance is untouched.
fn push_codex_home_env(env: &mut Vec<(String, String)>, role: &str, config_dir: Option<&str>) {
    if role != "worker" {
        return;
    }
    let Some(config_dir) = config_dir else {
        return;
    };

    let expanded = config_dir.strip_prefix('~').map_or_else(
        || config_dir.to_string(),
        |suffix| {
            dirs::home_dir()
                .map(|home| format!("{}{}", home.display(), suffix))
                .unwrap_or_else(|| config_dir.to_string())
        },
    );
    env.push(("CODEX_HOME".to_string(), expanded));
}

impl PtyConfig {
    /// Apply the Codex account home override to this worker config.
    ///
    /// `Pty::spawn` detects the resulting marker and removes inherited API keys
    /// from the child command, so the selected ChatGPT account actually wins.
    pub fn apply_codex_home(&mut self, config_dir: Option<&str>, source: Option<&str>) {
        push_codex_home_env(&mut self.env, "worker", config_dir);
        if config_dir.is_some() {
            if let Some(source) = source {
                self.env.push((
                    "CAS_FACTORY_CODEX_HOME_SOURCE".to_string(),
                    source.to_string(),
                ));
            }
        }
    }

    /// Apply the Claude-only account directory override to this worker config.
    ///
    /// `Pty::spawn` detects the resulting source marker and removes inherited
    /// API-key and OAuth-token overrides from the child command, allowing the
    /// selected Claude subscription account to take effect. An explicit main
    /// selector also removes an inherited secure-storage override so Claude
    /// sees the legacy unset form.
    pub fn apply_claude_config_dir(&mut self, config_dir: Option<&str>, source: Option<&str>) {
        self.apply_claude_account(config_dir, None, source);
    }

    /// Apply a Claude config directory plus an independently captured secure
    /// storage selector to this worker config.
    ///
    /// `secure_storage_dir == None` derives the selector from `config_dir` for
    /// an explicit worker override. `Some(None)` removes the inherited
    /// selector, while `Some(Some(value))` preserves a requester value,
    /// including `Some("")`.
    pub fn apply_claude_account(
        &mut self,
        config_dir: Option<&str>,
        secure_storage_dir: Option<Option<&str>>,
        source: Option<&str>,
    ) {
        let remove_secure_storage =
            push_claude_account_env(&mut self.env, "worker", config_dir, secure_storage_dir);
        if remove_secure_storage
            && !self
                .env_remove
                .iter()
                .any(|key| key == "CLAUDE_SECURESTORAGE_CONFIG_DIR")
        {
            self.env_remove
                .push("CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string());
        }
        if config_dir.is_some() || secure_storage_dir.is_some() {
            if let Some(source) = source {
                self.env.push((
                    "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE".to_string(),
                    source.to_string(),
                ));
            }
        }
    }

    /// Re-derate this worker's `CARGO_BUILD_JOBS` against the number of
    /// workers actually competing for the host (cas-4614, GH #107).
    ///
    /// The constructors set a conservative default that assumes
    /// `DEFAULT_WORKER_CONCURRENCY_ASSUMPTION` workers, because they have no
    /// view of the fleet. The mux does, so it calls this immediately after
    /// building the config. Replaces rather than appends: two
    /// `CARGO_BUILD_JOBS` entries would leave the effective value depending on
    /// `CommandBuilder`'s iteration order, which is not a contract worth
    /// relying on.
    ///
    /// No-op for non-worker roles (nothing to replace — supervisors never get
    /// the variable) and when the fleet is at or below the assumed floor.
    pub fn apply_worker_build_concurrency(&mut self, active_workers: Option<usize>) {
        if !self.env.iter().any(|(k, _)| k == "CARGO_BUILD_JOBS") {
            return;
        }
        let Some(jobs) = cargo_build_jobs_for_worker(active_workers) else {
            return;
        };
        self.env.retain(|(k, _)| k != "CARGO_BUILD_JOBS");
        self.env.push(("CARGO_BUILD_JOBS".to_string(), jobs));
    }

    /// Create config for a Claude CLI instance
    ///
    /// # Arguments
    /// * `name` - Agent name
    /// * `role` - Agent role (e.g., "worker", "supervisor")
    /// * `cwd` - Working directory for the agent
    /// * `cas_root` - Optional path to the .cas directory. If provided, sets CAS_ROOT env var
    ///   so workers in clones can access the main repo's CAS state.
    /// * `supervisor_name` - For workers, the name of their supervisor (enables `target: supervisor`)
    #[allow(clippy::too_many_arguments)]
    pub fn claude(
        name: &str,
        role: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: Option<&str>,
        factory_worker_cli: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> Self {
        // Use the lead_session_id for the team lead so leadSessionId in the
        // team config matches the supervisor's --session-id. Without this,
        // Claude Code thinks it's not the leader and won't process inbox.
        let session_id = teams
            .and_then(|t| t.lead_session_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let mut env = vec![
            ("CAS_AGENT_NAME".to_string(), name.to_string()),
            ("CAS_AGENT_ROLE".to_string(), role.to_string()),
            // Mark this process as running inside a factory session.
            // Read by pre_tool jail, close_ops, mcp server, and task update
            // to branch factory-vs-standalone behavior. Without this, the
            // is_factory_worker check in pre_tool.rs fails (it requires both
            // CAS_AGENT_ROLE=worker AND CAS_FACTORY_MODE), so workers get
            // jailed on every verification-pending task.
            ("CAS_FACTORY_MODE".to_string(), "1".to_string()),
            // Provide session ID so CAS MCP server can self-register without hooks
            ("CAS_SESSION_ID".to_string(), session_id.clone()),
            // Set clone path so subagents know the worktree directory
            (
                "CAS_CLONE_PATH".to_string(),
                cwd.to_string_lossy().to_string(),
            ),
            // Suppress interactive prompts and cost/UX chrome for factory agents.
            // The network-quieting vars are role-gated below (cas-7d8e).
            ("DISABLE_COST_WARNINGS".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE".to_string(),
                "1".to_string(),
            ),
            ("IS_DEMO".to_string(), "true".to_string()),
        ];

        // Claude uses this path to associate file-history snapshots and project
        // skills with a session. A factory worker must name its own worktree,
        // not inherit the supervisor/main checkout's project directory: the
        // latter lets Claude restore a foreign session snapshot over a tracked
        // skill in the worker checkout (GH #507).
        if role == "worker" {
            env.push((
                "CLAUDE_PROJECT_DIR".to_string(),
                cwd.to_string_lossy().to_string(),
            ));
        }

        // Set CAS_ROOT env var if provided (enables workers in clones to use main's .cas)
        if let Some(root) = cas_root {
            env.push(("CAS_ROOT".to_string(), root.to_string_lossy().to_string()));
        }

        // Set supervisor name for workers (enables `target: supervisor` in message action)
        if let Some(sup) = supervisor_name {
            env.push(("CAS_SUPERVISOR_NAME".to_string(), sup.to_string()));
        }
        push_factory_worker_metadata_env(&mut env, role, factory_worker_cli, model, effort);

        // cas-0bf4: cap cargo parallelism inside factory worker processes
        // so a 4-worker factory doesn't stack `num_cpus`-way rustc jobs
        // per worker and wedge the host via scheduler starvation
        // (cas-4513 Claude Code JS crash-screen symptom). Emitted only
        // for role="worker"; supervisor stays uncapped.
        push_worker_cargo_env(&mut env, role);
        // cas-eb39: share dependency compilation across isolated worktrees
        // without serializing their Cargo target directories.
        push_worker_build_cache_env(&mut env, role);
        // cas-7d8e: silence non-essential traffic and pin the binary for
        // workers only. The supervisor keeps feature-flag evaluation (Remote
        // Control) and the auto-updater (security patches).
        push_worker_quiet_network_env(&mut env, role);
        // Point factory workers at the repo's bootstrapped Zig toolchain so the
        // ghostty_vt_sys build script finds Zig on the first `cargo build` in a
        // fresh worker worktree instead of failing and forcing a manual
        // bootstrap-zig.sh + ZIG export dance (observed in the cas-3522 shakedown).
        push_worker_zig_env(&mut env, role, cas_root);

        // Enable native Agent Teams for inter-agent messaging
        if teams.is_some() {
            env.push((
                "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
                "1".to_string(),
            ));
        }

        let mut args = vec!["--dangerously-skip-permissions".to_string()];
        // Claude's native Agent Teams permission router can suspend a worker
        // on a team-lead approval request even when the worker's
        // PreToolUse/PermissionRequest hooks return `allow`. The dangerous
        // skip flag does not disable that router (and the resulting request
        // may never reach the transcript). Workers are already constrained by
        // the factory hook/worktree jail, so explicitly select Claude's
        // command-line bypass mode for the worker process itself.
        if role == "worker" {
            args.push("--permission-mode".to_string());
            args.push("bypassPermissions".to_string());
        }
        args.push("--session-id".to_string());
        args.push(session_id);
        if let Some(m) = model {
            args.push("--model".to_string());
            args.push(m.to_string());
        }
        // Add --effort flag only when the installed claude CLI supports it (cas-6ee8).
        // Older CLI versions crash silently (non-zero exit, no useful error) when they
        // encounter an unrecognised flag. The probe runs once and caches the result.
        // When effort is None, omit the flag entirely — defer to the CLI's own default.
        // Role-based defaults (supervisor→xhigh, worker→high) belong in the cascade
        // resolver (WorkerSpec / factory config), not here in the spawn layer (cas-34f7f).
        if claude_supports_effort_flag() {
            if let Some(e) = effort {
                args.push("--effort".to_string());
                args.push(e.to_string());
            }
        } else {
            tracing::warn!(
                "Skipping --effort flag: installed claude CLI does not support it \
                 (upgrade claude to enable effort/reasoning-depth control). \
                 Agents will run at the CLI's default effort level."
            );
        }

        // Add native Agent Teams CLI flags.
        // All agents (including the supervisor) get --teammate-mode tmux
        // so Claude Code activates inbox polling for everyone.
        if let Some(t) = teams {
            args.push("--team-name".to_string());
            args.push(t.team_name.clone());
            args.push("--agent-id".to_string());
            args.push(t.agent_id.clone());
            args.push("--agent-name".to_string());
            args.push(t.agent_name.clone());
            args.push("--agent-color".to_string());
            args.push(t.agent_color.clone());
            args.push("--agent-type".to_string());
            args.push(t.agent_type.clone());
            args.push("--teammate-mode".to_string());
            args.push("tmux".to_string());
            if let Some(ref parent_id) = t.parent_session_id {
                args.push("--parent-session-id".to_string());
                args.push(parent_id.clone());
            }
            // Per-role settings file — both supervisor and workers ship a
            // `permissions.allow` list via `--settings` so Read/Write/Edit/
            // Glob/Grep/Bash/NotebookEdit auto-approve instead of escalating
            // through team-approval routing (the phantom `team-lead` hang).
            // If the caller leaves `settings_path` as None (CLI usage,
            // standalone claude invocations, or tests that deliberately
            // opt out), no flag is emitted — that's a valid fallback.
            if let Some(ref settings_path) = t.settings_path {
                args.push("--settings".to_string());
                args.push(settings_path.clone());
            }

            // Factory agents cannot route AskUserQuestion to the human: under
            // native Agent Teams it becomes a self-directed permission prompt.
            // Remove it from Claude's model-visible tool surface up front.
            // The PreToolUse denial remains wired as defense in depth for
            // stale/replayed sessions or harnesses that bypass this argv path.
            args.push("--disallowedTools".to_string());
            args.push("AskUserQuestion".to_string());
        }

        // cas-0bf4: optionally lower the worker's scheduling priority so
        // the supervisor's Claude Code event loop wins scheduler fights.
        // Only fires for role="worker" when `CAS_FACTORY_NICE_WORKER=1`
        // is set by the supervisor-side factory config bridge.
        let (command, args) = maybe_wrap_with_nice("claude", args, role);

        Self {
            command,
            args,
            cwd: Some(cwd),
            env,
            env_remove: vec![],
            rows: 24,
            cols: 80,
        }
    }

    /// Create config for a Codex CLI instance
    ///
    /// # Arguments
    /// * `name` - Agent name
    /// * `role` - Agent role (e.g., "worker", "supervisor")
    /// * `cwd` - Working directory for the agent
    /// * `cas_root` - Optional path to the .cas directory. If provided, sets CAS_ROOT env var
    /// * `supervisor_name` - For workers, the name of their supervisor (enables `target: supervisor`)
    #[allow(clippy::too_many_arguments)]
    pub fn codex(
        name: &str,
        role: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: Option<&str>,
        factory_worker_cli: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
        _teams: Option<&TeamsSpawnConfig>,
    ) -> Self {
        // Native Agent Teams is Claude Code-only; Codex CLI does not support it.
        // Keep the human-readable `codex-<name>-` prefix (operator clarity in
        // worker_status/agent_list); nothing parses it, so the suffix is a uuid.
        let session_id = format!("codex-{name}-{}", uuid::Uuid::new_v4());

        let mut env = vec![
            ("CAS_AGENT_NAME".to_string(), name.to_string()),
            ("CAS_AGENT_ROLE".to_string(), role.to_string()),
            // Mark this process as running inside a factory session.
            // See equivalent comment in `claude()` above — without this the
            // pre_tool verification-jail exemption for factory workers does
            // not fire and workers get jailed on every pending task.
            ("CAS_FACTORY_MODE".to_string(), "1".to_string()),
            // Provide session ID so CAS MCP server can self-register without hooks
            ("CAS_SESSION_ID".to_string(), session_id.clone()),
            (
                "CAS_CLONE_PATH".to_string(),
                cwd.to_string_lossy().to_string(),
            ),
            // Suppress interactive prompts, telemetry, and updates for factory agents
            (
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
                "1".to_string(),
            ),
            ("DISABLE_AUTOUPDATER".to_string(), "1".to_string()),
            ("DISABLE_COST_WARNINGS".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE".to_string(),
                "1".to_string(),
            ),
            ("IS_DEMO".to_string(), "true".to_string()),
        ];

        if let Ok(term) = std::env::var("TERM")
            && term.contains("ghostty")
        {
            env.push(("TERM".to_string(), "xterm-256color".to_string()));
        }

        if let Some(root) = cas_root {
            env.push(("CAS_ROOT".to_string(), root.to_string_lossy().to_string()));
        }

        if let Some(sup) = supervisor_name {
            env.push(("CAS_SUPERVISOR_NAME".to_string(), sup.to_string()));
        }
        push_factory_worker_metadata_env(&mut env, role, factory_worker_cli, model, effort);

        // cas-0bf4: see equivalent comment in `claude()`.
        push_worker_cargo_env(&mut env, role);
        // cas-eb39: see equivalent comment in `claude()`.
        push_worker_build_cache_env(&mut env, role);
        // cas-3522 follow-on: see equivalent comment in `claude()`.
        push_worker_zig_env(&mut env, role, cas_root);

        let mut args = vec!["--yolo".to_string(), "--no-alt-screen".to_string()];

        // cas-bbc2: spawn-inject the CAS MCP server so every Codex agent (worker
        // and supervisor) has mcp__cs__* tools even when the project was never
        // integrated for the Codex harness (no .codex/config.toml). Must precede
        // the developer_instructions block but order among `-c` flags is
        // irrelevant to Codex.
        push_codex_mcp_server_args(
            &mut args,
            &session_id,
            name,
            role,
            cas_root,
            supervisor_name,
        );

        if let Some(m) = model {
            args.push("--model".to_string());
            args.push(m.to_string());
        }
        // Codex CLI 0.146.0 has no --effort flag; effort is set via -c TOML override.
        // Valid values: none, minimal, low, medium, high, xhigh (same vocabulary as Claude).
        // Unlike claude(), we do NOT apply a role-based default when effort is None — Codex
        // CLI's built-in server-side default is acceptable and avoids hard-coding a TOML
        // override that would need revisiting each Codex release.
        if let Some(e) = effort {
            args.push("-c".to_string());
            args.push(format!("model_reasoning_effort={e}"));
        }

        if role == "supervisor" {
            let escaped = CODEX_SUPERVISOR_INSTRUCTIONS.replace('"', "\\\"");
            args.push("--config".to_string());
            args.push(format!("developer_instructions=\"{escaped}\""));
        } else if role == "worker" {
            let escaped = CODEX_WORKER_INSTRUCTIONS.replace('"', "\\\"");
            args.push("--config".to_string());
            args.push(format!("developer_instructions=\"{escaped}\""));

            // Pass startup workflow as initial prompt arg so Codex executes it immediately.
            // This is more reliable than post-spawn typed injection, which can leave text
            // in the composer without submitting in some startup timing windows.
            let startup_prompt = format!(
                "{CODEX_WORKER_STARTUP_PREFIX}\n\
                 2) Run mcp__cs__task action=mine\n\
                 3) If a task is assigned: choose exactly ONE task, run mcp__cs__task action=show then action=start, add a progress note, implement it, commit and push, then close it (or hand to supervisor) BEFORE starting any other task. Never start more than one task at a time — this is the factory coordination policy even though verification waits do not block unrelated work.\n\
                 4) If no tasks are assigned: send mcp__cs__coordination action=message target=supervisor confirming ready state\n\
                 5) Do NOT message target=cas. Use target=supervisor."
            );
            args.push(startup_prompt);
        }

        // cas-0bf4: see equivalent comment in `claude()`.
        let (command, args) = maybe_wrap_with_nice("codex", args, role);

        Self {
            command,
            args,
            cwd: Some(cwd),
            env,
            env_remove: vec![],
            rows: 24,
            cols: 80,
        }
    }

    /// Create config for an OpenCode TUI instance (cas-753a).
    ///
    /// The worker launch contract is intentionally self-contained: an
    /// absolute project argument selects the worktree, `--prompt` starts the
    /// factory workflow, and `OPENCODE_CONFIG_CONTENT` injects both the
    /// role-specific primary agent and the local `cas serve` MCP server.
    /// Model selectors remain caller-owned full `provider/model` strings.
    #[allow(clippy::too_many_arguments)]
    pub fn opencode(
        name: &str,
        role: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: Option<&str>,
        factory_worker_cli: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
        _teams: Option<&TeamsSpawnConfig>,
    ) -> Self {
        // OpenCode creates its own fresh `ses_*` identity internally. CAS uses
        // this synthetic id for MCP registration until the later session-
        // mapping adapter persists the OpenCode id observed at runtime.
        let session_id = format!("opencode-{name}-{}", uuid::Uuid::new_v4());
        let cwd = std::path::absolute(cwd)
            .expect("OpenCode factory launch requires a resolvable working directory");
        let cwd_text = cwd.to_string_lossy().to_string();

        let mut env = vec![
            ("CAS_AGENT_NAME".to_string(), name.to_string()),
            ("CAS_AGENT_ROLE".to_string(), role.to_string()),
            ("CAS_FACTORY_MODE".to_string(), "1".to_string()),
            ("CAS_SESSION_ID".to_string(), session_id),
            ("CAS_CLONE_PATH".to_string(), cwd_text.clone()),
            ("PWD".to_string(), cwd_text.clone()),
            ("IS_DEMO".to_string(), "true".to_string()),
            // Real worker construction passes `factory_worker_cli=None`, so
            // the process must identify itself unconditionally.
            ("CAS_FACTORY_WORKER_CLI".to_string(), "opencode".to_string()),
        ];
        if let Some(root) = cas_root {
            env.push(("CAS_ROOT".to_string(), root.to_string_lossy().to_string()));
        }
        if let Some(supervisor) = supervisor_name {
            env.push(("CAS_SUPERVISOR_NAME".to_string(), supervisor.to_string()));
        }
        push_factory_worker_metadata_env(&mut env, role, factory_worker_cli, model, effort);
        push_worker_cargo_env(&mut env, role);
        push_worker_build_cache_env(&mut env, role);
        push_worker_zig_env(&mut env, role, cas_root);

        let (agent_name, instructions) = if role == "supervisor" {
            ("cassy-supervisor", OPENCODE_SUPERVISOR_INSTRUCTIONS)
        } else {
            ("cassy-worker", OPENCODE_WORKER_INSTRUCTIONS)
        };
        let mut agent = serde_json::Map::new();
        agent.insert(
            "description".to_string(),
            serde_json::Value::String(format!("Cassy factory {role}")),
        );
        agent.insert(
            "mode".to_string(),
            serde_json::Value::String("primary".to_string()),
        );
        agent.insert(
            "prompt".to_string(),
            serde_json::Value::String(instructions.to_string()),
        );
        if let Some(model) = model {
            agent.insert(
                "model".to_string(),
                serde_json::Value::String(model.to_string()),
            );
        }
        if let Some(effort) = effort {
            agent.insert(
                "variant".to_string(),
                serde_json::Value::String(effort.to_string()),
            );
        }
        let mut agents = serde_json::Map::new();
        agents.insert(agent_name.to_string(), serde_json::Value::Object(agent));
        let mut inline_config = serde_json::json!({
            "agent": serde_json::Value::Object(agents),
            "mcp": {
                "cas": {
                    "type": "local",
                    "command": ["cas", "serve"],
                    "enabled": true
                }
            }
        });
        if let Some(provider_config) = model.and_then(opencode_hosted_provider_config) {
            inline_config["provider"] = provider_config["provider"].clone();
        }
        env.push((
            "OPENCODE_CONFIG_CONTENT".to_string(),
            serde_json::to_string(&inline_config)
                .expect("OpenCode inline config contains only serializable values"),
        ));

        let mut args = vec![cwd_text];
        if let Some(model) = model {
            args.push("--model".to_string());
            args.push(model.to_string());
        }
        args.push("--agent".to_string());
        args.push(agent_name.to_string());
        if role == "worker" {
            args.push("--prompt".to_string());
            args.push(OPENCODE_WORKER_STARTUP_PROMPT.to_string());
        }
        args.push("--auto".to_string());

        let (command, args) = maybe_wrap_with_nice("opencode", args, role);
        Self {
            command,
            args,
            cwd: Some(cwd),
            env,
            env_remove: vec![],
            rows: 24,
            cols: 80,
        }
    }

    /// Create config for a Grok Build CLI instance (EPIC cas-8888, cas-6569
    /// Phase 2).
    ///
    /// # Verified against the installed grok 1.0.5 binary
    /// (2026-08-25; the retained 0.2.114 receipt remains historical).
    /// The complete real `PtyConfig::grok` isolated-worker matrix is recorded
    /// by `grok_factory_contract_runtime` and the typed
    /// `grok-build-1.0.5-2026-08-25` conformance receipt.
    /// - `-s/--session-id <uuid>`: "Use a specific session UUID for a NEW
    ///   conversation" — same anti-overwrite model as Claude, so (like
    ///   `claude()`) we always generate a fresh uuid rather than Codex's
    ///   `codex-<name>-<uuid>` prefixed style. Phase 4's transcript
    ///   resolver keys on this exact value.
    /// - `-m/--model <MODEL>`, `--reasoning-effort <EFFORT>` (aliased
    ///   `--effort`; same minimal/low/medium/high/xhigh vocabulary as
    ///   Claude/Codex — supplied by the caller's backend adapter),
    ///   `--cwd <CWD>`, `--permission-mode <MODE>` (accepts
    ///   `bypassPermissions`) all confirmed present.
    /// - `--rules <RULES>`: "Extra rules to append to the system prompt" —
    ///   used to deliver GROK_WORKER_INSTRUCTIONS/GROK_SUPERVISOR_INSTRUCTIONS
    ///   in place of the hook-based context bundle Claude uses (Grok's
    ///   SessionStart hook fires but its stdout is ignored — delta #2).
    ///
    /// # MCP registration — deliberately NOT mirroring `push_codex_mcp_server_args`
    ///
    /// The task description asked for a `push_grok_mcp_server_args` mirroring
    /// Codex's `-c mcp_servers.*` spawn-time override. Checked the real binary
    /// before writing one: `grok --help` has NO ephemeral per-launch
    /// config-override flag at all (no `-c`/`--config key=value`). The only
    /// way to register an MCP server is `grok mcp add` — which writes to
    /// PERSISTENT `~/.grok/config.toml` / `./.grok/config.toml`, not a
    /// per-spawn flag. So there is no flag to emit here.
    ///
    /// Verified live (this session) that `cas` is ALREADY discoverable
    /// without any Grok-specific setup: `grok mcp doctor` in this repo shows
    /// `cas (stdio: cas serve) ✓ handshake OK, ✓ 11 tools discovered`, and
    /// `grok inspect` resolves it from `~/.claude.json` (with a fallback
    /// tested live: removing `~/.grok/config.toml`'s own `cas` entry, Grok
    /// still found `cas` via `~/.claude.json`). Every CAS-integrated project
    /// already has one of `.mcp.json`/`~/.claude.json` from Claude support —
    /// unlike Codex, which has an entirely separate `.codex/config.toml`
    /// surface that many Claude-first projects never set up (the actual
    /// cas-bbc2 problem), there's no equivalent "never integrated" gap for
    /// Grok to defend against.
    ///
    /// Identity/factory env (CAS_SESSION_ID, CAS_AGENT_NAME, etc.) is set as
    /// plain process env vars on the grok process itself — exactly like
    /// `claude()` — relying on ordinary child-process environment
    /// inheritance when Grok spawns `cas serve` per its resolved MCP config.
    /// This is the same mechanism Claude Code already relies on for
    /// CAS_SESSION_ID delivery in production (`.mcp.json`'s own `cas` entry
    /// has no `env` block either), and Grok is explicitly "Claude-Code-shaped"
    /// (delta description). Revisit with a live end-to-end spawn if a real
    /// worker fails to auto-register.
    #[allow(clippy::too_many_arguments)]
    pub fn grok(
        name: &str,
        role: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: Option<&str>,
        factory_worker_cli: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
        _teams: Option<&TeamsSpawnConfig>,
    ) -> Self {
        // Grok uses Claude's anti-overwrite session model (a fresh uuid per
        // NEW conversation) — no Codex-style name-prefixed session id, so
        // Phase 4's transcript resolver can key on this value directly.
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut env = vec![
            ("CAS_AGENT_NAME".to_string(), name.to_string()),
            ("CAS_AGENT_ROLE".to_string(), role.to_string()),
            // See equivalent comment in `claude()` — without this the
            // pre_tool verification-jail exemption for factory workers
            // does not fire.
            ("CAS_FACTORY_MODE".to_string(), "1".to_string()),
            // Provide session ID so CAS MCP server can self-register
            // without hooks (Grok's SessionStart hook output is ignored —
            // this env var is the load-bearing identity signal instead).
            ("CAS_SESSION_ID".to_string(), session_id.clone()),
            (
                "CAS_CLONE_PATH".to_string(),
                cwd.to_string_lossy().to_string(),
            ),
            ("IS_DEMO".to_string(), "true".to_string()),
            // cas-921f (P1 fix-round): a grok process is ALWAYS grok — set
            // this unconditionally rather than relying on
            // `factory_worker_cli`, which the worker spawn path leaves
            // `None` (see `build_worker_config`, cas-mux/pane/mod.rs). Grok
            // has no Codex-style `-c mcp_servers.*.env` override mechanism
            // (Phase 2 finding: it discovers `cas serve` natively via
            // `.mcp.json`/`~/.claude.json`, inheriting this process's own
            // env like `claude()` does) — so this is the ONLY place the
            // signal can originate. Without it, `apply_factory_worker_metadata`
            // (mcp/daemon.rs) / the direct-registration fallback
            // (hooks/handlers/handlers_session.rs) never see
            // `CAS_FACTORY_WORKER_CLI`, `worker_cli_from_agent` silently
            // defaults every grok worker to `Claude`, and Phase 4's entire
            // harness-aware is-wedged/liveness path never engages for a
            // real grok worker — it globs `~/.claude/projects/*` for a
            // transcript that lives at `~/.grok/sessions/*` and always
            // resolves `None`.
            ("CAS_FACTORY_WORKER_CLI".to_string(), "grok".to_string()),
        ];

        if let Some(root) = cas_root {
            env.push(("CAS_ROOT".to_string(), root.to_string_lossy().to_string()));
        }
        if let Some(sup) = supervisor_name {
            env.push(("CAS_SUPERVISOR_NAME".to_string(), sup.to_string()));
        }
        push_factory_worker_metadata_env(&mut env, role, factory_worker_cli, model, effort);

        // cas-0bf4: see equivalent comment in `claude()`.
        push_worker_cargo_env(&mut env, role);
        // cas-eb39: see equivalent comment in `claude()`.
        push_worker_build_cache_env(&mut env, role);
        // cas-3522 follow-on: see equivalent comment in `claude()`.
        push_worker_zig_env(&mut env, role, cas_root);

        let mut args = vec![
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "--session-id".to_string(),
            session_id,
            "--cwd".to_string(),
            cwd.to_string_lossy().to_string(),
        ];
        if let Some(m) = model {
            args.push("-m".to_string());
            args.push(m.to_string());
        }
        if let Some(e) = effort {
            args.push("--reasoning-effort".to_string());
            args.push(e.to_string());
        }

        // Context-bundle substitute for the ignored-SessionStart-hook gap
        // (delta #2): append role-appropriate instructions via --rules.
        let instructions = if role == "supervisor" {
            GROK_SUPERVISOR_INSTRUCTIONS
        } else {
            GROK_WORKER_INSTRUCTIONS
        };
        args.push("--rules".to_string());
        args.push(instructions.to_string());

        // cas-0bf4: see equivalent comment in `claude()`.
        let (command, args) = maybe_wrap_with_nice("grok", args, role);

        Self {
            command,
            args,
            cwd: Some(cwd),
            env,
            env_remove: vec![],
            rows: 24,
            cols: 80,
        }
    }
}

/// Expected number of concurrent factory workers the CPU is being
/// shared among when auto-computing `CARGO_BUILD_JOBS`. On a 16-thread
/// dev box (soundwave, reference host for cas-4513 + cas-0bf4 evidence)
/// this divides the CPU budget into 4 × 4-thread slices, which kept the
/// host below scheduler saturation in the sessions where we observed
/// the Claude Code JS crash-screen wedges.
///
/// Override the assumption by setting `CAS_FACTORY_CARGO_BUILD_JOBS`
/// explicitly — e.g., a supervisor running 8 workers on a 16-thread
/// host should export `CAS_FACTORY_CARGO_BUILD_JOBS=2`.
const DEFAULT_WORKER_CONCURRENCY_ASSUMPTION: usize = 4;

/// Compute the `CARGO_BUILD_JOBS` value to export into a worker's env.
///
/// Precedence (first match wins):
///   1. `CAS_FACTORY_CARGO_BUILD_JOBS` env — set by the supervisor-side
///      factory config bridge from `factory.cargo_build_jobs` config.
///      Empty value or literal `"auto"` means "fall through to 2–4".
///   2. Auto-compute: `max(2, available_parallelism() / DEFAULT_WORKER_CONCURRENCY_ASSUMPTION)`.
///
/// Returns `None` only when auto-compute fails to read CPU topology,
/// which should be vanishingly rare. In that case we do NOT set
/// `CARGO_BUILD_JOBS` — cargo's own default (= num_cpus) then applies
/// and the cap is a no-op rather than misleading.
fn cargo_build_jobs_for_worker(active_workers: Option<usize>) -> Option<String> {
    if let Ok(explicit) = std::env::var("CAS_FACTORY_CARGO_BUILD_JOBS") {
        let trimmed = explicit.trim();
        // Case-insensitive `"auto"` falls through to the computed cap so
        // users who write `Auto`/`AUTO` in config don't silently defeat
        // the mitigation by shipping a literal non-integer value into
        // `CARGO_BUILD_JOBS`.
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
            return Some(trimmed.to_string());
        }
    }
    let cores = std::thread::available_parallelism().ok()?.get();
    let capped = std::cmp::max(2, cores / worker_concurrency_divisor(active_workers));
    Some(capped.to_string())
}

/// How many workers to assume are competing for the host when derating
/// `CARGO_BUILD_JOBS` (cas-4614, GH #107).
///
/// `DEFAULT_WORKER_CONCURRENCY_ASSUMPTION` is a **floor**, not a default that
/// a live count replaces. Taking the max in both directions would let a
/// single-worker fleet claim `cores / 1` — every thread on the box — which is
/// the storm the derate exists to prevent, and the count is only a snapshot
/// anyway: more workers usually follow. So a fleet at or below the assumption
/// keeps exactly today's behaviour, and a larger fleet derates further.
///
/// The count is read at spawn time and baked into the child's environment, so
/// a worker spawned early keeps its allocation as the fleet grows. That is a
/// known limitation, not an oversight: `CARGO_BUILD_JOBS` is an env var, and
/// re-deriving it would mean re-spawning the pane. Erring conservative at the
/// floor is what keeps the early-spawn case from being harmful.
fn worker_concurrency_divisor(active_workers: Option<usize>) -> usize {
    let observed = active_workers.unwrap_or(0);
    std::cmp::max(DEFAULT_WORKER_CONCURRENCY_ASSUMPTION, observed)
}

/// Spawn-inject the CAS MCP server into a Codex command via `-c` overrides
/// (cas-bbc2). Codex does NOT read Claude's `.mcp.json`; it only discovers the
/// CAS server from a project `.codex/config.toml` written by `cas init`/`cas
/// update` (`configure_codex_mcp_server`). Projects integrated for Claude but
/// never for Codex (e.g. gabber-studio: has `.mcp.json`, no `.codex/`) therefore
/// spawned Codex agents with **zero** `mcp__cs__*` tools, which burned the whole
/// session reverse-engineering the wire protocol and produced no code.
///
/// Injecting the server at spawn time makes every Codex agent (worker and
/// supervisor) self-contained regardless of downstream integration. We mirror
/// `configure_codex_mcp_server` exactly — `command="cas"`, `args=["serve"]`,
/// `env.CAS_CODEX_FALLBACK_SESSION="1"` — and register under the `cs` key so the
/// resulting tool prefix is `mcp__cs__` (the Codex alias used throughout the
/// factory prompts and skills; intentionally distinct from Claude's `mcp__cas__`).
///
/// Each value is valid TOML so Codex's `-c key=value` parser (value parsed as
/// TOML, raw-string fallback) yields the intended types: quoted strings stay
/// strings, `["serve"]` becomes a string array. If a project DOES ship a
/// `.codex/config.toml`, these `-c` overrides simply add the `cs` server on top
/// — they never remove the project's own entries.
fn push_codex_mcp_server_args(
    args: &mut Vec<String>,
    session_id: &str,
    name: &str,
    role: &str,
    cas_root: Option<&PathBuf>,
    supervisor_name: Option<&str>,
) {
    // cas-8c80: Codex 0.146's interactive code-mode catalog does not project
    // spawn-injected MCP servers into its nested `exec` tool catalog. Keep code
    // mode available, but make CAS a direct-only namespace so the native
    // coordination/task tools remain callable alongside code-mode tools. This
    // launch override is deliberately scoped to `mcp__cs`; it neither depends
    // on user config nor disables code mode for other supported namespaces.
    args.push("-c".to_string());
    args.push("features.code_mode.direct_only_tool_namespaces=[\"mcp__cs\"]".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.cs.command=\"cas\"".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.cs.args=[\"serve\"]".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.cs.env.CAS_CODEX_FALLBACK_SESSION=\"1\"".to_string());
    // Codex starts MCP servers with a restricted environment instead of
    // inheriting arbitrary process variables. Pin the subprocess to the same
    // store as its factory pane; otherwise isolated worktrees (and the
    // conformance sandbox) can start `cas serve` against an unrelated or
    // undiscoverable project and the namespace never reaches the tool catalog.
    if let Some(root) = cas_root {
        let root = serde_json::to_string(&root.to_string_lossy())
            .expect("serializing a filesystem path string cannot fail");
        args.push("-c".to_string());
        args.push(format!("mcp_servers.cs.env.CAS_ROOT={root}"));
    }
    // cas-3522: inject the canonical session id into the `cs` MCP server env so
    // `get_agent_id()` auto-registers the agent on its FIRST tool call — the same
    // env fast-path Claude workers rely on. Codex starts MCP servers with a
    // restricted env (it does not pass the codex process env through), so without
    // this the `cs` server comes up identity-less and whoami/task fail with
    // "Agent not registered" until the worker brute-forces a manual `register`.
    args.push("-c".to_string());
    args.push(format!(
        "mcp_servers.cs.env.CAS_SESSION_ID=\"{session_id}\""
    ));
    // cas-7592: also inject CAS_AGENT_NAME and CAS_AGENT_ROLE into the `cs` MCP
    // env. Codex does NOT forward the codex process env into MCP servers, so
    // without these the eager auto-registration (mcp/server/runtime.rs) falls
    // back to the literal name "worker" and Agent::new's default role=Standard.
    // The result is a three-way NAME-SPLIT: the mux registers the pane under
    // `name` (e.g. strong-gazelle-97) and keys delivery/interrupt on it, but the
    // CAS agent registers as name="worker"/role=Standard — invisible to
    // worker_status, agent_list, and shutdown_workers (all filter role==Worker)
    // and unaddressable (the supervisor can only send to a name == pane id).
    // Injecting `name` makes the registered agent name == pane id, and `role`
    // makes it surface as a Worker, unifying identity across registration,
    // worker_status, delivery, AND shutdown in one move. We pass `name` directly
    // (no `codex-<name>-<uuid>` suffix stripping) so names containing hyphens
    // (every adjective-animal-NN id) round-trip exactly.
    args.push("-c".to_string());
    args.push(format!("mcp_servers.cs.env.CAS_AGENT_NAME=\"{name}\""));
    args.push("-c".to_string());
    args.push(format!("mcp_servers.cs.env.CAS_AGENT_ROLE=\"{role}\""));
    // cas-ae2f: Codex does not inherit the pane's arbitrary process env into
    // its MCP subprocess. The factory spawner already supplies the owning
    // supervisor name to worker PtyConfig; mirror that trusted spawn datum
    // into `cs` so the documented logical `target=supervisor` route resolves.
    if let Some(supervisor_name) = supervisor_name {
        args.push("-c".to_string());
        args.push(format!(
            "mcp_servers.cs.env.CAS_SUPERVISOR_NAME=\"{supervisor_name}\""
        ));
    }
    // cas-8aaf: inject factory context env vars so the `cs` MCP server
    // knows it is running inside a factory session. Without these, the server
    // process has CAS_AGENT_ROLE=worker but lacks CAS_FACTORY_MODE. Two
    // downstream factory-policy selections depend on these values:
    //
    //   1. CAS_FACTORY_MODE=1: enables factory-worker close/review routing.
    //
    //   2. CAS_FACTORY_WORKER_CLI=codex: makes worker_harness_from_env() return
    //      Codex, which (a) causes verification_required_for_task_type() to
    //      return false for Codex workers under supervisor-owned review,
    //      (b) makes is_worker_without_subagents_from_env() true, and (c) selects
    //      mcp__cs__coordination (not mcp__cas__coordination) in guidance text.
    args.push("-c".to_string());
    args.push("mcp_servers.cs.env.CAS_FACTORY_MODE=\"1\"".to_string());
    args.push("-c".to_string());
    args.push("mcp_servers.cs.env.CAS_FACTORY_WORKER_CLI=\"codex\"".to_string());
    if role == "supervisor" {
        args.push("-c".to_string());
        args.push("mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"codex\"".to_string());
    }
}

/// Returns `true` when the installed `claude` CLI supports the `--effort` flag.
///
/// Probes by spawning `claude --help` once and scanning its output for
/// `--effort`. The result is cached in a `OnceLock` for the lifetime of the
/// process — subsequent calls return the cached value immediately.
///
/// **Conservative failure mode**: if `claude` is not on PATH, or the probe
/// subprocess fails for any reason, returns `false`. The caller should then
/// skip injecting `--effort` rather than risk a silent crash on unsupported
/// CLI versions (cas-6ee8).
///
/// **Test/CI override**: set the `CAS_FACTORY_EFFORT_SUPPORTED` env var to
/// `"1"` (force supported) or `"0"` (force unsupported) to bypass the probe
/// entirely. This prevents `OnceLock` state from leaking across tests.
pub(crate) fn claude_supports_effort_flag() -> bool {
    use std::sync::OnceLock;
    static CACHE: OnceLock<bool> = OnceLock::new();

    // Allow test / CI environments to bypass the probe via env var override.
    match std::env::var("CAS_FACTORY_EFFORT_SUPPORTED").as_deref() {
        Ok("1") => return true,
        Ok("0") => return false,
        _ => {}
    }

    *CACHE.get_or_init(|| {
        let Ok(output) = std::process::Command::new("claude").arg("--help").output() else {
            return false;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        stdout.contains("--effort") || stderr.contains("--effort")
    })
}

/// Returns `true` when an executable named `cas` is resolvable on `PATH`.
///
/// Used by the Codex spawn preflight (cas-bbc2). The CAS MCP server is now
/// spawn-injected as `mcp_servers.cs.command=cas`, but Codex still needs the
/// `cas` binary on `PATH` to actually launch it. If `PATH` is unset we return
/// `true` (skip the check) rather than risk a false refusal — the spawn will
/// surface its own error in that pathological case.
fn cas_binary_on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return true;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join("cas");
        candidate.is_file()
    })
}

/// Push the `CARGO_BUILD_JOBS` env entry into `env` when `role == "worker"`.
/// Extracted from `PtyConfig::{claude,codex}` to remove the duplicated
/// block those two call sites used to carry verbatim (cas-0bf4).
fn push_worker_cargo_env(env: &mut Vec<(String, String)>, role: &str) {
    if role != "worker" {
        return;
    }
    if let Some(cargo_jobs) = cargo_build_jobs_for_worker(None) {
        env.push(("CARGO_BUILD_JOBS".to_string(), cargo_jobs));
    }
}

/// Share Rust compilation across factory worktrees through `sccache`.
///
/// A shared `CARGO_TARGET_DIR` is safe from corruption because Cargo locks it,
/// but that lock serializes independent workers and branch switches churn the
/// shared workspace artifacts. Keeping each worktree's target directory while
/// using sccache gives concurrent workers a content-addressed dependency cache
/// instead. Workspace crates keep their normal incremental artifacts; the
/// fresh-worktree win is the dependency graph that no longer recompiles N
/// times across N workers.
///
/// The wiring is deliberately worker-only and best-effort:
/// - preserve an operator-provided `RUSTC_WRAPPER` exactly as-is;
/// - `CAS_FACTORY_DISABLE_SCCACHE=1` is the emergency kill switch;
/// - do nothing when `sccache` is absent, because exporting a missing wrapper
///   makes every Cargo invocation fail;
/// - raise sccache's small 10 GiB default to 50 GiB unless the operator already
///   chose a size. This repo's duplicated dependency artifacts were measured at
///   roughly 170 GiB across the main checkout and six worker worktrees.
fn push_worker_build_cache_env(env: &mut Vec<(String, String)>, role: &str) {
    let entries = worker_build_cache_env(
        role,
        std::env::var("CAS_FACTORY_DISABLE_SCCACHE").as_deref() == Ok("1"),
        std::env::var("RUSTC_WRAPPER").ok().as_deref(),
        std::env::var("SCCACHE_CACHE_SIZE").ok().as_deref(),
        executable_on_path("sccache"),
    );
    env.extend(entries);
}

/// Env-free decision core for [`push_worker_build_cache_env`]. Keeping the
/// policy pure makes every branch testable without process-wide env races.
fn worker_build_cache_env(
    role: &str,
    disabled: bool,
    rustc_wrapper: Option<&str>,
    cache_size: Option<&str>,
    sccache_available: bool,
) -> Vec<(String, String)> {
    if role != "worker" || disabled || rustc_wrapper.is_some() || !sccache_available {
        return Vec::new();
    }

    let mut entries = vec![("RUSTC_WRAPPER".to_string(), "sccache".to_string())];
    if cache_size.is_none() {
        entries.push(("SCCACHE_CACHE_SIZE".to_string(), "50G".to_string()));
    }
    entries
}

fn executable_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

/// Quiet a factory *worker's* outbound traffic and pin its binary (cas-7d8e).
///
/// `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` and `DISABLE_AUTOUPDATER` are
/// right for workers: a worker is a short-lived, headless lane that must not
/// swap its CLI binary mid-EPIC, and nobody drives it from a phone.
///
/// They are wrong for the supervisor, for two confirmed reasons:
/// - Remote Control requires feature-flag evaluation, which the traffic var
///   disables outright (`claude doctor` says so explicitly). The supervisor is
///   the one long-running session an operator plausibly wants to steer
///   remotely.
/// - The traffic var *bundles* the auto-updater kill switch, so a factory
///   supervisor never receives security/CVE patches (anthropics/claude-code#53899).
///
/// Follows the same worker-only shape as `push_worker_cargo_env`. The quiet-UX
/// vars (`DISABLE_COST_WARNINGS`, `CLAUDE_CODE_DISABLE_TERMINAL_TITLE`,
/// `IS_DEMO`) are unrelated to flag evaluation and stay unconditional.
///
/// Known trade-off, deliberately accepted: with the updater live, a supervisor
/// can update the shared binary under `~/.local/share/claude/versions/`
/// mid-run, so workers spawned before and after an update may differ in
/// version. If that bites in practice, the narrower fix is to keep
/// `DISABLE_AUTOUPDATER` unconditional and exempt only the traffic var — at
/// the cost of the security-patch half of this change.
fn push_worker_quiet_network_env(env: &mut Vec<(String, String)>, role: &str) {
    if role != "worker" {
        return;
    }
    env.push((
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
        "1".to_string(),
    ));
    env.push(("DISABLE_AUTOUPDATER".to_string(), "1".to_string()));
}

/// Export `ZIG` into a worker's env pointing at the repo's bootstrapped Zig
/// binary (`<repo>/.context/zig/zig`), so the `ghostty_vt_sys` build script can
/// find Zig on the first `cargo build` inside a fresh worker worktree.
///
/// Without this, a worker's first build fails in `ghostty_vt_sys` ("could not
/// find Zig") and the worker has to discover + run `scripts/bootstrap-zig.sh`
/// and export `ZIG` by hand before it can compile anything — wasted turns
/// observed in the cas-3522 Codex shakedown.
///
/// Worker-only and best-effort, mirroring `push_worker_cargo_env`:
/// - `cas_root` is `<repo>/.cas`, so the repo root is its parent.
/// - We only set `ZIG` when the binary actually exists at the expected path;
///   pointing `ZIG` at a missing file would break builds worse than leaving it
///   unset (the build script would still try to bootstrap). The path is
///   absolute, so it resolves correctly from any worktree cwd.
fn push_worker_zig_env(env: &mut Vec<(String, String)>, role: &str, cas_root: Option<&PathBuf>) {
    if role != "worker" {
        return;
    }
    let Some(repo) = cas_root.and_then(|r| r.parent()) else {
        return;
    };
    let zig = repo.join(".context").join("zig").join("zig");
    if zig.is_file() {
        env.push(("ZIG".to_string(), zig.to_string_lossy().to_string()));
    }
}

/// If `CAS_FACTORY_NICE_WORKER=1` is set in the supervisor's env and
/// `role == "worker"`, wrap the spawn command in `nice -n 10` so the
/// worker's process tree (including cargo-driven rustc jobs) runs at
/// a lower scheduling priority than the supervisor. Supervisor panes
/// stay at nice 0 and therefore win CPU-contention fights, which keeps
/// the factory steerable when workers start cargo-storming (cas-0bf4).
///
/// Non-worker roles and sessions without the sentinel env are passed
/// through unchanged. `nice` must be on PATH (standard on every Linux
/// and macOS host CAS supports); if it isn't, the worker will fail to
/// spawn with a clear "nice not found" error from the PTY layer rather
/// than silently running unwrapped — that's the safer fallback.
fn maybe_wrap_with_nice(command: &str, args: Vec<String>, role: &str) -> (String, Vec<String>) {
    if role != "worker" {
        return (command.to_string(), args);
    }
    if std::env::var("CAS_FACTORY_NICE_WORKER").as_deref() != Ok("1") {
        return (command.to_string(), args);
    }
    // Default niceness increment is 10; honour CAS_FACTORY_NICE_LEVEL
    // for power users who want a harder or softer cap. Parse as i32 so
    // a typo like `CAS_FACTORY_NICE_LEVEL=high` cannot propagate to
    // `nice -n high claude ...` and kill every worker spawn with an
    // opaque PTY error — we quietly fall back to the default 10.
    let level = std::env::var("CAS_FACTORY_NICE_LEVEL")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "10".to_string());
    let mut new_args = Vec::with_capacity(args.len() + 3);
    new_args.push("-n".to_string());
    new_args.push(level);
    new_args.push(command.to_string());
    new_args.extend(args);
    ("nice".to_string(), new_args)
}

/// Events emitted by a PTY
#[derive(Debug, Clone)]
pub enum PtyEvent {
    /// Terminal output (raw bytes - parsing done by ghostty_vt)
    Output(Vec<u8>),
    /// Process exited
    Exited(Option<i32>),
    /// Error occurred
    Error(String),
}

/// A running PTY process
pub struct Pty {
    /// Unique identifier
    id: String,
    /// Writer handle for sending input
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Channel for receiving raw output
    event_rx: mpsc::Receiver<PtyEvent>,
    /// Handle to the reader task
    _reader_handle: std::thread::JoinHandle<()>,
    /// Child process handle
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Master PTY (keep alive)
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Whether this PTY is running Codex CLI
    is_codex: bool,
}

/// Whether a configured command ultimately launches the Codex harness.
///
/// Factory workers may be started through `nice -n … codex` to keep a busy
/// compile from starving the supervisor.  This classification feeds prompt
/// submission timing as well as the spawn preflight, so it must describe the
/// executable behind the wrapper rather than only the outer command.
fn command_launches_codex(command: &str, args: &[String]) -> bool {
    command == "codex" || (command == "nice" && args.iter().any(|arg| arg == "codex"))
}

impl Pty {
    /// Spawn a new PTY with the given configuration
    pub fn spawn(id: impl Into<String>, config: PtyConfig) -> Result<Self> {
        let id = id.into();
        let is_codex = command_launches_codex(&config.command, &config.args);

        // cas-bbc2 preflight: a Codex agent's CAS MCP server is spawn-injected as
        // `mcp_servers.cs.command=cas`, but Codex can only launch it if the `cas`
        // binary is resolvable on PATH. Detect Codex by the direct command or the
        // niced wrapper form (cas-0bf4) so the preflight covers both. Refuse loudly
        // with remediation rather than spawning a worker that comes up with zero
        // CAS tools and flails.
        let codex_spawn = is_codex;
        if codex_spawn && !cas_binary_on_path() {
            return Err(Error::pty(
                "Codex agent cannot start: the `cas` MCP server binary is not on PATH. \
                 CAS is spawn-injected as mcp_servers.cs (command=cas), but Codex needs \
                 `cas` resolvable to launch it. Install CAS / add it to PATH, or run \
                 `cas init` / `cas update` in this project to enable the Codex harness."
                    .to_string(),
            ));
        }

        // Create PTY system and open a PTY pair
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.rows,
                cols: config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::pty(format!("Failed to open PTY: {e}")))?;

        // Build command
        let mut cmd = CommandBuilder::new(&config.command);
        cmd.args(&config.args);

        if let Some(cwd) = &config.cwd {
            cmd.cwd(cwd);
            // STEP 1 (cas-5232): Log the actual cwd being set on the PTY command so
            // the daemon trace carries an auditable record of where each worker process
            // will land.  This runs on the main thread immediately before spawn, so the
            // log timestamp is tightly coupled to the PTY launch.
            tracing::info!(
                command = %config.command,
                cwd = %cwd.display(),
                "pty: spawning process with explicit cwd"
            );
        }

        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        for key in &config.env_remove {
            cmd.env_remove(key);
        }

        // Strip CLAUDECODE to prevent nested-session detection in spawned Claude CLI
        cmd.env_remove("CLAUDECODE");

        // A resolved CLAUDE_CONFIG_DIR selects a subscription account, whether it
        // came from an explicit worker parameter or the requesting supervisor.
        // Inherited API keys or OAuth-token overrides would defeat that selection,
        // so remove them whenever the account-source marker is present. Omitted
        // config_dir remains pure inheritance, byte-for-byte with existing spawns.
        if config
            .env
            .iter()
            .any(|(key, _)| key == "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE")
        {
            cmd.env_remove("ANTHROPIC_API_KEY");
            cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
            cmd.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
            cmd.env_remove("CLAUDE_CODE_OAUTH_REFRESH_TOKEN");
            cmd.env_remove("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR");
        }

        // Same contract on the Codex side: a resolved CODEX_HOME selects a
        // ChatGPT account, and an inherited API key would silently override it.
        if config
            .env
            .iter()
            .any(|(key, _)| key == "CAS_FACTORY_CODEX_HOME_SOURCE")
        {
            cmd.env_remove("OPENAI_API_KEY");
            cmd.env_remove("CODEX_API_KEY");
            cmd.env_remove("CODEX_ACCESS_TOKEN");
        }

        if config
            .env
            .iter()
            .any(|(key, value)| key == "CAS_AGENT_ROLE" && value == "worker")
        {
            let explicit =
                config.env.iter().rev().find_map(|(key, value)| {
                    (key == "CLAUDE_CONFIG_DIR").then_some(value.as_str())
                });
            let inherited = std::env::var("CLAUDE_CONFIG_DIR").ok();
            let pushed_source = config.env.iter().rev().find_map(|(key, value)| {
                (key == "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE").then_some(value.as_str())
            });
            let (config_dir, source) = match (explicit, pushed_source, inherited.as_deref()) {
                (Some(dir), Some("explicit"), _) => (dir, "explicit param"),
                (Some(dir), Some("supervisor"), _) => (dir, "supervisor session"),
                (Some(dir), _, _) => (dir, "explicit param"),
                (None, _, Some(dir)) => (dir, "host env"),
                (None, _, None) => ("default (~/.claude)", "default"),
            };
            let worker = config
                .env
                .iter()
                .find_map(|(key, value)| (key == "CAS_AGENT_NAME").then_some(value.as_str()))
                .unwrap_or("unknown");
            tracing::info!(
                worker,
                claude_config_dir = config_dir,
                source,
                "factory worker spawn: effective Claude account directory"
            );
        }

        // Spawn the child process
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::pty(format!("Failed to spawn command: {e}")))?;

        // Drop slave - the child process owns it now
        drop(pair.slave);

        // Get reader and writer
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::pty(format!("Failed to clone reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| Error::pty(format!("Failed to get writer: {e}")))?;

        let writer = Arc::new(Mutex::new(writer));

        if is_codex {
            // Codex blocks at startup until it gets a reply to its cursor-position
            // (DSR) probe, so feed it a synthetic cursor-position report a few times
            // to unstick rendering.
            //
            // This MUST NOT use `tokio::spawn`. `Pty::spawn` is a *synchronous*
            // constructor and is routinely called from threads with no ambient Tokio
            // runtime (the factory daemon's blocking spawn path), where `tokio::spawn`
            // panics with "there is no reactor running" and takes the whole supervisor
            // down at INIT (cas-e202). Use a detached std thread + `blocking_lock`,
            // mirroring `reader_loop` below, so the keep-alive has zero runtime
            // dependency. `blocking_lock` is safe here because this is a fresh OS
            // thread, never a Tokio worker thread.
            let writer = Arc::clone(&writer);
            std::thread::spawn(move || {
                for _ in 0..10 {
                    let mut locked = writer.blocking_lock();
                    let _ = locked.write_all(b"\x1b[1;1R");
                    let _ = locked.flush();
                    drop(locked);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            });
        }

        // Create channel for events - larger buffer for multi-agent scenarios
        let (event_tx, event_rx) = mpsc::channel::<PtyEvent>(1024);

        // Spawn reader thread - sends raw bytes, no parsing
        let reader_handle = std::thread::spawn({
            let writer = Arc::clone(&writer);
            move || {
                Self::reader_loop(reader, writer, event_tx);
            }
        });

        Ok(Self {
            id,
            writer,
            event_rx,
            _reader_handle: reader_handle,
            child,
            master: pair.master,
            is_codex,
        })
    }

    /// Reader loop that forwards raw PTY output
    fn reader_loop(
        mut reader: Box<dyn Read + Send>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        event_tx: mpsc::Sender<PtyEvent>,
    ) {
        // Larger buffer for high-throughput scenarios (6 Claudes generating long responses)
        let mut buf = [0u8; 16384];
        let mut carry: Vec<u8> = Vec::new();

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    if !carry.is_empty() {
                        let _ =
                            event_tx.blocking_send(PtyEvent::Output(std::mem::take(&mut carry)));
                    }
                    let _ = event_tx.blocking_send(PtyEvent::Exited(None));
                    break;
                }
                Ok(n) => {
                    let (data, new_carry, saw_cpr) =
                        filter_cursor_position_requests(&carry, &buf[..n]);
                    carry = new_carry;

                    if saw_cpr {
                        let mut locked = writer.blocking_lock();
                        let _ = locked.write_all(b"\x1b[1;1R");
                        let _ = locked.flush();
                    }

                    if !data.is_empty() && event_tx.blocking_send(PtyEvent::Output(data)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.blocking_send(PtyEvent::Error(e.to_string()));
                    break;
                }
            }
        }
    }

    /// Get the PTY's identifier
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns true when this PTY is running Codex CLI.
    pub fn is_codex(&self) -> bool {
        self.is_codex
    }

    /// Get a clone of the writer handle (for concurrent writing)
    pub fn writer_handle(&self) -> Arc<Mutex<Box<dyn Write + Send>>> {
        self.writer.clone()
    }

    /// Write input to the PTY (for prompt injection)
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer
            .write_all(data)
            .map_err(|e| Error::pty(format!("Write failed: {e}")))?;
        writer
            .flush()
            .map_err(|e| Error::pty(format!("Flush failed: {e}")))?;
        Ok(())
    }

    /// Write a string to the PTY
    pub async fn write_str(&self, s: &str) -> Result<()> {
        self.write(s.as_bytes()).await
    }

    /// Send a line of input (appends carriage return to submit, same as Enter key)
    pub async fn send_line(&self, line: &str) -> Result<()> {
        self.write_str(&format!("{line}\r")).await
    }

    /// Receive the next event from the PTY (blocking)
    pub async fn recv(&mut self) -> Option<PtyEvent> {
        self.event_rx.recv().await
    }

    /// Try to receive an event from the PTY (non-blocking)
    pub fn try_recv(&mut self) -> Option<PtyEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Resize the PTY
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::pty(format!("Resize failed: {e}")))
    }

    /// Send Ctrl+C to the process
    pub async fn interrupt(&self) -> Result<()> {
        self.write(&[0x03]).await
    }

    /// Send Ctrl+D (EOF) to the process
    pub async fn send_eof(&self) -> Result<()> {
        self.write(&[0x04]).await
    }

    /// Kill the child process
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Process-group identifier owned by this PTY.
    ///
    /// `portable_pty` makes the child a session leader with `setsid()`, so the
    /// direct child's PID is also the PGID inherited by its descendants.
    pub fn process_group_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Kill the child and its entire process group (cas-8c5a).
    ///
    /// `portable_pty` calls `setsid()` in the child before exec, making the
    /// spawned process a new session leader whose PGID equals its own PID.
    /// Every descendant (e.g. the node → codex tree inside a Codex worker)
    /// inherits that PGID, so `killpg(pgid, sig)` terminates the whole tree.
    ///
    /// * `force = true`  → SIGKILL  (immediate, cannot be caught)
    /// * `force = false` → SIGTERM, a real three-second grace period, then
    ///   SIGKILL escalation if any member of the group remains
    ///
    /// Falls back to `child.kill()` (SIGKILL on the direct child) when no PID
    /// is available (non-unix builds, already-reaped process, etc.).
    pub async fn kill_tree(&mut self, force: bool) {
        #[cfg(unix)]
        {
            if let Some(pid) = self.process_group_id() {
                let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
                // SAFETY: standard POSIX call; pid is a valid u32 just returned
                // by portable_pty — casting to pid_t is always safe on all
                // Unix targets where pid_t is i32/i64.
                unsafe { libc::killpg(pid as libc::pid_t, sig) };
                if !force {
                    const GRACE: std::time::Duration = std::time::Duration::from_secs(3);
                    const POLL: std::time::Duration = std::time::Duration::from_millis(50);
                    let deadline = tokio::time::Instant::now() + GRACE;
                    loop {
                        // Reap the direct child when it honored SIGTERM. This
                        // prevents a zombie group leader from making killpg(0)
                        // look live for the entire grace window.
                        let _ = self.child.try_wait();
                        // SAFETY: signal 0 only probes group existence.
                        let alive = unsafe {
                            libc::killpg(pid as libc::pid_t, 0) == 0
                                || std::io::Error::last_os_error().raw_os_error()
                                    != Some(libc::ESRCH)
                        };
                        if !alive {
                            let _ = self.child.wait();
                            return;
                        }
                        if tokio::time::Instant::now() >= deadline {
                            // SAFETY: this is the same process group selected
                            // above and it remained continuously live throughout
                            // the bounded grace period.
                            unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
                            break;
                        }
                        tokio::time::sleep(POLL).await;
                    }
                }
                // Force mode and graceful escalation both finish by touching
                // the direct-child handle as a belt-and-suspenders fallback.
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            }
        }
        // Non-Unix/no-PID fallback: portable_pty only exposes an immediate
        // direct-child kill.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Immediate group-wide teardown for synchronous process-exit paths.
    pub fn kill_tree_force(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.process_group_id() {
            // SAFETY: standard POSIX call on the PGID returned by portable_pty.
            unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
        }
        let _ = self.child.kill();
    }
}

fn filter_cursor_position_requests(carry: &[u8], chunk: &[u8]) -> (Vec<u8>, Vec<u8>, bool) {
    const CPR: [u8; 4] = [0x1b, 0x5b, 0x36, 0x6e]; // ESC [ 6 n
    const CPR_ALT: [u8; 5] = [0x1b, 0x5b, 0x3f, 0x36, 0x6e]; // ESC [ ? 6 n
    let max_seq = CPR_ALT.len();

    let total_len = carry.len() + chunk.len();
    if total_len == 0 {
        return (Vec::new(), Vec::new(), false);
    }

    let process_len = total_len.saturating_sub(max_seq - 1);
    let mut out = Vec::with_capacity(process_len);
    let mut i = 0usize;
    let mut saw_cpr = false;

    let byte_at = |idx: usize| -> u8 {
        if idx < carry.len() {
            carry[idx]
        } else {
            chunk[idx - carry.len()]
        }
    };

    while i < process_len {
        if i + CPR_ALT.len() <= total_len {
            let mut matches = true;
            for (j, byte) in CPR_ALT.iter().enumerate() {
                if byte_at(i + j) != *byte {
                    matches = false;
                    break;
                }
            }
            if matches {
                saw_cpr = true;
                i += CPR_ALT.len();
                continue;
            }
        }
        if i + CPR.len() <= total_len {
            let mut matches = true;
            for (j, byte) in CPR.iter().enumerate() {
                if byte_at(i + j) != *byte {
                    matches = false;
                    break;
                }
            }
            if matches {
                saw_cpr = true;
                i += CPR.len();
                continue;
            }
        }
        out.push(byte_at(i));
        i += 1;
    }

    let mut new_carry = Vec::with_capacity(total_len - process_len);
    for idx in process_len..total_len {
        new_carry.push(byte_at(idx));
    }

    (out, new_carry, saw_cpr)
}

#[cfg(test)]
mod tests {
    use crate::pty::*;
    use std::sync::{Mutex, MutexGuard};

    // cas-0bf4: module-wide serialization for any test that constructs a
    // `PtyConfig::{claude,codex}` with role="worker". Those constructors
    // now read process-wide env vars (CAS_FACTORY_CARGO_BUILD_JOBS and
    // CAS_FACTORY_NICE_WORKER) at call time; parallel tests can race if
    // one sets the sentinel while another asserts on the non-wrapped
    // command name. All worker-role PtyConfig tests must hold this
    // mutex for the duration of their body.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Lock the env mutex, clear the cas-0bf4 sentinels on entry, and
    /// clear them again on drop. Safe to use from any test that may
    /// observe or mutate those vars.
    pub(crate) struct ScopedEnv {
        _guard: MutexGuard<'static, ()>,
    }

    impl ScopedEnv {
        pub(crate) fn new() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: mutex serializes env mutation across tests.
            unsafe {
                std::env::remove_var("CAS_FACTORY_CARGO_BUILD_JOBS");
                std::env::remove_var("CAS_FACTORY_NICE_WORKER");
                std::env::remove_var("CAS_FACTORY_NICE_LEVEL");
                // cas-6ee8: clear effort-support override so tests don't bleed state.
                std::env::remove_var("CAS_FACTORY_EFFORT_SUPPORTED");
            }
            Self { _guard: guard }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: mutex held for duration of this scope.
            unsafe {
                std::env::remove_var("CAS_FACTORY_CARGO_BUILD_JOBS");
                std::env::remove_var("CAS_FACTORY_NICE_WORKER");
                std::env::remove_var("CAS_FACTORY_NICE_LEVEL");
                // cas-6ee8: clear effort-support override on exit.
                std::env::remove_var("CAS_FACTORY_EFFORT_SUPPORTED");
            }
        }
    }

    #[tokio::test]
    async fn test_pty_config_default() {
        let config = PtyConfig::default();
        assert_eq!(config.command, "bash");
        assert_eq!(config.rows, 24);
        assert_eq!(config.cols, 80);
    }

    #[tokio::test]
    async fn test_pty_config_claude() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        assert_eq!(config.command, "claude");
        assert!(
            config
                .args
                .contains(&"--dangerously-skip-permissions".to_string())
        );
        assert_eq!(
            config
                .args
                .windows(2)
                .find(|pair| pair[0] == "--permission-mode")
                .map(|pair| pair[1].as_str()),
            Some("bypassPermissions"),
            "factory Claude workers must bypass the team leader permission router"
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_AGENT_NAME" && v == "test-agent")
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_AGENT_ROLE" && v == "worker")
        );
        // No CAS_ROOT when not provided
        assert!(!config.env.iter().any(|(k, _)| k == "CAS_ROOT"));
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CLAUDE_PROJECT_DIR" && v == "/tmp"),
            "a worker must explicitly scope Claude file-history and skill loading to its clone"
        );
    }

    #[test]
    fn factory_worker_skill_load_keeps_tracked_worktree_porcelain_clean_cas_fb41() {
        use std::process::Command;

        let _e = ScopedEnv::new();
        let sandbox = std::env::temp_dir().join(format!("cas-fb41-{}", uuid::Uuid::new_v4()));
        let main = sandbox.join("main");
        let worker = sandbox.join("worker");
        std::fs::create_dir_all(main.join(".claude/skills/cas-history-probe"))
            .expect("create tracked skill fixture");
        std::fs::write(
            main.join(".claude/skills/cas-history-probe/SKILL.md"),
            "tracked skill\n",
        )
        .expect("write tracked skill fixture");

        fn git(dir: &std::path::Path, args: &[&str]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Cassy Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "Cassy Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["add", ".claude/skills/cas-history-probe/SKILL.md"]);
        git(&main, &["commit", "-qm", "add tracked project skill"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/history-probe",
                worker.to_str().expect("UTF-8 worker path"),
            ],
        );

        let config = PtyConfig::claude(
            "history-probe",
            "worker",
            worker.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
        );

        // Simulate a worker startup loading the tracked project skill while a
        // main-checkout value is present in its inherited environment. The
        // launch config must replace it with the worker worktree before the
        // harness can select file-history for the skill.
        let load = Command::new("sh")
            .args([
                "-c",
                "test \"$CLAUDE_PROJECT_DIR\" = \"$PWD\" && cat \"$CLAUDE_PROJECT_DIR/.claude/skills/cas-history-probe/SKILL.md\" >/dev/null",
            ])
            .current_dir(&worker)
            .env("CLAUDE_PROJECT_DIR", &main)
            .envs(config.env.iter().map(|(key, value)| (key, value)))
            .output()
            .expect("start worker skill loader probe");
        assert!(
            load.status.success(),
            "worker skill loading must use its own project directory: {}",
            String::from_utf8_lossy(&load.stderr)
        );

        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&worker)
            .output()
            .expect("inspect worker porcelain");
        assert!(status.status.success(), "git status must succeed");
        assert!(
            status.stdout.is_empty(),
            "skill loading must leave the isolated worker worktree clean, got: {}",
            String::from_utf8_lossy(&status.stdout)
        );

        std::fs::remove_dir_all(&sandbox).expect("remove test sandbox");
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_cas_root() {
        let cas_root = PathBuf::from("/home/user/project/.cas");
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            Some(&cas_root),
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_ROOT" && v == "/home/user/project/.cas")
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_supervisor() {
        let config = PtyConfig::claude(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            Some("test-supervisor"),
            None,
            None, // model
            None, // effort
            None, // teams
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_SUPERVISOR_NAME" && v == "test-supervisor")
        );
    }

    #[tokio::test]
    async fn test_pty_config_sets_clone_path() {
        let config = PtyConfig::claude(
            "test-worker",
            "worker",
            PathBuf::from("/tmp/worktree"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_CLONE_PATH" && v == "/tmp/worktree")
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_model() {
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            Some("claude-opus-4-6"),
            None, // effort
            None, // teams
        );
        assert!(config.args.contains(&"--model".to_string()));
        assert!(config.args.contains(&"claude-opus-4-6".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_claude_without_model() {
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        assert!(!config.args.contains(&"--model".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_codex_with_model() {
        let config = PtyConfig::codex(
            "test-agent",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            Some("gpt-5.3-codex"),
            None, // effort
            None, // teams
        );
        assert!(config.args.contains(&"--model".to_string()));
        assert!(config.args.contains(&"gpt-5.3-codex".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_codex_worker_uses_cs_prefix() {
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp__cs__"),
            "Codex worker instructions should use mcp__cs__ prefix"
        );
    }

    /// cas-bbc2 AC#2: a Codex worker spawn must inject the CAS MCP server via
    /// `-c` overrides so it has `mcp__cs__*` tools without a project
    /// `.codex/config.toml`. Mirrors `configure_codex_mcp_server`.
    #[tokio::test]
    async fn test_pty_config_codex_worker_injects_cas_mcp_server() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.command=\"cas\""),
            "codex worker must inject mcp_servers.cs.command=cas; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.args=[\"serve\"]"),
            "codex worker must inject mcp_servers.cs.args=[\"serve\"]; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_CODEX_FALLBACK_SESSION=\"1\""),
            "codex worker must inject CAS_CODEX_FALLBACK_SESSION env; got: {all_args}"
        );
        assert!(
            all_args.contains("features.code_mode.direct_only_tool_namespaces=[\"mcp__cs\"]"),
            "codex worker must project CAS as a direct-only namespace; got: {all_args}"
        );
        assert!(
            !all_args.contains("features.code_mode.enabled=false")
                && !all_args.contains("code_mode=false"),
            "CAS projection must not disable Codex code mode; got: {all_args}"
        );
    }

    #[tokio::test]
    async fn test_pty_config_codex_pins_cas_mcp_server_to_factory_root() {
        let _e = ScopedEnv::new();
        let root = PathBuf::from("/tmp/cas root with spaces");
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            Some(&root),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            config
                .args
                .iter()
                .any(|arg| arg == "mcp_servers.cs.env.CAS_ROOT=\"/tmp/cas root with spaces\""),
            "Codex's restricted MCP environment must receive the pane's CAS_ROOT"
        );
    }

    /// cas-bbc2: the supervisor is equally self-contained — a Codex supervisor
    /// must also get the spawn-injected CAS MCP server.
    #[tokio::test]
    async fn test_pty_config_codex_supervisor_injects_cas_mcp_server() {
        let config = PtyConfig::codex(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.command=\"cas\"")
                && all_args.contains("mcp_servers.cs.args=[\"serve\"]")
                && all_args.contains("mcp_servers.cs.env.CAS_CODEX_FALLBACK_SESSION=\"1\""),
            "codex supervisor must inject the cas MCP server; got: {all_args}"
        );
    }

    /// cas-bbc2 AC#3: the Codex worker startup prompt must drive a single-task
    /// loop (start exactly one task at a time), preserving factory coordination
    /// policy without claiming verification blocks unrelated work.
    #[tokio::test]
    async fn test_pty_config_codex_worker_single_task_loop() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("exactly ONE task"),
            "startup prompt must instruct starting exactly one task at a time; got: {all_args}"
        );
        assert!(
            !all_args.contains("show/start each task"),
            "old batch-start wording must be gone; got: {all_args}"
        );
        // The developer_instructions must carry the same discipline.
        assert!(
            all_args.contains("Work exactly ONE task at a time"),
            "worker developer_instructions must enforce one-task-at-a-time; got: {all_args}"
        );
    }

    /// cas-3522: the Codex `cs` MCP server must receive the canonical session id
    /// so `get_agent_id()` auto-registers the agent on the first tool call.
    /// Without it the worker burns ~6 failed calls ("Agent not registered")
    /// before brute-forcing a manual register.
    #[tokio::test]
    async fn test_pty_config_codex_worker_injects_session_id() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_SESSION_ID=\"codex-test-worker-"),
            "codex worker must inject CAS_SESSION_ID into the cs MCP env; got: {all_args}"
        );
        // The same id must be exported into the process env (they must match).
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_SESSION_ID" && v.starts_with("codex-test-worker-")),
            "codex worker process env must carry the matching CAS_SESSION_ID; got: {:?}",
            config.env
        );
    }

    /// cas-7592: the Codex `cs` MCP server must also receive CAS_AGENT_NAME (==
    /// the mux pane id) and CAS_AGENT_ROLE so eager auto-registration names the
    /// agent after its pane (not the literal "worker") and marks it role=Worker.
    /// Without this the codex worker is unaddressable (delivery keys on a name ==
    /// pane id) and invisible to worker_status/shutdown_workers (both filter
    /// role==Worker).
    #[tokio::test]
    async fn test_pty_config_codex_worker_injects_agent_name_and_role() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "strong-gazelle-97",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_AGENT_NAME=\"strong-gazelle-97\""),
            "codex worker must inject CAS_AGENT_NAME (== pane id) into the cs MCP env; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_AGENT_ROLE=\"worker\""),
            "codex worker must inject CAS_AGENT_ROLE=worker into the cs MCP env; got: {all_args}"
        );
    }

    /// cas-7592: the Codex supervisor's `cs` MCP server likewise receives its
    /// pane name and role so it registers under its real identity.
    #[tokio::test]
    async fn test_pty_config_codex_supervisor_injects_agent_name_and_role() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "brave-panther-92",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_AGENT_NAME=\"brave-panther-92\""),
            "codex supervisor must inject CAS_AGENT_NAME (== pane id) into the cs MCP env; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_AGENT_ROLE=\"supervisor\""),
            "codex supervisor must inject CAS_AGENT_ROLE=supervisor into the cs MCP env; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"codex\""),
            "codex supervisor must inject CAS_FACTORY_SUPERVISOR_CLI=codex into the cs MCP env; got: {all_args}"
        );
    }

    /// cas-3522: the supervisor's cs MCP server also needs CAS_SESSION_ID.
    #[tokio::test]
    async fn test_pty_config_codex_supervisor_injects_session_id() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_SESSION_ID=\"codex-test-supervisor-"),
            "codex supervisor must inject CAS_SESSION_ID into the cs MCP env; got: {all_args}"
        );
    }

    /// cas-8aaf: the Codex `cs` MCP server must receive CAS_FACTORY_MODE=1 and
    /// CAS_FACTORY_WORKER_CLI=codex so the MCP server's is_factory_worker check
    /// fires and worker_harness_from_env() returns Codex. Without these env vars
    /// the `cs` server doesn't know it is inside a factory session:
    ///   - CAS_FACTORY_MODE absent → factory close/review routing is wrong.
    ///   - CAS_FACTORY_WORKER_CLI absent → worker_harness_from_env() defaults to
    ///     Claude → verification_required_for_task_type() returns the wrong policy.
    #[tokio::test]
    async fn test_pty_config_codex_worker_injects_factory_mode_and_worker_cli_cas_8aaf() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_FACTORY_MODE=\"1\""),
            "codex worker cs MCP server must receive CAS_FACTORY_MODE=1 so \
             is_factory_worker is true; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_FACTORY_WORKER_CLI=\"codex\""),
            "codex worker cs MCP server must receive CAS_FACTORY_WORKER_CLI=codex so \
             worker_harness_from_env() returns Codex; got: {all_args}"
        );
    }

    /// cas-8aaf: a Codex supervisor spawn must also receive the factory env vars
    /// in its `cs` MCP server so supervisor-side harness detection is consistent.
    #[tokio::test]
    async fn test_pty_config_codex_supervisor_injects_factory_mode_and_worker_cli_cas_8aaf() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_FACTORY_MODE=\"1\""),
            "codex supervisor cs MCP server must receive CAS_FACTORY_MODE=1; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp_servers.cs.env.CAS_FACTORY_WORKER_CLI=\"codex\""),
            "codex supervisor cs MCP server must receive CAS_FACTORY_WORKER_CLI=codex; \
             got: {all_args}"
        );
    }

    /// cas-3522: the startup prompt and worker instructions must NOT drive a
    /// `session_start` invocation anymore — auto-registration replaces it.
    #[tokio::test]
    async fn test_pty_config_codex_worker_no_session_start_invocation() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            !all_args.contains("action=session_start"),
            "codex worker must no longer invoke session_start; got: {all_args}"
        );
        // whoami remains the first explicit identity check.
        assert!(
            all_args.contains("action=whoami"),
            "codex worker startup should still confirm identity via whoami; got: {all_args}"
        );
    }

    /// cas-3522 follow-on: a worker gets `ZIG` pointed at the repo's
    /// bootstrapped binary when it exists; non-workers and missing-binary cases
    /// leave `ZIG` unset.
    #[tokio::test]
    async fn test_push_worker_zig_env_sets_zig_for_worker_when_present() {
        let dir = std::env::temp_dir().join("cas-3522-zig-env-test");
        let _ = std::fs::remove_dir_all(&dir);
        let zig = dir.join(".context").join("zig").join("zig");
        std::fs::create_dir_all(zig.parent().unwrap()).unwrap();
        std::fs::write(&zig, b"#!/bin/sh\n").unwrap();
        let cas_root = dir.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let zig_str = zig.to_string_lossy().to_string();

        let mut worker_env: Vec<(String, String)> = Vec::new();
        push_worker_zig_env(&mut worker_env, "worker", Some(&cas_root));
        assert!(
            worker_env.iter().any(|(k, v)| k == "ZIG" && v == &zig_str),
            "worker must get ZIG pointing at the bootstrapped binary; got: {worker_env:?}"
        );

        let mut sup_env: Vec<(String, String)> = Vec::new();
        push_worker_zig_env(&mut sup_env, "supervisor", Some(&cas_root));
        assert!(
            !sup_env.iter().any(|(k, _)| k == "ZIG"),
            "supervisor must NOT get ZIG; got: {sup_env:?}"
        );

        // Missing binary -> no ZIG even for a worker.
        let empty_root = dir.join("empty").join(".cas");
        std::fs::create_dir_all(&empty_root).unwrap();
        let mut missing_env: Vec<(String, String)> = Vec::new();
        push_worker_zig_env(&mut missing_env, "worker", Some(&empty_root));
        assert!(
            !missing_env.iter().any(|(k, _)| k == "ZIG"),
            "worker must not get ZIG when the binary is absent; got: {missing_env:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_worker_build_cache_env_decision_table() {
        let none = Vec::<(String, String)>::new();
        assert_eq!(
            worker_build_cache_env("supervisor", false, None, None, true),
            none,
            "supervisors must not receive worker build-cache policy"
        );
        assert_eq!(
            worker_build_cache_env("worker", true, None, None, true),
            none,
            "the factory kill switch must disable sccache"
        );
        assert_eq!(
            worker_build_cache_env("worker", false, Some("rustc-wrapper"), None, true),
            none,
            "an operator-provided RUSTC_WRAPPER must win"
        );
        assert_eq!(
            worker_build_cache_env("worker", false, Some(""), None, true),
            none,
            "even an explicitly empty RUSTC_WRAPPER is an operator choice"
        );
        assert_eq!(
            worker_build_cache_env("worker", false, None, None, false),
            none,
            "a missing sccache binary must not poison every Cargo invocation"
        );
        assert_eq!(
            worker_build_cache_env("worker", false, None, None, true),
            vec![
                ("RUSTC_WRAPPER".to_string(), "sccache".to_string()),
                ("SCCACHE_CACHE_SIZE".to_string(), "50G".to_string()),
            ],
            "the worker default should enable sccache and raise its cache size"
        );
        assert_eq!(
            worker_build_cache_env("worker", false, None, Some("80G"), true),
            vec![("RUSTC_WRAPPER".to_string(), "sccache".to_string())],
            "an operator-provided SCCACHE_CACHE_SIZE must be inherited unchanged"
        );
    }

    /// cas-bbc2: `cas_binary_on_path()` returns true when an executable named
    /// `cas` lives in a PATH entry. Builds a temp dir with a fake `cas` file and
    /// points PATH at it, under ENV_LOCK to avoid racing other env-mutating tests.
    #[tokio::test]
    async fn test_cas_binary_on_path_detects_binary() {
        let _e = ScopedEnv::new();
        let dir = std::env::temp_dir().join("cas-bbc2-preflight-present");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cas"), b"#!/bin/sh\n").unwrap();
        let saved = std::env::var_os("PATH");
        // SAFETY: ENV_LOCK held via ScopedEnv serializes PATH mutation.
        unsafe {
            std::env::set_var("PATH", &dir);
        }
        let found = cas_binary_on_path();
        unsafe {
            match &saved {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found, "cas_binary_on_path must find a `cas` file on PATH");
    }

    /// cas-bbc2: when no `cas` exists on PATH, the preflight helper reports false
    /// so `Pty::spawn` can refuse a Codex spawn loudly.
    #[tokio::test]
    async fn test_cas_binary_on_path_absent_when_missing() {
        let _e = ScopedEnv::new();
        let dir = std::env::temp_dir().join("cas-bbc2-preflight-absent");
        std::fs::create_dir_all(&dir).unwrap();
        let saved = std::env::var_os("PATH");
        // SAFETY: ENV_LOCK held via ScopedEnv serializes PATH mutation.
        unsafe {
            std::env::set_var("PATH", &dir);
        }
        let found = cas_binary_on_path();
        unsafe {
            match &saved {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !found,
            "cas_binary_on_path must be false when no `cas` is on PATH"
        );
    }

    #[tokio::test]
    async fn test_pty_config_codex_supervisor_instructions() {
        let config = PtyConfig::codex(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("Cassy Factory Supervisor"),
            "Codex supervisor should have supervisor instructions"
        );
    }

    /// cas-83c8: the Codex supervisor prompt must explain that worker messages
    /// arrive asynchronously as injected turns and must be triaged + replied to
    /// via mcp__cs__coordination, not treated as a fresh startup.
    #[tokio::test]
    async fn test_pty_config_codex_supervisor_handles_injected_worker_messages() {
        let config = PtyConfig::codex(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("Message from <sender>"),
            "supervisor prompt must reference the injected message framing; got: {all_args}"
        );
        assert!(
            all_args.contains("triage trigger"),
            "supervisor prompt must frame incoming worker messages as a triage trigger; got: {all_args}"
        );
        assert!(
            all_args.contains("mcp__cs__coordination action=message target=<worker>"),
            "supervisor prompt must tell it to reply/redirect via mcp__cs__coordination; got: {all_args}"
        );
        // Must keep the mcp__cs__ alias (not the claude mcp__cas__ alias).
        assert!(
            !all_args.contains("mcp__cas__"),
            "Codex supervisor prompt must use the mcp__cs__ alias, never mcp__cas__; got: {all_args}"
        );
    }

    /// cas-83c8: the Codex worker prompt must instruct continued availability
    /// after a task closes (you are not permanently done) and acting on injected
    /// supervisor messages, without breaking the one-task-at-a-time rule.
    #[tokio::test]
    async fn test_pty_config_codex_worker_stays_available_for_injected_messages() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("not permanently done"),
            "worker prompt must say it is not permanently done after closing a task; got: {all_args}"
        );
        assert!(
            all_args.contains("Message from <sender>"),
            "worker prompt must instruct acting on injected 'Message from <sender>' turns; got: {all_args}"
        );
        // The one-task-at-a-time discipline must still be present alongside the
        // new continued-availability clause.
        assert!(
            all_args.contains("Work exactly ONE task at a time"),
            "worker prompt must retain one-task-at-a-time discipline; got: {all_args}"
        );
        assert!(
            all_args.contains("finish or hand off your current task before starting the next"),
            "worker prompt must reconcile continued availability with the one-task rule; got: {all_args}"
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_teams() {
        let teams = TeamsSpawnConfig {
            team_name: "test-team".to_string(),
            agent_id: "worker-1@test-team".to_string(),
            agent_name: "worker-1".to_string(),
            agent_color: "blue".to_string(),
            agent_type: "general-purpose".to_string(),
            parent_session_id: Some("lead-session-123".to_string()),
            lead_session_id: None,
            settings_path: None,
        };
        let config = PtyConfig::claude(
            "worker-1",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            Some(&teams),
        );
        assert!(config.args.contains(&"--team-name".to_string()));
        assert!(config.args.contains(&"test-team".to_string()));
        assert!(config.args.contains(&"--agent-id".to_string()));
        assert!(config.args.contains(&"worker-1@test-team".to_string()));
        assert!(config.args.contains(&"--agent-name".to_string()));
        assert!(config.args.contains(&"--agent-color".to_string()));
        assert!(config.args.contains(&"--teammate-mode".to_string()));
        assert!(config.args.contains(&"tmux".to_string()));
        assert!(config.args.contains(&"--parent-session-id".to_string()));
        assert!(config.args.contains(&"lead-session-123".to_string()));
        // Workers get --session-id for CAS agent auto-registration
        assert!(config.args.contains(&"--session-id".to_string()));
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS" && v == "1")
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_custom_effort() {
        // Hold ENV_LOCK and force effort-supported=1 so the version guard
        // does not race with other tests that set CAS_FACTORY_EFFORT_SUPPORTED=0.
        let _e = ScopedEnv::new();
        // SAFETY: ENV_LOCK held by ScopedEnv.
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "1");
        }
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,        // model
            Some("low"), // effort override
            None,        // teams
        );
        let effort_idx = config
            .args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be present");
        assert_eq!(
            config.args[effort_idx + 1],
            "low",
            "custom effort should override hardcoded default"
        );
    }

    /// cas-34f7f: when effort=None, --effort must be OMITTED (supervisor).
    /// Role-based defaults belong in the cascade resolver, not pty.rs.
    #[tokio::test]
    async fn test_pty_config_claude_supervisor_no_effort_omits_flag() {
        let _e = ScopedEnv::new();
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "1");
        }
        let config = PtyConfig::claude(
            "sup",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // no effort — must NOT default to "xhigh" anymore
            None,
        );
        assert!(
            !config.args.contains(&"--effort".to_string()),
            "None effort must omit --effort entirely (cas-34f7f); got: {:?}",
            config.args
        );
    }

    /// cas-34f7f: when effort=None, --effort must be OMITTED (worker).
    #[tokio::test]
    async fn test_pty_config_claude_worker_no_effort_omits_flag() {
        let _e = ScopedEnv::new();
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "1");
        }
        let config = PtyConfig::claude(
            "wrk",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // no effort — must NOT default to "high" anymore
            None,
        );
        assert!(
            !config.args.contains(&"--effort".to_string()),
            "None effort must omit --effort entirely (cas-34f7f); got: {:?}",
            config.args
        );
    }

    /// cas-34f7f: explicit effort is passed through verbatim for Claude.
    #[tokio::test]
    async fn test_pty_config_claude_worker_with_explicit_effort() {
        let _e = ScopedEnv::new();
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "1");
        }
        let config = PtyConfig::claude(
            "wrk",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            Some("medium"),
            None,
        );
        let idx = config
            .args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be present when Some(effort) is given");
        assert_eq!(config.args[idx + 1], "medium");
    }

    /// cas-34f7f: Codex worker with explicit effort → --config model_reasoning_effort=<v>.
    #[tokio::test]
    async fn test_pty_config_codex_worker_with_effort_high() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "wrk",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            Some("high"),
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("model_reasoning_effort=high"),
            "Codex worker must emit model_reasoning_effort=high; got: {all_args}"
        );
    }

    /// cas-34f7f: Codex worker with None effort → no model_reasoning_effort arg.
    #[tokio::test]
    async fn test_pty_config_codex_worker_with_no_effort() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "wrk",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None, // no effort
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            !all_args.contains("model_reasoning_effort"),
            "Codex worker with None effort must omit model_reasoning_effort; got: {all_args}"
        );
    }

    // ── cas-6ee8: --effort version guard tests ────────────────────────────

    #[tokio::test]
    async fn test_effort_flag_included_when_supported() {
        // Verify that PtyConfig::claude includes --effort when the installed
        // claude CLI reports support (forced via env var to avoid a live probe).
        let _e = ScopedEnv::new();
        // SAFETY: ENV_LOCK held by ScopedEnv.
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "1");
        }

        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            Some("low"),
            None,
        );
        let effort_idx = config
            .args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be present when effort is supported (cas-6ee8)");
        assert_eq!(config.args[effort_idx + 1], "low");
    }

    #[tokio::test]
    async fn test_effort_flag_omitted_when_unsupported() {
        // Regression test for cas-6ee8: when the installed claude CLI does not
        // support --effort, the flag must be silently omitted (not injected)
        // so the subprocess does not crash with an unrecognised-flag error.
        let _e = ScopedEnv::new();
        // SAFETY: ENV_LOCK held by ScopedEnv.
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "0");
        }

        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            Some("low"),
            None,
        );
        assert!(
            !config.args.contains(&"--effort".to_string()),
            "--effort must be absent when CLI does not support it (cas-6ee8), got: {:?}",
            config.args
        );
        assert!(
            !config.args.contains(&"low".to_string()),
            "effort value must also be absent when --effort is skipped, got: {:?}",
            config.args
        );
    }

    #[tokio::test]
    async fn test_effort_flag_omitted_unsupported_default_effort() {
        // When effort=None (default) and CLI is unsupported, neither the flag
        // nor a role-default value should appear.
        let _e = ScopedEnv::new();
        // SAFETY: ENV_LOCK held by ScopedEnv.
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "0");
        }

        let config = PtyConfig::claude(
            "sup",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None, // no explicit effort — would default to "xhigh" if supported
            None,
        );
        assert!(
            !config.args.contains(&"--effort".to_string()),
            "--effort must be absent for supervisor when unsupported (cas-6ee8), got: {:?}",
            config.args
        );
        assert!(
            !config.args.contains(&"xhigh".to_string()),
            "role-default effort value must also be absent, got: {:?}",
            config.args
        );
    }

    #[tokio::test]
    async fn test_claude_supports_effort_flag_env_override() {
        // Verify the env var bypass works for both true and false.
        let _e = ScopedEnv::new();
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "1");
        }
        assert!(
            claude_supports_effort_flag(),
            "env override '1' must return true"
        );
        unsafe {
            std::env::set_var("CAS_FACTORY_EFFORT_SUPPORTED", "0");
        }
        assert!(
            !claude_supports_effort_flag(),
            "env override '0' must return false"
        );
    }

    #[tokio::test]
    async fn test_pty_config_codex_with_effort() {
        // Worker-role tests must hold ENV_LOCK (via ScopedEnv) so CAS_FACTORY_NICE_WORKER
        // cannot be set concurrently, which would shift arg indices via maybe_wrap_with_nice.
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,           // model
            Some("medium"), // effort
            None,           // teams
        );
        // Multiple `-c` flags exist now that the CAS MCP server is spawn-injected
        // (cas-bbc2), so locate the effort override by its value rather than
        // assuming it is the first `-c`. It must be emitted as a `-c` pair.
        let effort_idx = config
            .args
            .iter()
            .position(|a| a == "model_reasoning_effort=medium")
            .expect("effort override must emit model_reasoning_effort TOML key");
        assert_eq!(
            config.args[effort_idx - 1],
            "-c",
            "model_reasoning_effort override must be preceded by a -c flag"
        );
    }

    #[tokio::test]
    async fn test_pty_config_codex_no_effort_when_none() {
        // Worker-role tests must hold ENV_LOCK (via ScopedEnv).
        let _e = ScopedEnv::new();
        let config = PtyConfig::codex(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort — None means no override; Codex CLI server-side default applies
            None, // teams
        );
        assert!(
            !config
                .args
                .iter()
                .any(|a| a.starts_with("model_reasoning_effort")),
            "no model_reasoning_effort arg should be emitted when effort is None"
        );
        // The CAS MCP server injection (cas-bbc2) always emits `-c` flags, so we
        // can no longer assert the total absence of `-c`. Instead assert that the
        // only `-c` overrides present are the MCP server and direct namespace
        // projection ones — none configure reasoning effort.
        let c_values: Vec<&String> = config
            .args
            .windows(2)
            .filter(|w| w[0] == "-c")
            .map(|w| &w[1])
            .collect();
        assert!(
            c_values.iter().all(|v| {
                v.starts_with("mcp_servers.cs.")
                    || *v == "features.code_mode.direct_only_tool_namespaces=[\"mcp__cs\"]"
            }),
            "with effort=None the only -c overrides should be the cas MCP server injection and direct namespace projection; got: {c_values:?}"
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_teams_lead() {
        let teams = TeamsSpawnConfig {
            team_name: "test-team".to_string(),
            agent_id: "supervisor@test-team".to_string(),
            agent_name: "supervisor".to_string(),
            agent_color: "green".to_string(),
            agent_type: "team-lead".to_string(),
            parent_session_id: None,
            lead_session_id: None,
            settings_path: None,
        };
        let config = PtyConfig::claude(
            "supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            Some(&teams),
        );
        // Lead also gets --teammate-mode so it polls its inbox
        assert!(config.args.contains(&"--teammate-mode".to_string()));
        assert!(config.args.contains(&"tmux".to_string()));
        // No --parent-session-id for the lead
        assert!(!config.args.contains(&"--parent-session-id".to_string()));
    }

    /// When `TeamsSpawnConfig::settings_path` is set (as it is for the
    /// supervisor in factory mode), the spawned `claude` invocation must
    /// include `--settings <path>` so Claude Code loads the allowlist that
    /// sidesteps the self-leadership routing deadlock. Workers without a
    /// `settings_path` must not get the flag.
    #[tokio::test]
    async fn test_pty_config_claude_teams_supervisor_gets_settings_flag() {
        let settings_path = "/home/pippenz/.claude/teams/deadlock-team/supervisor-settings.json";
        let teams = TeamsSpawnConfig {
            team_name: "deadlock-team".to_string(),
            agent_id: "supervisor@deadlock-team".to_string(),
            agent_name: "supervisor".to_string(),
            agent_color: "green".to_string(),
            agent_type: "team-lead".to_string(),
            parent_session_id: None,
            lead_session_id: None,
            settings_path: Some(settings_path.to_string()),
        };
        let config = PtyConfig::claude(
            "supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None, // effort
            Some(&teams),
        );
        assert!(
            config.args.contains(&"--settings".to_string()),
            "supervisor spawn must include --settings flag"
        );
        assert!(
            config.args.contains(&settings_path.to_string()),
            "supervisor spawn must pass the settings file path"
        );
        assert!(
            config
                .args
                .windows(2)
                .any(|pair| pair == ["--disallowedTools", "AskUserQuestion"]),
            "factory supervisor spawn must remove AskUserQuestion from Claude's tool surface"
        );
    }

    /// Workers now ship their own settings file (cas-e15d). When
    /// `settings_path` is populated, the `--settings <path>` flag must
    /// appear in argv so `claude` loads the per-worker allowlist and the
    /// phantom `team-lead` escalation cannot fire.
    #[tokio::test]
    async fn test_pty_config_claude_teams_worker_gets_settings_flag() {
        let settings_path = "/home/pippenz/.claude/teams/deadlock-team/worker-1-settings.json";
        let teams = TeamsSpawnConfig {
            team_name: "deadlock-team".to_string(),
            agent_id: "worker-1@deadlock-team".to_string(),
            agent_name: "worker-1".to_string(),
            agent_color: "blue".to_string(),
            agent_type: "general-purpose".to_string(),
            parent_session_id: Some("lead-session-xyz".to_string()),
            lead_session_id: None,
            settings_path: Some(settings_path.to_string()),
        };
        let config = PtyConfig::claude(
            "worker-1",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None, // effort
            Some(&teams),
        );
        assert!(
            config.args.contains(&"--settings".to_string()),
            "worker spawn must include --settings flag"
        );
        assert!(
            config.args.contains(&settings_path.to_string()),
            "worker spawn must pass the worker settings file path"
        );
        assert!(
            config
                .args
                .windows(2)
                .any(|pair| pair == ["--disallowedTools", "AskUserQuestion"]),
            "factory worker spawn must remove AskUserQuestion from Claude's tool surface"
        );
    }

    #[tokio::test]
    async fn test_pty_config_non_factory_claude_keeps_default_tool_surface() {
        let config = PtyConfig::claude(
            "solo",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            !config
                .args
                .iter()
                .any(|arg| arg == "--disallowedTools" || arg == "AskUserQuestion"),
            "non-factory Claude spawn must not apply the factory-only tool exclusion"
        );
    }

    /// Argv builder contract: when `settings_path` is deliberately left as
    /// `None` (CLI usage, tests that opt out), the flag must be absent. This
    /// is the correctness gate for the `if let Some(..)` branch — not a
    /// statement about worker doctrine (workers get a path in production).
    #[tokio::test]
    async fn test_pty_config_claude_teams_no_settings_path_omits_flag() {
        let teams = TeamsSpawnConfig {
            team_name: "no-settings-team".to_string(),
            agent_id: "worker-bare@no-settings-team".to_string(),
            agent_name: "worker-bare".to_string(),
            agent_color: "blue".to_string(),
            agent_type: "general-purpose".to_string(),
            parent_session_id: Some("lead-session-xyz".to_string()),
            lead_session_id: None,
            settings_path: None,
        };
        let config = PtyConfig::claude(
            "worker-bare",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None, // effort
            Some(&teams),
        );
        assert!(
            !config.args.contains(&"--settings".to_string()),
            "no settings_path → argv must omit --settings"
        );
    }

    // cas-0bf4: resource-contention mitigation tests.
    //
    // These exercise `cargo_build_jobs_for_worker` and
    // `maybe_wrap_with_nice` plus their integration with
    // `PtyConfig::claude`. They poke process-wide env vars, so they
    // share a serializing mutex to avoid cross-test flakes when the
    // suite runs with multiple threads. Scope is per-test: each test
    // clears its own env on entry and on the exit via the guard.
    mod cas_0bf4_resource_contention {
        use crate::pty::tests::ScopedEnv;
        use crate::pty::*;

        /// Local twin of the helper in `claude_config_dir_contract_tests`,
        /// which is private to that module.
        fn env_value(env: &[(String, String)], key: &str) -> Option<String> {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.to_string())
        }

        #[test]
        fn cargo_build_jobs_honours_explicit_env_override() {
            let _e = ScopedEnv::new();
            // SAFETY: _e holds ENV_LOCK.
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "3");
            }
            assert_eq!(cargo_build_jobs_for_worker(None).as_deref(), Some("3"));
        }

        #[test]
        fn cargo_build_jobs_trims_whitespace_override() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "  6  ");
            }
            assert_eq!(cargo_build_jobs_for_worker(None).as_deref(), Some("6"));
        }

        #[test]
        fn cargo_build_jobs_auto_falls_through_to_computed() {
            let _e = ScopedEnv::new();
            // Explicit "auto" reads as fallthrough, computed value comes back.
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "auto");
            }
            let got = cargo_build_jobs_for_worker(None)
                .expect("available_parallelism should succeed on test host");
            let n: usize = got.parse().expect("computed CARGO_BUILD_JOBS must parse");
            assert!(
                n >= 2,
                "floor of 2 must hold even on 1–4 core hosts: got {n}"
            );
        }

        #[test]
        fn cargo_build_jobs_empty_env_falls_through_to_computed() {
            let _e = ScopedEnv::new();
            // No env set at all → compute. Same assertion as "auto".
            let got = cargo_build_jobs_for_worker(None)
                .expect("available_parallelism should succeed on test host");
            let n: usize = got.parse().expect("computed CARGO_BUILD_JOBS must parse");
            assert!(n >= 2);
        }

        // cas-4614 (GH #107): the divisor is count-aware, with the old
        // hardcoded assumption kept as a floor.
        #[test]
        fn worker_concurrency_divisor_treats_assumption_as_a_floor() {
            // A fleet smaller than the assumption must not raise the jobs
            // allocation — a lone worker claiming cores/1 is the storm the
            // derate exists to prevent.
            assert_eq!(
                worker_concurrency_divisor(Some(1)),
                DEFAULT_WORKER_CONCURRENCY_ASSUMPTION
            );
            assert_eq!(
                worker_concurrency_divisor(Some(DEFAULT_WORKER_CONCURRENCY_ASSUMPTION)),
                DEFAULT_WORKER_CONCURRENCY_ASSUMPTION
            );
            // Unknown count behaves exactly like today.
            assert_eq!(
                worker_concurrency_divisor(None),
                DEFAULT_WORKER_CONCURRENCY_ASSUMPTION
            );
            // A larger fleet derates further — this is the new behaviour.
            assert_eq!(worker_concurrency_divisor(Some(12)), 12);
        }

        #[test]
        fn cargo_build_jobs_derates_further_for_a_large_fleet() {
            let _e = ScopedEnv::new();
            let cores = std::thread::available_parallelism()
                .expect("available_parallelism should succeed on test host")
                .get();
            let floor = cargo_build_jobs_for_worker(Some(1))
                .and_then(|v| v.parse::<usize>().ok())
                .expect("computed value must parse");
            // Pick a fleet well above both the assumption and the core count
            // so the derate is forced onto the 2-job floor regardless of the
            // test host's topology.
            let huge = cargo_build_jobs_for_worker(Some(cores * 4))
                .and_then(|v| v.parse::<usize>().ok())
                .expect("computed value must parse");
            assert_eq!(huge, 2, "a fleet larger than the core count floors at 2");
            assert!(
                huge <= floor,
                "a larger fleet must never be granted more jobs than a small one: \
                 {huge} > {floor}"
            );
        }

        #[test]
        fn explicit_env_override_still_wins_over_the_fleet_count() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "7");
            }
            // Operator intent outranks the computed derate at every fleet size.
            assert_eq!(cargo_build_jobs_for_worker(Some(64)).as_deref(), Some("7"));
            assert_eq!(cargo_build_jobs_for_worker(None).as_deref(), Some("7"));
        }

        #[test]
        fn apply_worker_build_concurrency_replaces_rather_than_appends() {
            let _e = ScopedEnv::new();
            let mut config = PtyConfig::claude(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                Some("sup"),
                None,
                None,
                None,
                None,
            );
            // Simulate a stale duplicate from an older builder. On small CI
            // hosts the constructor's default can already be "2", so proving
            // replacement by comparing numeric values is not portable.
            config
                .env
                .push(("CARGO_BUILD_JOBS".to_string(), "99".to_string()));

            config.apply_worker_build_concurrency(Some(64));

            let entries: Vec<_> = config
                .env
                .iter()
                .filter(|(k, _)| k == "CARGO_BUILD_JOBS")
                .collect();
            assert_eq!(
                entries.len(),
                1,
                "a duplicate entry would make the effective value depend on \
                 CommandBuilder iteration order"
            );
            let after = entries[0].1.clone();
            assert_eq!(after, "2", "a 64-worker fleet floors at 2 jobs");
        }

        #[test]
        fn apply_worker_build_concurrency_is_noop_for_supervisor_config() {
            let _e = ScopedEnv::new();
            let mut config = PtyConfig::claude(
                "sup",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,
                None,
            );
            assert!(env_value(&config.env, "CARGO_BUILD_JOBS").is_none());
            // Must not invent the variable for a role that never had it —
            // supervisors are deliberately left uncapped.
            config.apply_worker_build_concurrency(Some(8));
            assert!(
                env_value(&config.env, "CARGO_BUILD_JOBS").is_none(),
                "supervisors must never gain CARGO_BUILD_JOBS"
            );
        }

        #[test]
        fn maybe_wrap_with_nice_is_noop_for_supervisor_role() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let (cmd, args) = maybe_wrap_with_nice(
                "claude",
                vec!["--session-id".to_string(), "abc".to_string()],
                "supervisor",
            );
            assert_eq!(cmd, "claude");
            assert_eq!(args, vec!["--session-id".to_string(), "abc".to_string()]);
        }

        #[test]
        fn maybe_wrap_with_nice_is_noop_without_env_sentinel() {
            let _e = ScopedEnv::new();
            // No CAS_FACTORY_NICE_WORKER set — passthrough for workers too.
            let (cmd, args) = maybe_wrap_with_nice("claude", vec!["--foo".to_string()], "worker");
            assert_eq!(cmd, "claude");
            assert_eq!(args, vec!["--foo".to_string()]);
        }

        #[test]
        fn maybe_wrap_with_nice_wraps_worker_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let (cmd, args) = maybe_wrap_with_nice(
                "claude",
                vec!["--session-id".to_string(), "xyz".to_string()],
                "worker",
            );
            assert_eq!(cmd, "nice");
            // Default level 10, original argv preserved after the wrapped command.
            assert_eq!(
                args,
                vec![
                    "-n".to_string(),
                    "10".to_string(),
                    "claude".to_string(),
                    "--session-id".to_string(),
                    "xyz".to_string(),
                ]
            );
        }

        #[test]
        fn maybe_wrap_with_nice_honours_level_override() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
                std::env::set_var("CAS_FACTORY_NICE_LEVEL", "15");
            }
            let (cmd, args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(cmd, "nice");
            assert_eq!(args[..2], ["-n".to_string(), "15".to_string()]);
            assert_eq!(args[2], "claude");
        }

        #[test]
        fn maybe_wrap_with_nice_rejects_non_1_sentinel_value() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "true"); // not "1"
            }
            let (cmd, _args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(
                cmd, "claude",
                "only the exact value '1' activates nice-wrap"
            );
        }

        #[test]
        fn claude_worker_gets_cargo_build_jobs_env() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::claude(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert!(
                config
                    .env
                    .iter()
                    .any(|(k, v)| k == "CARGO_BUILD_JOBS" && v == "4"),
                "worker PtyConfig must export CARGO_BUILD_JOBS when override is set"
            );
        }

        #[test]
        fn claude_supervisor_does_not_get_cargo_build_jobs_env() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::claude(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert!(
                !config.env.iter().any(|(k, _)| k == "CARGO_BUILD_JOBS"),
                "supervisor must NOT get CARGO_BUILD_JOBS cap — only workers do"
            );
        }

        // cas-7d8e: the quiet-network vars are worker-only so the supervisor
        // keeps feature-flag evaluation (Remote Control) and the auto-updater
        // (security/CVE patches). Workers stay pinned and silent.
        #[test]
        fn claude_worker_gets_quiet_network_env() {
            let _e = ScopedEnv::new();
            let config = PtyConfig::claude(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert!(
                config
                    .env
                    .iter()
                    .any(|(k, v)| k == "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" && v == "1"),
                "worker PtyConfig must keep CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1"
            );
            assert!(
                config
                    .env
                    .iter()
                    .any(|(k, v)| k == "DISABLE_AUTOUPDATER" && v == "1"),
                "worker PtyConfig must keep DISABLE_AUTOUPDATER=1 — a worker must not \
                 swap its binary mid-EPIC"
            );
        }

        #[test]
        fn claude_supervisor_does_not_get_quiet_network_env() {
            let _e = ScopedEnv::new();
            let config = PtyConfig::claude(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert!(
                !config
                    .env
                    .iter()
                    .any(|(k, _)| k == "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
                "supervisor must NOT get CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC — it \
                 disables the feature-flag evaluation Remote Control depends on"
            );
            assert!(
                !config.env.iter().any(|(k, _)| k == "DISABLE_AUTOUPDATER"),
                "supervisor must NOT get DISABLE_AUTOUPDATER — it would never receive \
                 security patches"
            );
        }

        // The unrelated quiet-UX vars are not part of the role gate: both roles
        // keep them, and the codex harness is untouched by cas-7d8e.
        #[test]
        fn claude_both_roles_keep_quiet_ux_env() {
            let _e = ScopedEnv::new();
            for role in ["worker", "supervisor"] {
                let config = PtyConfig::claude(
                    "a1",
                    role,
                    PathBuf::from("/tmp"),
                    None,
                    None,
                    None,
                    None,
                    None, // effort
                    None,
                );
                for (key, want) in [
                    ("DISABLE_COST_WARNINGS", "1"),
                    ("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1"),
                    ("IS_DEMO", "true"),
                ] {
                    assert!(
                        config.env.iter().any(|(k, v)| k == key && v == want),
                        "{role} must keep {key}={want}"
                    );
                }
            }
        }

        #[test]
        fn codex_keeps_quiet_network_env_for_both_roles() {
            let _e = ScopedEnv::new();
            for role in ["worker", "supervisor"] {
                let config = PtyConfig::codex(
                    "c1",
                    role,
                    PathBuf::from("/tmp"),
                    None,
                    None,
                    None,
                    None,
                    None, // effort
                    None,
                );
                assert!(
                    config
                        .env
                        .iter()
                        .any(|(k, v)| k == "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" && v == "1"),
                    "codex {role} env is out of scope for cas-7d8e and must be unchanged"
                );
                assert!(
                    config
                        .env
                        .iter()
                        .any(|(k, v)| k == "DISABLE_AUTOUPDATER" && v == "1"),
                    "codex {role} env is out of scope for cas-7d8e and must be unchanged"
                );
            }
        }

        #[test]
        fn claude_worker_command_wraps_in_nice_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::claude(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert_eq!(config.command, "nice");
            assert_eq!(config.args[0], "-n");
            assert_eq!(config.args[2], "claude");
        }

        #[test]
        fn cargo_build_jobs_case_insensitive_auto_falls_through() {
            // cas-0bf4 adversarial P2: a user who writes "Auto" or "AUTO"
            // in config must not leak the literal string into
            // CARGO_BUILD_JOBS (cargo would reject it as a non-integer
            // and silently defeat the cap).
            let _e = ScopedEnv::new();
            for variant in ["Auto", "AUTO", "auto", "  Auto  "] {
                unsafe {
                    std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", variant);
                }
                let got = cargo_build_jobs_for_worker(None)
                    .expect("available_parallelism should succeed on test host");
                let n: usize = got.parse().expect("computed value must parse as integer");
                assert!(
                    n >= 2,
                    "variant {variant:?} should fall through to auto-compute, got {got}"
                );
            }
        }

        #[test]
        fn maybe_wrap_with_nice_rejects_non_numeric_level() {
            // cas-0bf4 correctness P2: a non-numeric NICE_LEVEL must not
            // reach `nice -n <garbage>` — would fail every worker spawn.
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
                std::env::set_var("CAS_FACTORY_NICE_LEVEL", "high");
            }
            let (cmd, args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(cmd, "nice");
            assert_eq!(
                args[..2],
                ["-n".to_string(), "10".to_string()],
                "non-numeric NICE_LEVEL must fall back to default 10"
            );
        }

        #[test]
        fn maybe_wrap_with_nice_accepts_negative_numeric_level() {
            // Negative values parse as valid i32 and pass through; `nice`
            // itself rejects them for non-root, which is a separate OS
            // concern outside this helper. Documents the contract so a
            // future clamp-to-positive refactor is an explicit decision.
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
                std::env::set_var("CAS_FACTORY_NICE_LEVEL", "-5");
            }
            let (_cmd, args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(args[1], "-5");
        }

        #[test]
        fn codex_worker_gets_cargo_build_jobs_env() {
            // cas-0bf4 testing P1: codex spawn path must mirror claude.
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::codex(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert!(
                config
                    .env
                    .iter()
                    .any(|(k, v)| k == "CARGO_BUILD_JOBS" && v == "4"),
                "codex worker PtyConfig must export CARGO_BUILD_JOBS when override is set"
            );
        }

        #[test]
        fn codex_supervisor_does_not_get_cargo_build_jobs_env() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::codex(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert!(
                !config.env.iter().any(|(k, _)| k == "CARGO_BUILD_JOBS"),
                "codex supervisor must NOT get CARGO_BUILD_JOBS cap"
            );
        }

        #[test]
        fn codex_worker_command_wraps_in_nice_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::codex(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert_eq!(config.command, "nice");
            assert_eq!(config.args[0], "-n");
            assert_eq!(config.args[2], "codex");
        }

        #[test]
        fn nice_wrapped_codex_keeps_codex_submit_classification_cas_6e76() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::codex(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );

            assert!(command_launches_codex(&config.command, &config.args));
            assert!(
                !command_launches_codex("nice", &["-n".into(), "10".into(), "claude".into()]),
                "the wrapper test must not classify every niced harness as Codex"
            );
        }

        #[test]
        fn claude_supervisor_command_unwrapped_even_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::claude(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None, // effort
                None,
            );
            assert_eq!(
                config.command, "claude",
                "supervisor must not be niced — the whole point is it stays at nice 0"
            );
        }
    }

    // ---- cas-c931: turn-break keystroke characterization ----
    //
    // The urgent interrupt-and-redirect path breaks a worker's turn with Esc
    // (0x1b), NOT Ctrl+C (0x03). `Pty::interrupt` sends 0x03; the Esc payload
    // is sent at the Pane/Mux layer (`Pane::break_turn`). These tests lock the
    // byte values against a real PTY so we never regress the payload.

    const PTY_CONTROL_EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    const PTY_EVENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    async fn next_pty_event_until(
        pty: &mut Pty,
        deadline: tokio::time::Instant,
    ) -> Option<PtyEvent> {
        loop {
            if let Some(event) = pty.try_recv() {
                return Some(event);
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return None;
            }
            tokio::time::sleep((deadline - now).min(PTY_EVENT_POLL_INTERVAL)).await;
        }
    }

    /// Esc (0x1b) is NOT a signal-generating control char (unlike 0x03 = INTR),
    /// so it traverses the PTY rather than killing the child. We send Esc then a
    /// newline through `cat`: canonical-mode `cat` flushes the line and the
    /// content echoes back. The Esc surfaces either verbatim (0x1b) or as the
    /// line-discipline control rendering `^[` (0x5e 0x5b) depending on ECHOCTL.
    /// Either proves the exact `Pane::break_turn` payload reaches the child
    /// intact — and, crucially, that it does NOT terminate the process the way
    /// Ctrl+C does (the contrast locked by `interrupt_sends_ctrl_c...`).
    #[tokio::test]
    async fn esc_byte_reaches_pty_child_verbatim() {
        let config = PtyConfig {
            command: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        };
        let mut pty = match Pty::spawn("esc-probe", config) {
            Ok(p) => p,
            Err(_) => return, // `cat` unavailable in this environment — skip.
        };

        // Esc (the exact payload of Pane::break_turn) followed by newline so
        // canonical-mode cat flushes the line back to us.
        pty.write(&[0x1b]).await.expect("write esc");
        pty.write(b"\r").await.expect("write newline");
        // The PTY reader retains four trailing bytes while checking for a
        // cursor-position response. A second, ordinary line makes the
        // expected short echo observable without sleeping or closing cat.
        pty.write(b"flush\r").await.expect("write reader flush");

        // Poll events to a bounded deadline, returning immediately once the
        // expected echo arrives. Accept raw 0x1b OR the ECHOCTL rendering
        // "^[". Also assert the child stays ALIVE (no Exited event) — Esc must
        // not behave like Ctrl+C.
        let mut saw_esc = false;
        let mut exited = false;
        let deadline = tokio::time::Instant::now() + PTY_CONTROL_EVENT_TIMEOUT;
        while !saw_esc {
            match next_pty_event_until(&mut pty, deadline).await {
                Some(PtyEvent::Output(data)) => {
                    let rendered_caret = data.windows(2).any(|w| w == [0x5e, 0x5b]); // "^["
                    if data.contains(&0x1b) || rendered_caret {
                        saw_esc = true;
                    }
                }
                Some(PtyEvent::Exited(_)) | Some(PtyEvent::Error(_)) => {
                    exited = true;
                    break;
                }
                None => break, // absolute deadline elapsed
            }
        }
        pty.kill();
        assert!(
            !exited,
            "Esc (0x1b) must NOT terminate the child the way Ctrl+C does"
        );
        assert!(
            saw_esc,
            "Esc (0x1b) must reach the PTY child and echo back (verbatim or as ^[)"
        );
    }

    /// Lock the `Pty::interrupt` payload: it sends Ctrl+C (0x03), the quit
    /// signal — distinct from the Esc turn-break. We assert by behavior: 0x03
    /// is INTR in the default line discipline, so it terminates `cat`.
    #[tokio::test]
    async fn interrupt_sends_ctrl_c_and_terminates_cat() {
        let config = PtyConfig {
            command: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        };
        let mut pty = match Pty::spawn("intr-probe", config) {
            Ok(p) => p,
            Err(_) => return, // `cat` unavailable — skip.
        };

        pty.interrupt().await.expect("interrupt"); // writes 0x03

        // 0x03 = INTR → SIGINT → cat exits. Wait on events with the same
        // bounded ceiling as the Esc test: an event ends the test immediately.
        let deadline = tokio::time::Instant::now() + PTY_CONTROL_EVENT_TIMEOUT;
        let exited = loop {
            match next_pty_event_until(&mut pty, deadline).await {
                Some(PtyEvent::Exited(_)) | Some(PtyEvent::Error(_)) => {
                    break true;
                }
                Some(_) => {}
                None => {
                    break false; // no exit event before the deadline
                }
            }
        };
        pty.kill();
        assert!(
            exited,
            "Ctrl+C (0x03) from interrupt() must terminate cat (INTR signal)"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn kill_tree_force_reaps_the_direct_child_without_a_zombie() {
        let config = PtyConfig {
            command: "sleep".to_string(),
            args: vec!["120".to_string()],
            cwd: None,
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        };
        let mut pty = match Pty::spawn("zombie-reap-probe", config) {
            Ok(pty) => pty,
            Err(_) => return,
        };
        let pid = pty.process_group_id().expect("PTY child pid");
        pty.kill_tree(true).await;
        assert!(
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_err(),
            "force teardown must wait the direct child"
        );
    }

    // -------------------------------------------------------------------
    // cas-753a: PtyConfig::opencode
    // -------------------------------------------------------------------

    #[test]
    fn opencode_worker_launch_is_self_contained_and_model_driven() {
        let _e = ScopedEnv::new();
        let cwd = std::env::temp_dir().join("cas-opencode-worker");
        let config = PtyConfig::opencode(
            "open-worker",
            "worker",
            cwd.clone(),
            None,
            Some("factory-lead"),
            None,
            Some("local/qwen3.8"),
            Some("medium"),
            None,
        );

        assert_eq!(config.command, "opencode");
        assert_eq!(config.cwd, Some(cwd.clone()));
        assert_eq!(config.args[0], cwd.to_string_lossy());
        assert_eq!(
            &config.args[1..5],
            ["--model", "local/qwen3.8", "--agent", "cassy-worker"]
        );
        let prompt = config
            .args
            .windows(2)
            .find(|pair| pair[0] == "--prompt")
            .map(|pair| pair[1].as_str())
            .expect("worker launch must submit the startup workflow");
        assert!(prompt.contains("cas_coordination") && prompt.contains("cas_task"));
        assert_eq!(config.args.last().map(String::as_str), Some("--auto"));

        let env_value = |key: &str| {
            config
                .env
                .iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(env_value("PWD"), Some(cwd.to_string_lossy().as_ref()));
        assert_eq!(env_value("CAS_FACTORY_WORKER_CLI"), Some("opencode"));
        assert_eq!(env_value("CAS_SUPERVISOR_NAME"), Some("factory-lead"));
        assert_eq!(env_value("CAS_FACTORY_WORKER_MODEL"), Some("local/qwen3.8"));
        assert_eq!(env_value("CAS_FACTORY_WORKER_EFFORT"), Some("medium"));

        let inline: serde_json::Value = serde_json::from_str(
            env_value("OPENCODE_CONFIG_CONTENT").expect("inline OpenCode config must exist"),
        )
        .unwrap();
        assert!(
            inline["agent"]["cassy-worker"]["prompt"]
                .as_str()
                .is_some_and(|text| text.contains("cas_task"))
        );
        assert_eq!(inline["agent"]["cassy-worker"]["variant"], "medium");
        assert_eq!(inline["mcp"]["cas"]["type"], "local");
        assert_eq!(
            inline["mcp"]["cas"]["command"],
            serde_json::json!(["cas", "serve"])
        );
        assert_eq!(inline["mcp"]["cas"]["enabled"], true);
    }

    #[test]
    fn opencode_hosted_lanes_pin_endpoint_and_key_environment_without_values() {
        let _e = ScopedEnv::new();
        let cases = [
            (
                "qwencloud/qwen3.8-max",
                "QwenCloud Token Plan",
                OPENCODE_TOKEN_PLAN_ENDPOINT,
                "QWENCLOUD_TOKEN_PLAN_API_KEY",
                true,
            ),
            (
                "alibaba/qwen3.8-max",
                "QwenCloud Pay-as-you-go",
                OPENCODE_PAYG_ENDPOINT,
                "DASHSCOPE_API_KEY",
                false,
            ),
        ];
        for (model, name, endpoint, key_env, token_plan) in cases {
            let config = PtyConfig::opencode(
                "open-worker",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                Some(model),
                None,
                None,
            );
            let inline = config
                .env
                .iter()
                .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
                .map(|(_, value)| serde_json::from_str::<serde_json::Value>(value).unwrap())
                .unwrap();
            let provider = &inline["provider"][model.split_once('/').unwrap().0];
            assert_eq!(provider["name"], name);
            assert_eq!(provider["options"]["baseURL"], endpoint);
            assert_eq!(provider["options"]["apiKey"], format!("{{env:{key_env}}}"));
            assert_eq!(
                provider["models"]["qwen3.8-max"]["options"]["extra_body"]
                    .get("enable_thinking")
                    .and_then(serde_json::Value::as_bool),
                token_plan.then_some(true)
            );
            let encoded = serde_json::to_string(&inline).unwrap();
            assert!(
                !encoded.contains("sk-"),
                "inline config must contain no key value"
            );
        }
    }

    #[test]
    fn opencode_omits_unrequested_model_and_effort_without_a_default() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::opencode(
            "open-worker",
            "worker",
            std::env::temp_dir().join("cas-opencode-no-default"),
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert!(!config.args.iter().any(|arg| arg == "--model"));
        let inline = config
            .env
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
            .map(|(_, value)| serde_json::from_str::<serde_json::Value>(value).unwrap())
            .unwrap();
        assert!(inline["agent"]["cassy-worker"].get("model").is_none());
        assert!(inline["agent"]["cassy-worker"].get("variant").is_none());
    }

    #[test]
    fn opencode_resolves_relative_worktree_and_selects_supervisor_agent() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::opencode(
            "open-lead",
            "supervisor",
            PathBuf::from("relative-opencode-worktree"),
            None,
            None,
            Some("opencode"),
            Some("local/qwen3.8"),
            None,
            None,
        );

        let worktree = PathBuf::from(&config.args[0]);
        assert!(worktree.is_absolute());
        assert_eq!(config.cwd.as_ref(), Some(&worktree));
        assert!(
            config
                .args
                .windows(2)
                .any(|pair| { pair[0] == "--agent" && pair[1] == "cassy-supervisor" })
        );
        assert!(!config.args.iter().any(|arg| arg == "--prompt"));
        let inline = config
            .env
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
            .map(|(_, value)| serde_json::from_str::<serde_json::Value>(value).unwrap())
            .unwrap();
        assert!(
            inline["agent"]["cassy-supervisor"]["prompt"]
                .as_str()
                .is_some_and(|text| text.contains("cas_coordination"))
        );
    }

    // cas-6569 (EPIC cas-8888, Phase 2): PtyConfig::grok
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn test_pty_config_grok_basic() {
        // ScopedEnv clears CAS_FACTORY_NICE_WORKER etc. for the duration of
        // this test — this suite runs inside a real factory worker session
        // that sets CAS_FACTORY_NICE_WORKER=1 in its own ambient env, which
        // would otherwise wrap `config.command` in `nice` and break the
        // exact-match assertion below (same reason `test_pty_config_claude`
        // uses it).
        let _e = ScopedEnv::new();
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None, // model
            None, // effort
            None, // teams
        );
        assert_eq!(config.command, "grok");
        assert!(config.args.contains(&"--permission-mode".to_string()));
        assert!(config.args.contains(&"bypassPermissions".to_string()));
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_AGENT_NAME" && v == "test-agent")
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_AGENT_ROLE" && v == "worker")
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_FACTORY_MODE" && v == "1"),
            "worker verification-jail exemption depends on this"
        );
        // No CAS_ROOT when not provided.
        assert!(!config.env.iter().any(|(k, _)| k == "CAS_ROOT"));
    }

    #[tokio::test]
    async fn test_pty_config_grok_with_cas_root() {
        let cas_root = PathBuf::from("/home/user/project/.cas");
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            Some(&cas_root),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_ROOT" && v == "/home/user/project/.cas")
        );
    }

    #[tokio::test]
    async fn test_pty_config_grok_with_supervisor() {
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            Some("sup-1"),
            None,
            None,
            None,
            None,
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_SUPERVISOR_NAME" && v == "sup-1")
        );
    }

    /// EPIC cas-8888: Grok uses Claude's anti-overwrite session model — a
    /// fresh uuid, not Codex's name-prefixed style — because Phase 4's
    /// transcript resolver keys on this exact value.
    #[tokio::test]
    async fn test_pty_config_grok_injects_session_id_as_bare_uuid() {
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let session_id = config
            .env
            .iter()
            .find(|(k, _)| k == "CAS_SESSION_ID")
            .map(|(_, v)| v.clone())
            .expect("CAS_SESSION_ID must be set");
        assert!(
            uuid::Uuid::parse_str(&session_id).is_ok(),
            "grok session id must be a bare uuid (anti-overwrite model), got: {session_id}"
        );
        // The --session-id arg must carry the SAME value as the env var.
        let idx = config
            .args
            .iter()
            .position(|a| a == "--session-id")
            .expect("--session-id flag must be present");
        assert_eq!(config.args[idx + 1], session_id);
    }

    #[tokio::test]
    async fn test_pty_config_grok_with_model_and_effort() {
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            Some("grok-4.5"),
            Some("high"),
            None,
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("-m grok-4.5"),
            "expected -m <model>; got: {all_args}"
        );
        assert!(
            all_args.contains("--reasoning-effort high"),
            "expected --reasoning-effort <effort>; got: {all_args}"
        );
    }

    #[tokio::test]
    async fn test_pty_config_grok_without_model_or_effort_omits_flags() {
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(!config.args.contains(&"-m".to_string()));
        assert!(!config.args.contains(&"--reasoning-effort".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_grok_injects_cwd_flag() {
        let cwd = PathBuf::from("/home/user/myproject");
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            cwd.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let idx = config
            .args
            .iter()
            .position(|a| a == "--cwd")
            .expect("--cwd flag must be present");
        assert_eq!(config.args[idx + 1], cwd.to_string_lossy().to_string());
        // The actual process working directory must ALSO be set (the
        // `--cwd` flag and the spawned process's real cwd are independent
        // — both are set for correctness).
        assert_eq!(config.cwd, Some(cwd));
    }

    /// EPIC cas-8888 delta #2: Grok's SessionStart hook output is ignored,
    /// so the context bundle is delivered via `--rules` instead. Worker and
    /// supervisor get role-appropriate instructions.
    #[tokio::test]
    async fn test_pty_config_grok_worker_injects_rules_with_cas_prefix() {
        let config = PtyConfig::grok(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let idx = config
            .args
            .iter()
            .position(|a| a == "--rules")
            .expect("--rules flag must be present");
        let rules = &config.args[idx + 1];
        assert!(
            rules.contains("cas__task") && rules.contains("cas__coordination"),
            "grok worker rules must reference the cas__ tool prefix, not mcp__cas__/mcp__cs__: {rules}"
        );
        // No actual TOOL-CALL-shaped usage of Claude's/Codex's prefix (e.g.
        // "mcp__cas__task action=..." or "mcp__cs__task action=..."). The
        // text is allowed to mention those prefixes in passing (as a "not
        // this" clarification) — only real call-shaped occurrences would
        // be a bug.
        assert!(
            !rules.contains("mcp__cas__task") && !rules.contains("mcp__cs__task"),
            "grok rules must not use Claude's or Codex's tool-call syntax: {rules}"
        );
    }

    #[tokio::test]
    async fn test_pty_config_grok_supervisor_injects_rules_with_cas_prefix() {
        let config = PtyConfig::grok(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let idx = config
            .args
            .iter()
            .position(|a| a == "--rules")
            .expect("--rules flag must be present");
        let rules = &config.args[idx + 1];
        assert!(
            rules.contains("cas__coordination") && rules.contains("Supervisor"),
            "grok supervisor rules must reference the cas__ prefix and supervisor role: {rules}"
        );
        // cas-c145: Grok supervisors must get merge-queue-first lifecycle
        // guidance equivalent to other harnesses (no SessionStart bundle).
        assert!(
            rules.contains("MERGE REQUIRED")
                && rules.contains("awaiting_merge")
                && rules.contains("factory/<worker>")
                && rules.contains("do not poll"),
            "grok supervisor rules must surface AwaitingMerge merge-queue priority \
             (task/branch/next action, no polling): {rules}"
        );
        assert!(
            rules.contains("cas__task action=list status=awaiting_merge")
                || rules.contains("status=awaiting_merge"),
            "grok supervisor rules must name the awaiting_merge list command: {rules}"
        );
    }

    /// EPIC cas-8888 (cas-9a31, Phase 1 wiring): CAS_FACTORY_WORKER_CLI must
    /// be injected so downstream harness_policy resolution (cas__ tool
    /// prefix) works for a Grok worker, same as Claude/Codex.
    ///
    /// cas-921f P1 fix-round: this test originally passed `Some("grok")` for
    /// `factory_worker_cli` — a shape that never actually occurs on the real
    /// worker spawn path (`build_worker_config`, cas-mux/pane/mod.rs, always
    /// passes `None` there), so it gave false confidence: it proved the
    /// PASSTHROUGH worked, never that a real worker gets the env var at all.
    /// Fixed to pass `None`, matching the real call shape, so this test
    /// actually exercises the P1 bug (Phase 4 liveness was completely inert
    /// for real grok workers — no `CAS_FACTORY_WORKER_CLI` ⇒ `is-wedged`
    /// globs `~/.claude/projects/*` for a transcript that lives at
    /// `~/.grok/sessions/*` and always resolves `None`).
    #[tokio::test]
    async fn test_pty_config_grok_worker_injects_factory_worker_cli() {
        let config = PtyConfig::grok(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None, // factory_worker_cli — always None on the real worker spawn path
            None,
            None,
            None,
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_FACTORY_WORKER_CLI" && v == "grok"),
            "a grok process must set its own CAS_FACTORY_WORKER_CLI unconditionally — \
             it cannot rely on the factory_worker_cli param, which the worker spawn path \
             always leaves None: {:?}",
            config.env
        );
    }

    /// The supervisor path passes `factory_worker_cli = Some(<workers' cli>)`
    /// — a DIFFERENT semantic meaning of the same env var ("what CLI do MY
    /// WORKERS run", not "what CLI am I"). That value must still win over
    /// the unconditional grok-self push above; `apply_factory_worker_metadata`
    /// only ever reads this env var for worker-role agents anyway (cas-921f),
    /// so a supervisor never persists bogus `worker_cli` metadata from it —
    /// but pin the env-vec ordering contract directly too, since `Vec<(K,V)>`
    /// → `Command::env` applies last-write-wins per key.
    #[tokio::test]
    async fn test_pty_config_grok_supervisor_factory_worker_cli_overrides_self_grok() {
        let config = PtyConfig::grok(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            Some("codex"), // "my workers run codex"
            None,
            None,
            None,
        );
        let values: Vec<&str> = config
            .env
            .iter()
            .filter(|(k, _)| k == "CAS_FACTORY_WORKER_CLI")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(
            values.last().copied(),
            Some("codex"),
            "the last CAS_FACTORY_WORKER_CLI entry (which Command::env applies last, \
             winning) must be the supervisor's actual workers' cli, not grok's self-id: {values:?}"
        );
    }

    // -----------------------------------------------------------------------
    // cas-0263: canonical role-contract parity across all six launch shapes.
    // -----------------------------------------------------------------------

    #[test]
    fn parity_normalization_preserves_external_mcp_tool_names() {
        let parity_text = "Claude mcp__cas__task; Codex mcp__cs__task; Grok cas__task; \
            external mcp__viktor__ask_viktor mcp__viktor-shadow__ask_viktor \
            mcp__foreign__read_file";
        let normalized = normalize_tool_prefix(parity_text);

        assert!(normalized.contains("tool__task"));
        assert!(
            normalized.contains("mcp__viktor__ask_viktor"),
            "the explicit Viktor tool must not be normalized as CAS"
        );
        assert!(
            normalized.contains("mcp__viktor-shadow__ask_viktor"),
            "a lookalike external server must not be normalized as CAS"
        );
        assert!(
            normalized.contains("mcp__foreign__read_file"),
            "a foreign external tool must not be normalized as CAS"
        );
    }

    /// AC-1/AC-2: every one of the six launch shapes (Claude/Codex/Grok ×
    /// supervisor/worker) carries the full canonical role contract — coordinate-
    /// only / one-task, async message handling, task lifecycle, merge/re-close,
    /// and safe urgent-stop recovery — on the surface each runtime consumes.
    #[test]
    fn test_all_six_launch_shapes_carry_full_role_contract() {
        let shapes = [
            ("claude", ContractRole::Supervisor),
            ("claude", ContractRole::Worker),
            ("codex", ContractRole::Supervisor),
            ("codex", ContractRole::Worker),
            ("grok", ContractRole::Supervisor),
            ("grok", ContractRole::Worker),
        ];
        for (harness, role) in shapes {
            let surface = rendered_contract_surface(harness, role);
            let missing = missing_contract_elements(&surface, role);
            assert!(
                missing.is_empty(),
                "launch shape {harness}/{role:?} is missing canonical contract \
                 element(s) {missing:?} — all six shapes must carry one \
                 semantically equivalent CAS role contract"
            );
        }
    }

    /// AC-5: normalized parity allows only the intentional tool-prefix
    /// difference — each harness's surface issues tool CALLS with ITS prefix and
    /// never a foreign one. Checked on the tool-CALL form (`<prefix>task` /
    /// `<prefix>coordination`), not the bare prefix, because a surface may
    /// legitimately NAME the other prefixes in "not mcp__cas__ or mcp__cs__"
    /// negative guidance (Grok/Codex do this).
    #[test]
    fn test_launch_shapes_use_harness_correct_tool_prefix() {
        let claude_calls = ["mcp__cas__task", "mcp__cas__coordination"];
        let codex_calls = ["mcp__cs__task", "mcp__cs__coordination"];
        let grok_calls = ["cas__task", "cas__coordination"];
        for role in [ContractRole::Supervisor, ContractRole::Worker] {
            let c = rendered_contract_surface("claude", role);
            assert!(
                claude_calls.iter().any(|m| c.contains(m)),
                "claude/{role:?} must issue mcp__cas__ tool calls"
            );
            for bad in codex_calls {
                assert!(!c.contains(bad), "claude/{role:?} leaks Codex call {bad}");
            }
            let x = rendered_contract_surface("codex", role);
            assert!(
                codex_calls.iter().any(|m| x.contains(m)),
                "codex/{role:?} must issue mcp__cs__ tool calls"
            );
            for bad in claude_calls {
                assert!(!x.contains(bad), "codex/{role:?} leaks Claude call {bad}");
            }
            let g = rendered_contract_surface("grok", role);
            assert!(
                grok_calls.iter().any(|m| g.contains(m)),
                "grok/{role:?} must issue cas__ tool calls"
            );
            for bad in claude_calls.iter().chain(codex_calls.iter()) {
                assert!(!g.contains(bad), "grok/{role:?} leaks wrapped call {bad}");
            }
        }
    }

    /// AC-3 (AGENTS.md decision): the Codex role contract is supplied entirely by
    /// the launch instruction string injected via `--config
    /// developer_instructions` — NO AGENTS.md file is written or required. The
    /// launched Codex process is not observed to auto-discover a cwd AGENTS.md
    /// for the CAS contract, so CAS delivers the contract inline at launch. This
    /// regression proves that inline delivery is complete, documenting why no
    /// file is managed.
    #[test]
    fn test_codex_contract_supplied_by_launch_injection_no_agents_md() {
        for role in [ContractRole::Supervisor, ContractRole::Worker] {
            let surface = rendered_contract_surface("codex", role);
            assert!(
                missing_contract_elements(&surface, role).is_empty(),
                "Codex {role:?} contract must be fully supplied by the launch \
                 developer_instructions string (no AGENTS.md fallback needed)"
            );
        }
        let cfg = PtyConfig::codex(
            "w1",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(
            cfg.args
                .iter()
                .any(|a| a.contains("developer_instructions")),
            "Codex launch argv must carry developer_instructions (the contract surface)"
        );
        assert!(
            !cfg.args.iter().any(|a| a.contains("AGENTS.md")),
            "Codex launch must not reference an AGENTS.md file — contract is inline"
        );
    }
}
