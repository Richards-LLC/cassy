//! Turn-start inbox surfacing for factory agents (cas-7a01, GH #155).
//!
//! # Why this file exists
//!
//! Cassy had a delivery path and no surfacing path. The daemon wrote a message
//! into a Claude teammate's inbox file, stamped the queue row
//! `stage=delivered`, and stopped — nothing anywhere read that queue back and
//! put it in front of the recipient. Every hook handler in `hooks/` touched the
//! prompt queue exactly once, and that one touch was an *enqueue*
//! (`handlers_events/pre_tool.rs`, the `SendMessage` auto-route). "Delivery ≠
//! surfacing" was not a race or a filter: the surfacing half was never built.
//!
//! The consequences were reported three times (GH #130, #139, #155): a
//! non-urgent message to an idle Claude worker could sit unread across turns
//! the worker took, while `message_status` reported `delivered` and
//! `wake: unobserved`. Only urgent messages ever arrived, because the urgent
//! path is structurally different — an unconditional PTY interrupt that
//! bypasses the inbox entirely.
//!
//! # What this does
//!
//! On `UserPromptSubmit`, a factory agent's unread queue rows are drained and
//! injected into the turn that is starting. The drain writes its surfacing
//! receipt in the same transaction it selects the rows, so:
//!
//! - A row that reaches the model is always receipted (never re-injected —
//!   that is the GH #124 storm, cas-ceae).
//! - A row whose receipt failed to persist was never returned, so it stays
//!   unread and is retried at the next turn start (never silently dropped —
//!   that is GH #139).
//!
//! There is no third state. The storm guard and the silent-drop guard are the
//! same invariant read from two directions, which is why the atomicity lives
//! in the store rather than in a policy here.
//!
//! # What this deliberately does not do
//!
//! It does not consume an inbox *drain*: `inbox_poll` remains non-consuming
//! exactly as cas-ef14 left it. Only an actual injection into a turn marks a
//! row seen.

use std::path::Path;

use cas_core::hooks::types::HookInput;
use cas_store::{PromptQueueStore, QueuedPrompt};

fn assignment_is_stale_for_recipient(
    queue: &dyn PromptQueueStore,
    task_store: &dyn crate::store::TaskStore,
    row: &QueuedPrompt,
    recipient: &str,
) -> bool {
    let Some(task_id) = crate::prompt_revalidation::assignment_solicited_task_id(&row.prompt) else {
        return false;
    };
    // Recovery bodies quote historical work; only the store's authenticated
    // row predicate can exempt them from a fresh assignment's stale check.
    match queue.is_abandonment_recovery(row.id) {
        Ok(true) => return false,
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(prompt_id = row.id, %error, "could not classify abandonment recovery at hook surface");
        }
    }
    let Ok(task) = task_store.get(&task_id) else {
        return false;
    };
    crate::prompt_revalidation::assignment_stale_task(
        &row.prompt,
        task.status,
        task.assignee.as_deref(),
        recipient,
    )
    .is_some()
}

/// Rows surfaced into a single turn.
///
/// Bounded because the injection lands in the model's context window: an agent
/// returning from a long absence with a large backlog must still get a usable
/// turn. Anything beyond this stays unread and surfaces at the next turn — it
/// is not dropped.
const SURFACE_LIMIT: usize = 10;

/// cas-bf890 (GH #1215 §3): digest notices one turn may fold into its single
/// digest row. They cost one line, not a body, so the bound is generous;
/// anything past it folds into the next turn's row.
const DIGEST_LIMIT: usize = 50;

/// cas-bf890: notices the digest row names one by one before `+N more`.
const DIGEST_NAMED: usize = 6;

/// cas-bf890: the lane a surfaced row renders in. Peer-supervisor and
/// operator traffic leads the block with its own label and is never folded;
/// everything else keeps its delivery order behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum InboxLane {
    Operator,
    PeerSupervisor,
    Ordinary,
}

impl InboxLane {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Operator => "[operator] ",
            Self::PeerSupervisor => "[peer supervisor] ",
            Self::Ordinary => "",
        }
    }
}

/// cas-bf890: classify `row` by its server-stamped origin, never by its
/// caller-settable `source`: a paired Commander device is the operator; a
/// Cloud peer-mailbox envelope CAS itself wrote, or a registered agent whose
/// registry role is supervisor (another one: `own_names` excludes an echo),
/// is a peer supervisor. The same stamps the daemon's wake gate reads
/// (`FactoryDaemon::supervisor_wake_class`), so the label cannot be forged by
/// spelling a name.
pub(crate) fn inbox_lane(
    row: &QueuedPrompt,
    supervisor_ids: &std::collections::HashMap<String, String>,
    own_names: &[String],
) -> InboxLane {
    match row.origin.as_ref() {
        Some(cas_store::QueueOrigin::PairedDevice { .. }) => InboxLane::Operator,
        Some(cas_store::QueueOrigin::Daemon)
            if crate::cloud::peer_mailbox::parse_envelope(&row.prompt).is_some() =>
        {
            InboxLane::PeerSupervisor
        }
        Some(cas_store::QueueOrigin::RegisteredAgent { agent_id }) => supervisor_ids
            .get(agent_id)
            .filter(|name| !own_names.iter().any(|own| own.eq_ignore_ascii_case(name)))
            .map_or(InboxLane::Ordinary, |_| InboxLane::PeerSupervisor),
        _ => InboxLane::Ordinary,
    }
}

/// cas-bf890: registry id to name for every supervisor among the senders of
/// `rows`; empty when no row came from a registered agent.
fn supervisor_senders(
    cas_root: &Path,
    rows: &[QueuedPrompt],
) -> std::collections::HashMap<String, String> {
    let mut found = std::collections::HashMap::new();
    let ids: Vec<&str> = rows
        .iter()
        .filter_map(|row| match row.origin.as_ref() {
            Some(cas_store::QueueOrigin::RegisteredAgent { agent_id }) => Some(agent_id.as_str()),
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        return found;
    }
    let Ok(agents) = crate::store::open_agent_store(cas_root) else {
        return found;
    };
    for id in ids {
        if let Ok(agent) = agents.get(id)
            && matches!(agent.role, crate::types::AgentRole::Supervisor)
        {
            found.insert(id.to_string(), agent.name.clone());
        }
    }
    found
}

/// cas-bf890: the one digest row that stands for `notices`. It names a few by
/// id and summary and says how to read any of them whole.
fn digest_row(notices: &[QueuedPrompt]) -> Option<String> {
    if notices.is_empty() {
        return None;
    }
    let named: Vec<String> = notices
        .iter()
        .take(DIGEST_NAMED)
        .map(|row| {
            let summary = row
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|summary| !summary.is_empty())
                .unwrap_or("system notice");
            format!("#{} {summary}", row.id)
        })
        .collect();
    let more = notices.len().saturating_sub(DIGEST_NAMED);
    Some(format!(
        "[digest] {} inbox update{}: {}{}. Read one whole with \
         inbox_poll notification_id=<id>.",
        notices.len(),
        if notices.len() == 1 { "" } else { "s" },
        named.join("; "),
        if more > 0 { format!("; +{more} more") } else { String::new() },
    ))
}

/// Resolve the queue recipient names this agent answers to.
///
/// A worker answers to exactly one name. A supervisor answers to two: its pane
/// name and the logical `"supervisor"` alias that the whole factory addresses
/// it by (the daemon resolves `target == "supervisor"` to the pane at delivery
/// time). Surfacing only the pane name would leave every supervisor-addressed
/// message — which is nearly all of them — invisible.
fn recipient_aliases(input: &HookInput) -> Vec<String> {
    // cas-3bf1 (GH #176): delegated to the shared resolver so this hook and the
    // MCP `inbox_poll` can never again disagree about who a supervisor is.
    crate::harness_policy::inbox_aliases(
        &std::env::var("CAS_AGENT_NAME").unwrap_or_default(),
        crate::harness_policy::is_supervisor(input),
    )
}

/// Factory environment is inherited by children. Only the parent owns its
/// inbox and turn-context receipt; child hook payloads identify themselves.
///
/// cas-7bfa (GH #1209): a Claude Agent Teams member is launched with
/// `--agent-id <name>@<team>` (`supervisor@<team>` for the supervisor; see
/// `teams.rs`), and Claude Code puts that `agent_id`, plus its `agent_type`,
/// in EVERY hook payload. Treating any `agent_id` as a child made every Teams
/// worker and supervisor a "subagent" to its own hooks (Claude Code 2.1.300's
/// hook-input builder sets `agent_id: ctx?.agentId, agent_type: ctx?.agentType
/// ?? mainThreadAgentType`). Tool-boundary and
/// turn-start mail surfacing then never ran: 373 of 373 PostToolUse hooks
/// were empty in one worker session, and a message stayed gated for the whole
/// turn. A subagent's id is a bare hex string with no `@`; only the
/// session's own teammate id counts as the parent.
pub(crate) fn owns_factory_inbox(input: &HookInput) -> bool {
    // A child always arrives with its own `agent_id` (a hex id, alongside its
    // `agent_type`). An `agent_type` on its own is the main thread's type:
    // Claude Code fills it from `--agent-type` (`general-purpose`,
    // `team-lead`, `director`) for every hook of a Teams member.
    let is_child = input
        .agent_id
        .as_deref()
        .is_some_and(|agent_id| !is_own_teammate_id(input, agent_id));
    crate::harness_policy::is_factory_agent(input)
        && !is_child
        && input.subagent_type.is_none()
        && !input.transcript_path.as_deref().is_some_and(|path| {
            Path::new(path).components().any(|part| part.as_os_str() == "subagents")
        })
}

/// Whether `agent_id` is this session's own Agent Teams identity:
/// `<CAS_AGENT_NAME>@<team>`, or `supervisor@<team>` for a supervisor, where
/// the team is the factory session when one is set.
fn is_own_teammate_id(input: &HookInput, agent_id: &str) -> bool {
    let Some((local, team)) = agent_id.split_once('@') else {
        return false;
    };
    if let Some(session) = factory_session()
        && !team.eq_ignore_ascii_case(&session)
    {
        return false;
    }
    let name = std::env::var("CAS_AGENT_NAME").unwrap_or_default();
    (!name.is_empty() && local.eq_ignore_ascii_case(name.trim()))
        || (crate::harness_policy::is_supervisor(input)
            && local.eq_ignore_ascii_case(crate::harness_policy::SUPERVISOR_ALIAS))
}

/// cas-b5e4 (GH #989): the newest inbox stamp (epoch milliseconds) across
/// every recipient name this agent answers to, read without opening a store.
/// `None` when nothing was ever enqueued for it.
pub(crate) fn inbox_signal_stamp(cas_root: &Path, input: &HookInput) -> Option<i64> {
    if !owns_factory_inbox(input) {
        return None;
    }
    let mut names = recipient_aliases(input);
    names.push("all_workers".to_string());
    names
        .iter()
        .filter_map(|name| cas_store::read_inbox_signal(cas_root, name))
        .max()
}

fn factory_session() -> Option<String> {
    std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Drain and render this agent's unread mail for the turn that is starting.
///
/// `None` when there is nothing to surface, when Cassy is not initialised, or
/// when the agent has no resolvable factory identity. Every failure is silent
/// by design: a hook that errors is a hook the harness may disable, and losing
/// prompt capture to a queue problem would be a worse bug than the one this
/// fixes.
///
/// cas-1f0c: the rendered bodies fit [`cas_store::HOOK_SURFACE_BUDGET_CHARS`];
/// a row that does not fit stays unread and the block says so.
pub fn surface_factory_inbox(cas_root: Option<&Path>, input: &HookInput) -> Option<String> {
    let mut budget = cas_store::HOOK_SURFACE_BUDGET_CHARS;
    surface_factory_inbox_with_transport_delivery(
        cas_root,
        input,
        Surfacing::TurnStart,
        &mut budget,
    )
    .and_then(|outcome| outcome.text)
}

/// The turn a tool-boundary surfacing runs inside, read from the transcript.
pub(crate) struct CurrentTurn<'a> {
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub prompt: &'a str,
}

/// Surfacing at a tool boundary, inside a turn (cas-b5e4, GH #989).
///
/// The transport path may already have injected a queue row as the prompt
/// that started this turn, so rows handed off before the turn began are
/// excluded (cas-9568). A row handed off after the turn began reached a busy
/// recipient mid-turn and is surfaced now, within one tool call, unless its
/// body is the turn's own prompt. Without a known turn only unmarked rows are
/// eligible. Rendered bodies spend `budget` (cas-1f0c). Keep the completion
/// result so an overflow or failed drain is retried at the next boundary.
pub(crate) fn surface_factory_inbox_after_tool_result(
    cas_root: Option<&Path>,
    input: &HookInput,
    turn: Option<CurrentTurn<'_>>,
    budget: &mut usize,
) -> Option<SurfacingOutcome> {
    surface_factory_inbox_with_transport_delivery(
        cas_root,
        input,
        Surfacing::ToolBoundary(turn),
        budget,
    )
}

/// cas-1f0c: what one hook's additional context may hold in total. Claude
/// Code keeps hook context whole only up to about 10,000 characters and shows
/// a short preview past that. Mail is bounded by
/// [`cas_store::HOOK_SURFACE_BUDGET_CHARS`] plus its frames and notes; the
/// rest (supervisor reminder, ambient recall) must fit what is left.
pub(crate) const HOOK_CONTEXT_CAP_CHARS: usize = 9_500;

/// cas-1f0c: what a hook's mail block may spend beyond its bodies: the
/// header and the notes (already shown, withdrawn, not shown, sent in full).
pub(crate) const HOOK_MAIL_NOTES_RESERVE_CHARS: usize = 1_500;

/// cas-1f0c: the body budget for a hook whose output already carries
/// `reserved` characters of other context (PostToolUse lessons and
/// guardrail warnings), so bodies, notes and that context stay within
/// [`HOOK_CONTEXT_CAP_CHARS`] together.
pub(crate) fn hook_body_budget(reserved: usize) -> usize {
    cas_store::HOOK_SURFACE_BUDGET_CHARS.min(
        HOOK_CONTEXT_CAP_CHARS.saturating_sub(reserved + HOOK_MAIL_NOTES_RESERVE_CHARS),
    )
}

/// cas-1f0c: append `addition` to a hook's context only if the whole, plus
/// `reserved` characters of other context in the same output, stays within
/// [`HOOK_CONTEXT_CAP_CHARS`]. Returns whether it was appended.
pub(crate) fn push_within_hook_cap(context: &mut String, addition: &str, reserved: usize) -> bool {
    let separator = if context.is_empty() { 0 } else { 2 };
    if reserved + context.chars().count() + separator + addition.chars().count()
        > HOOK_CONTEXT_CAP_CHARS
    {
        return false;
    }
    if separator > 0 {
        context.push_str("\n\n");
    }
    context.push_str(addition);
    true
}

/// cas-1f0c: build optional context after mail was already receipted. A
/// panic here must not take the rendered mail down with the hook process.
pub(crate) fn after_mail<T>(build: impl FnOnce() -> Option<T>) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(build))
        .ok()
        .flatten()
}

/// cas-1f0c: what one hook surfacing did.
pub(crate) struct SurfacingOutcome {
    /// The `[incoming messages]` block, if anything was shown or noted.
    pub text: Option<String>,
    /// Every row the surfacing selected was resolved: shown, withdrawn, or
    /// too long for any hook (a Claude Teams copy then carries the body).
    /// `false` when a drain failed or a row only lacked room this time, so a
    /// later tool boundary retries.
    pub complete: bool,
}

/// cas-1f0c: surface, at a tool boundary, the rows this turn's own prompt
/// names with a `CAS wake: message N` pointer.
///
/// A Claude Teams inbox carries a pointer instead of the body, and the
/// teammate-message turn it starts fires no UserPromptSubmit, so the body
/// would otherwise reach the agent only by its own `inbox_poll`. The same
/// path recovers a pointer-woken turn whose turn-start hook failed or ran out
/// of room. Rows already shown are not drained again (the store's receipt
/// decides), so this never duplicates a body.
///
/// `first_boundary` is true at the first tool boundary of a turn no
/// UserPromptSubmit served. Only then does the block carry the turn-start
/// notes (already shown, withdrawn, did not fit), which no hook has given
/// yet; an `[already shown]` note can also come from a `[cas #N …]` envelope.
///
/// cas-6ccb: `polled_before` also shows, in the same block, rows read with
/// `inbox_poll` before that turn start and never acknowledged, so one row
/// gets one note (the repeat), not also "[already shown]".
pub(crate) fn surface_turn_prompt_rows(
    cas_root: &Path,
    input: &HookInput,
    prompt: &str,
    first_boundary: bool,
    polled_before: Option<chrono::DateTime<chrono::Utc>>,
    budget: &mut usize,
) -> Option<SurfacingOutcome> {
    if !names_wake_pointer(prompt) && !(first_boundary && !envelope_ids(prompt).is_empty()) {
        return None;
    }
    surface_factory_inbox_with_transport_delivery(
        Some(cas_root),
        input,
        Surfacing::TurnPrompt {
            prompt,
            notes: first_boundary,
            polled_before,
        },
        budget,
    )
}

/// cas-1f0c: whether `prompt` names any row by a pointer wake. Reads no store.
pub(crate) fn names_wake_pointer(prompt: &str) -> bool {
    !wake_named_ids(prompt).is_empty()
}

enum Surfacing<'a> {
    TurnStart,
    ToolBoundary(Option<CurrentTurn<'a>>),
    /// cas-1f0c: a tool boundary handling the rows its turn's prompt names.
    TurnPrompt {
        prompt: &'a str,
        notes: bool,
        polled_before: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// cas-6ccb: once per turn, at a tool boundary, rows polled before the
    /// turn began and never acknowledged (independent of new mail).
    PolledUnacked(chrono::DateTime<chrono::Utc>),
}

/// cas-6ccb (GH #1215): once per turn, at its first tool boundary, show the
/// direct rows this agent read with `inbox_poll` before the turn began and
/// never acknowledged. A turn-start hook already does this for typed turns;
/// a teammate-message turn has none, and new mail need not have arrived.
pub(crate) fn surface_polled_unacked(
    cas_root: &Path,
    input: &HookInput,
    turn_started_at: chrono::DateTime<chrono::Utc>,
    budget: &mut usize,
) -> Option<SurfacingOutcome> {
    surface_factory_inbox_with_transport_delivery(
        Some(cas_root),
        input,
        Surfacing::PolledUnacked(turn_started_at),
        budget,
    )
}

const POLLED_REPEAT_HEADER: &str = "The following message(s) are a one-time repeat of \
     earlier inbox_poll reads you did not acknowledge.";
const TURN_START_HEADER: &str = "The following message(s) arrived while you were not in a turn. \
     They are delivered here once — act on them now.";
const TOOL_BOUNDARY_HEADER: &str = "The following message(s) arrived while you were working. \
     They are delivered here once — read them before your next step.";
const TURN_PROMPT_HEADER: &str = "The following message(s) are the ones this turn's CAS wake \
     names. They are delivered here once — act on them now.";

fn surface_factory_inbox_with_transport_delivery(
    cas_root: Option<&Path>,
    input: &HookInput,
    surfacing: Surfacing<'_>,
    budget: &mut usize,
) -> Option<SurfacingOutcome> {
    if !owns_factory_inbox(input) {
        return None;
    }
    let cas_root = cas_root?;
    let aliases = recipient_aliases(input);
    if aliases.is_empty() {
        return None;
    }
    let session = factory_session();
    let queue = crate::store::open_prompt_queue_store(cas_root).ok()?;
    let task_store = crate::store::open_task_store_local(cas_root).ok();

    let turn_prompt = match &surfacing {
        Surfacing::TurnStart => Some(input.submitted_prompt().unwrap_or_default()),
        Surfacing::TurnPrompt { prompt, .. } => Some(*prompt),
        Surfacing::ToolBoundary(_) | Surfacing::PolledUnacked(_) => None,
    };
    let wake_ids = turn_prompt.map(wake_named_ids).unwrap_or_default();
    // Turn-start notes go to the first hook that serves a turn: UserPromptSubmit,
    // or the first tool boundary of a teammate turn that fired none.
    let notes = matches!(
        surfacing,
        Surfacing::TurnStart | Surfacing::TurnPrompt { notes: true, .. }
    );
    // cas-7ce4: read before this turn's drain, so only an earlier surfacing
    // counts as "already shown".
    let already_shown = if notes {
        already_surfaced_envelopes(&*queue, turn_prompt.unwrap_or_default(), &aliases)
    } else {
        Vec::new()
    };
    let mut rows: Vec<QueuedPrompt> = Vec::new();
    let mut over_budget: Vec<(i64, usize)> = Vec::new();
    let mut withheld: Vec<i64> = Vec::new();
    let mut polled_unacked: Vec<i64> = Vec::new();
    // cas-6ccb: a turn-start hook runs as its turn begins.
    let turn_started = chrono::Utc::now();
    let mut drain_failed = false;
    for alias in &aliases {
        let drains = match &surfacing {
            // cas-ad92: a pointer wake names the row whose body the harness
            // holds back until the next turn boundary; surface it into the
            // wake turn first, over the daemon's transport claim.
            // cas-6ccb (GH #1215): a row read with inbox_poll in an earlier
            // turn and never acknowledged is shown once more.
            Surfacing::TurnStart => vec![
                cas_store::HookDrain::WakeNamed(&wake_ids),
                cas_store::HookDrain::Unseen,
                cas_store::HookDrain::PolledBefore(turn_started),
            ],
            Surfacing::ToolBoundary(Some(turn)) => {
                vec![cas_store::HookDrain::DeliveredAfter(turn.started_at)]
            }
            Surfacing::PolledUnacked(at) => vec![cas_store::HookDrain::PolledBefore(*at)],
            Surfacing::ToolBoundary(None) => vec![cas_store::HookDrain::WithoutTransport],
            Surfacing::TurnPrompt { polled_before, .. } => {
                let mut drains = vec![cas_store::HookDrain::WakeNamed(&wake_ids)];
                drains.extend(polled_before.map(cas_store::HookDrain::PolledBefore));
                drains
            }
        };
        for drain in drains {
            let repeat_of_poll = matches!(drain, cas_store::HookDrain::PolledBefore(_));
            let remaining = SURFACE_LIMIT.saturating_sub(rows.len());
            if remaining == 0 {
                break;
            }
            let found = match queue.surface_for_recipient_within_budget(
                alias,
                session.as_deref(),
                drain,
                remaining,
                *budget,
            ) {
                Ok(found) => found,
                Err(error) => {
                    drain_failed = true;
                    tracing::debug!(
                        target: "cas::coordination",
                        recipient = %alias,
                        %error,
                        "cas-7a01: turn-start inbox surfacing failed"
                    );
                    continue;
                }
            };
            over_budget.extend(found.over_budget);
            for row in found.rows {
                // Transport-delivered rows remain eligible for this hook
                // so a wake the harness dropped can recover. Revalidate
                // assignment/start boilerplate before rendering it: the
                // task may have moved beyond Open while the row waited.
                if task_store.as_ref().is_some_and(|store| {
                    assignment_is_stale_for_recipient(queue.as_ref(), store.as_ref(), &row, alias)
                }) {
                    tracing::info!(
                        target: "cas::coordination",
                        stage = "suppress_stale_assignment_at_hook_surface",
                        prompt_id = row.id,
                        recipient = %alias,
                        "cas-7c1a: withheld stale assignment boilerplate from turn-start surfacing"
                    );
                    withheld.push(row.id);
                    continue;
                }
                // A supervisor's two aliases are two distinct recipient
                // keys in the receipt table, so a broadcast row can come
                // back from both. Injecting it twice into one turn is the
                // duplicate this task must not create.
                if rows.iter().any(|existing| existing.id == row.id) {
                    continue;
                }
                // A row injected as this turn's own prompt is already in
                // front of the model; its receipt is now correct, and
                // rendering it again would duplicate it.
                if let Surfacing::ToolBoundary(Some(turn)) = &surfacing
                    && !row.prompt.trim().is_empty()
                    && turn.prompt.contains(row.prompt.trim())
                {
                    continue;
                }
                *budget = budget.saturating_sub(cas_store::hook_surface_cost(&row.prompt));
                if repeat_of_poll {
                    polled_unacked.push(row.id);
                }
                rows.push(row);
            }
        }
    }
    // cas-bf890: the first surfacing of a turn folds every unread digest
    // notice into one row. The daemon never transports these, so this is
    // their only automatic path.
    let mut digest: Vec<QueuedPrompt> = Vec::new();
    if notes {
        for alias in &aliases {
            match queue.surface_for_recipient_within_budget(
                alias,
                session.as_deref(),
                cas_store::HookDrain::NoticeDigest,
                DIGEST_LIMIT.saturating_sub(digest.len()),
                0,
            ) {
                Ok(found) => {
                    for row in found.rows {
                        if !digest.iter().any(|existing| existing.id == row.id) {
                            digest.push(row);
                        }
                    }
                }
                Err(error) => {
                    drain_failed = true;
                    tracing::debug!(
                        target: "cas::coordination",
                        recipient = %alias,
                        %error,
                        "cas-bf890: digest notice surfacing failed"
                    );
                }
            }
        }
        if !digest.is_empty() {
            crate::harness_policy::mirror_receipts_across_aliases_with_source(
                &*queue,
                &digest,
                &aliases,
                cas_store::SurfacingSource::HookSurfaced,
            );
        }
    }
    over_budget.sort_unstable();
    over_budget.dedup_by_key(|(id, _)| *id);
    over_budget.retain(|(id, _)| !rows.iter().any(|row| row.id == *id));
    // A row that would fit an empty block only lacked room this time.
    let complete = !drain_failed
        && over_budget
            .iter()
            .all(|(_, cost)| *cost > cas_store::HOOK_SURFACE_BUDGET_CHARS);

    let mut block_notes: Vec<String> = Vec::new();
    block_notes.extend(polled_unacked_note(&polled_unacked));
    if notes {
        // cas-6ccb: a row shown in full again gets the [not acknowledged]
        // note, not also "[already shown]".
        let already_shown: Vec<i64> = already_shown
            .iter()
            .copied()
            .filter(|id| !rows.iter().any(|row| row.id == *id))
            .collect();
        block_notes.extend(already_shown_note(&already_shown));
        // cas-1f0c: the wake promised this turn the body or a reason.
        let withdrawn: Vec<i64> = withheld
            .iter()
            .copied()
            .filter(|id| wake_ids.contains(id))
            .collect();
        block_notes.extend(withdrawn_note(&withdrawn));
        block_notes.extend(over_budget_note(&over_budget, &*queue));
        // cas-1f0c: a named row whose full body was re-sent as a Teams copy
        // is not rendered here (the copy carries it).
        let sent_in_full: Vec<i64> = wake_ids
            .iter()
            .copied()
            .filter(|id| {
                !rows.iter().any(|row| row.id == *id)
                    && !already_shown.contains(id)
                    && aliases.iter().any(|alias| {
                        queue
                            .teams_body_fallback_claimed(*id, alias)
                            .unwrap_or(false)
                    })
            })
            .collect();
        block_notes.extend(sent_in_full_note(&sent_in_full));
    }
    if !rows.is_empty() {
        // cas-3bf1 (GH #176): a surfacing retires the row for the WHOLE
        // identity, not just the alias that happened to fetch it. Without
        // this, a row drained under `warm-jaguar-96` keeps no receipt under
        // `supervisor`, so the other reader re-injects it on a later turn —
        // and the in-turn dedupe above only covers duplicates WITHIN one
        // turn, never across them.
        crate::harness_policy::mirror_receipts_across_aliases_with_source(
            &*queue,
            &rows,
            &aliases,
            cas_store::SurfacingSource::HookSurfaced,
        );
    }
    // cas-6ccb: a block of repeats only says so; a mixed block keeps its
    // header and the [not acknowledged] note names the repeats.
    let header = if !rows.is_empty() && polled_unacked.len() == rows.len() {
        POLLED_REPEAT_HEADER
    } else {
        match surfacing {
            Surfacing::TurnStart => TURN_START_HEADER,
            Surfacing::ToolBoundary(_) => TOOL_BOUNDARY_HEADER,
            Surfacing::TurnPrompt { .. } => TURN_PROMPT_HEADER,
            Surfacing::PolledUnacked(_) => POLLED_REPEAT_HEADER,
        }
    };
    // cas-bf890: peer-supervisor and operator rows lead the block.
    let supervisor_ids = supervisor_senders(cas_root, &rows);
    rows.sort_by_key(|row| inbox_lane(row, &supervisor_ids, &aliases));
    let lanes: Vec<InboxLane> = rows
        .iter()
        .map(|row| inbox_lane(row, &supervisor_ids, &aliases))
        .collect();
    Some(SurfacingOutcome {
        text: render_block(&block_notes, &rows, &lanes, digest_row(&digest), header),
        complete,
    })
}

/// cas-1f0c: the `[incoming messages]` block: notes first, then the rows.
/// `None` when there is neither.
///
/// cas-bf890: `rows` arrive lane-ordered with their `lanes`; the digest row,
/// when there is one, closes the block as a single line.
fn render_block(
    notes: &[String],
    rows: &[QueuedPrompt],
    lanes: &[InboxLane],
    digest: Option<String>,
    header: &str,
) -> Option<String> {
    if notes.is_empty() && rows.is_empty() && digest.is_none() {
        return None;
    }
    let mut out = String::from("[incoming messages]\n");
    for note in notes {
        out.push_str(note);
        out.push('\n');
    }
    if !rows.is_empty() {
        let rendered = render_rows_with_header(rows, lanes, header);
        out.push_str(
            rendered
                .strip_prefix("[incoming messages]\n")
                .unwrap_or(&rendered),
        );
    }
    if let Some(digest) = digest {
        out.push('\n');
        out.push_str(&digest);
        out.push('\n');
    }
    Some(out)
}

/// cas-6ccb (GH #1215): rows shown again because they were read with
/// `inbox_poll` in an earlier turn and never acknowledged.
fn polled_unacked_note(ids: &[i64]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    Some(format!(
        "[not acknowledged] Message {} below is a one-time repeat: you read it with inbox_poll \
         in an earlier turn and did not acknowledge it. Skip it if you already handled it.",
        id_list(ids)
    ))
}

/// cas-1f0c: a row the wake named whose assignment no longer applies. Its
/// receipt is written, so without this the wake's promise of a body would end
/// in silence.
fn withdrawn_note(ids: &[i64]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    Some(format!(
        "[withdrawn] Message {} named by this turn's CAS wake is an assignment for a task that \
         is no longer open for you, so CAS did not show it. No action is needed.",
        id_list(ids)
    ))
}

/// cas-1f0c: named rows whose body the daemon's time fallback claimed to
/// send in full as a Teams copy. A crash between claim and write leaves no
/// copy, so the note also names `inbox_poll`.
fn sent_in_full_note(ids: &[i64]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    Some(format!(
        "[sent in full] Message {} is sent to you in full as its own teammate message (or \
         read it with inbox_poll notification_id=<id>).",
        id_list(ids)
    ))
}

/// cas-1f0c: rows left unread because their bodies did not fit this block.
fn over_budget_note(
    rows: &[(i64, usize)],
    queue: &dyn cas_store::PromptQueueStore,
) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let (too_long, no_room): (Vec<(i64, usize)>, Vec<(i64, usize)>) = rows
        .iter()
        .copied()
        .partition(|(_, cost)| *cost > cas_store::HOOK_SURFACE_BUDGET_CHARS);
    let ids = |rows: &[(i64, usize)]| id_list(&rows.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    let mut notes = Vec::new();
    if !too_long.is_empty() {
        notes.push(format!(
            "[not shown here] Message {} is too long for this block and stays unread. Its full \
             text comes as its own message; if it is not in your prompt, run inbox_poll \
             notification_id=<id>.",
            ids(&too_long)
        ));
    }
    if !no_room.is_empty() {
        notes.push(format!(
            "[not shown here] Message {} did not fit in this block and stays unread; it is \
             retried automatically at the next eligible turn or tool boundary.",
            ids(&no_room)
        ));
    }
    let mut note = notes.join("\n");
    // Keep the id/summary list inside the hook's existing notes allowance.
    let labels: Vec<_> = rows.iter().map(|(id, _)| format!("#{id} ")).collect();
    let fixed = labels.iter().map(|s| s.chars().count()).sum::<usize>()
        + rows.len().saturating_sub(1) * 2
        + "\nPending: ".len();
    let summary_limit =
        HOOK_MAIL_NOTES_RESERVE_CHARS.saturating_sub(note.chars().count() + fixed) / rows.len();
    let named: Vec<_> = rows
        .iter()
        .zip(labels)
        .map(|((id, _), label)| {
            let summary = match queue.queued_prompt(*id) {
                Ok(row) => row
                    .and_then(|row| row.summary)
                    .unwrap_or_else(|| "no summary".into()),
                Err(error) => {
                    tracing::warn!(prompt_id = id, %error, "overflow summary unavailable");
                    "summary unavailable".into()
                }
            };
            let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(
                "{label}{}",
                summary
                    .chars()
                    .take(summary_limit.min(80))
                    .collect::<String>()
            )
        })
        .collect();
    note.push_str("\nPending: ");
    note.push_str(&named.join("; "));
    Some(note)
}

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// cas-7bfa (GH #1209): record that this turn's own prompt carried queued
/// rows. Claude Code renders a held Agent Teams copy as a teammate-message
/// turn, and that turn fires no UserPromptSubmit hook, so nothing ever
/// stamped those rows. They stayed `gated` behind later delivered rows and
/// raised false "delivery stalled — recipient unread" notices. At the turn's
/// first tool boundary, each row this turn's prompt really carries takes over
/// the transport claim and is stamped `hook_surfaced`, which marks it
/// delivered. Nothing is rendered: the body is already the turn's prompt.
///
/// "Really carries" means all three hold: the prompt names the row's
/// `[cas #N …]` envelope, the row's own body text appears in the prompt, and
/// the row is addressed to this agent and already holds a transport receipt
/// (`transport_claimed` or `transport_delivered`). An envelope quoted inside
/// another message, or one naming a row no transport ever handed over,
/// therefore never confirms an undelivered row (cas-7bfa review).
///
/// Returns the rows recorded, in id order, so a caller can build on them
/// (cas-1f0c renders pointer-only copies).
pub(crate) fn record_turn_prompt_envelopes(
    cas_root: &Path,
    input: &HookInput,
    prompt: &str,
) -> Vec<QueuedPrompt> {
    let ids = envelope_ids(prompt);
    if ids.is_empty() || !owns_factory_inbox(input) {
        return Vec::new();
    }
    let Ok(queue) = crate::store::open_prompt_queue_store(cas_root) else {
        return Vec::new();
    };
    let aliases = recipient_aliases(input);
    let carried: Vec<i64> = ids
        .into_iter()
        .filter(|id| turn_prompt_carries_row(&*queue, prompt, &aliases, *id))
        .collect();
    if carried.is_empty() {
        return Vec::new();
    }
    let session = factory_session();
    let mut recorded: Vec<QueuedPrompt> = Vec::new();
    for alias in &aliases {
        if let Ok(rows) =
            queue.surface_wake_named_for_recipient(alias, session.as_deref(), &carried)
        {
            recorded.extend(rows);
        }
    }
    recorded.sort_by_key(|row| row.id);
    recorded.dedup_by_key(|row| row.id);
    recorded
}

/// Whether the turn `prompt` delivered queue row `id` to one of `aliases`:
/// a row addressed to them, directly or as a broadcast, for which one of
/// them holds a transport receipt, and whose body text is in the prompt
/// (cas-7bfa). cas-1f0c: the receipt is read per alias, so a broadcast's
/// full-body Teams copy is recorded too.
fn turn_prompt_carries_row(
    queue: &dyn cas_store::PromptQueueStore,
    prompt: &str,
    aliases: &[String],
    id: i64,
) -> bool {
    let Ok(Some(row)) = queue.queued_prompt(id) else {
        return false;
    };
    let body = row.prompt.trim();
    if body.is_empty()
        || !prompt.contains(body)
        || !(row.target == "all_workers"
            || aliases.iter().any(|alias| alias.eq_ignore_ascii_case(&row.target)))
    {
        return false;
    }
    aliases.iter().any(|alias| {
        matches!(
            queue.recipient_receipt(id, alias),
            Ok(Some(
                cas_store::SurfacingSource::TransportClaimed
                    | cas_store::SurfacingSource::TransportDelivered
            ))
        )
    })
}

/// cas-7ce4: the `[cas #N …]` message ids in a submitted prompt. The daemon
/// stamps every queued message's body with this envelope, so a teams copy
/// Claude Code renders as a teammate message names its row.
fn envelope_ids(prompt: &str) -> Vec<i64> {
    const MARKER: &str = "[cas #";
    let mut ids = Vec::new();
    let mut rest = prompt;
    while let Some(start) = rest.find(MARKER) {
        rest = &rest[start + MARKER.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse::<i64>()
            && rest[digits.len()..].starts_with(' ')
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// cas-7ce4: messages this turn's prompt names that were already put in front
/// of this agent before this turn: by a hook (turn start or tool boundary) or
/// by its own `inbox_poll`. The prompt names a message by its `[cas #N …]`
/// envelope or by a `CAS wake: message N` pointer.
///
/// cas-1f0c: since a Claude Teams inbox carries only a pointer, the teammate
/// block Claude Code renders after the body was shown holds no second body;
/// this note tells the agent the pointer is to something it already has. The
/// receipt is read per alias, so a broadcast (`target = all_workers`, read
/// state per recipient) is covered too.
fn already_surfaced_envelopes(
    queue: &dyn cas_store::PromptQueueStore,
    prompt: &str,
    aliases: &[String],
) -> Vec<i64> {
    let mut ids = envelope_ids(prompt);
    for id in wake_named_ids(prompt) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.into_iter()
        .filter(|id| {
            aliases.iter().any(|alias| {
                matches!(
                    queue.recipient_receipt(*id, alias),
                    Ok(Some(
                        cas_store::SurfacingSource::HookSurfaced
                            | cas_store::SurfacingSource::InboxPoll
                    ))
                )
            })
        })
        .collect()
}

fn already_shown_note(ids: &[i64]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    Some(format!(
        "[already shown] Message {} named in this turn's prompt was already shown to you \
         before this turn, by CAS or your own inbox_poll. This is a pointer or the harness's held \
         copy of the same message, not a new one: act on it only if you have not already.",
        id_list(ids)
    ))
}

/// cas-ad92: the message ids named by daemon pointer wakes in a submitted
/// prompt (`CAS wake: message {id} from …`, see `pointer_wake_payload`).
/// A named id only selects among rows already addressed to this recipient.
fn wake_named_ids(prompt: &str) -> Vec<i64> {
    const MARKER: &str = "CAS wake: message ";
    let mut ids = Vec::new();
    for line in prompt.lines() {
        let Some(rest) = line.trim_start().strip_prefix(MARKER) else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse::<i64>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// Render surfaced rows for injection into the turn.
///
/// Pure so the shape is testable without a store. The rendering states the
/// sender and the message id for every row, because the recipient's only way
/// to acknowledge or reply is to name that id back.
#[cfg(test)]
pub(crate) fn render_surfaced(rows: &[QueuedPrompt]) -> String {
    render_rows_with_header(rows, &[], TURN_START_HEADER)
}

/// `lanes[i]` labels `rows[i]`; a missing lane renders as ordinary.
fn render_rows_with_header(rows: &[QueuedPrompt], lanes: &[InboxLane], header: &str) -> String {
    let mut out = format!("[incoming messages]\n{header}\n");
    for (index, row) in rows.iter().enumerate() {
        let label = lanes.get(index).copied().unwrap_or(InboxLane::Ordinary).label();
        let reply_hint = crate::mcp::tools::service::agent_search_system::message::commander_reply_command(row)
            .map(|command| format!("Reply with: `{command}`\n"))
            .unwrap_or_default();
        out.push_str(&format!(
            "\n--- {label}from {} (message {}{}) ---\n{}\n{}{}\n",
            row.source,
            row.id,
            if row.urgent { ", urgent" } else { "" },
            crate::mcp::tools::service::agent_search_system::message::first_message_provenance(row),
            reply_hint,
            row.prompt.trim()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{
        ConfirmationSource, NotificationPriority, PromptQueueStore, SqlitePromptQueueStore,
    };
    use chrono::Utc;
    use tempfile::TempDir;

    fn row(id: i64, source: &str, prompt: &str) -> QueuedPrompt {
        QueuedPrompt {
            id,
            source: source.to_string(),
            target: "worker-1".to_string(),
            prompt: prompt.to_string(),
            created_at: Utc::now(),
            processed_at: None,
            factory_session: Some("session".to_string()),
            summary: None,
            priority: NotificationPriority::Normal,
            acked_at: None,
            urgent: false,
            origin: None,
            operator: None,
            recipient_device_id: None,
            kind: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn lifecycle_digest_names_a_relay_delayed_past_delivery_window() {
        let mut delayed = row(13952, "lifecycle:13952", "started");
        delayed.created_at = Utc::now() - chrono::Duration::minutes(29);
        delayed.summary = Some("task_started: cas-example".into());
        let mut timely = row(13956, "lifecycle:13956", "closed");
        timely.summary = Some("task_closed: cas-other".into());
        let rendered = digest_row(&[delayed, timely]).unwrap();
        assert!(rendered.contains("#13952 task_started: cas-example [delayed"), "{rendered}");
        assert!(rendered.contains("#13956 task_closed: cas-other"), "{rendered}");
        assert!(!rendered.contains("#13956 task_closed: cas-other [delayed"), "{rendered}");
    }

    /// The injected block must name the sender and the message id: a recipient
    /// that cannot identify the message cannot ack or reply to it.
    #[test]
    fn rendering_names_sender_and_message_id() {
        let rendered = render_surfaced(&[row(7640, "supervisor", "start cas-7a01")]);
        assert!(rendered.contains("supervisor"), "{rendered}");
        assert!(rendered.contains("7640"), "{rendered}");
        assert!(
            rendered.contains("[cas #7640 supervisor-authored")
                && rendered.contains("s first]"),
            "every hook-surfaced message must retain actionable queue provenance: {rendered}"
        );
        assert!(rendered.contains("start cas-7a01"), "{rendered}");
    }

    /// cas-7f81: the hook surface renders the same operator header the MCP
    /// path does, so a verified Commander message reads as the user on both.
    #[test]
    fn rendering_keeps_the_operator_header_on_hook_surfaced_rows() {
        let mut verified = row(81, "commander:Daniel@iphone-15", "Status please");
        verified.origin = Some(cas_store::QueueOrigin::PairedDevice {
            device_id: "dev-42".into(),
        });
        let rendered = render_surfaced(&[verified]);
        assert!(
            rendered.contains("[cas #81 operator Daniel@iphone-15 verified "),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "coordination action=message target=operator in_reply_to=81 summary=… message=…"
            ),
            "{rendered}"
        );
        let spoofed = row(82, "commander:Daniel@iphone-15", "Status please");
        let rendered = render_surfaced(&[spoofed]);
        assert!(rendered.contains("[cas #82 unverified:Daniel@iphone-15 "), "{rendered}");
        assert!(!rendered.contains("in_reply_to=82"), "{rendered}");
    }

    /// cas-1f0c round 3: PostToolUse lessons and guardrail warnings share
    /// the hook output with the mail, so they are reserved inside the cap.
    #[test]
    fn reserved_context_shrinks_the_body_budget_and_the_recall_room() {
        assert_eq!(hook_body_budget(0), cas_store::HOOK_SURFACE_BUDGET_CHARS);
        let lessons = 4_000;
        let budget = hook_body_budget(lessons);
        assert!(budget < cas_store::HOOK_SURFACE_BUDGET_CHARS);
        assert!(lessons + budget + HOOK_MAIL_NOTES_RESERVE_CHARS <= HOOK_CONTEXT_CAP_CHARS);
        assert_eq!(hook_body_budget(HOOK_CONTEXT_CAP_CHARS), 0);

        let recall = "r".repeat(3_000);
        let mut context = "m".repeat(5_000);
        assert!(push_within_hook_cap(&mut context.clone(), &recall, 0));
        assert!(!push_within_hook_cap(&mut context, &recall, 1_600), "lessons leave no room");
        assert_eq!(context.len(), 5_000, "a refused addition leaves the context unchanged");
    }

    #[test]
    fn envelope_ids_read_the_message_stamp_cas_7ce4() {
        assert_eq!(
            envelope_ids(
                "[cas #4184530 supervisor-authored 9s replay]\nbody [cas #12 agent-authored 0s first]"
            ),
            vec![4184530, 12]
        );
        assert!(envelope_ids("see [cas #abc] or [cas #12x] or cas #13").is_empty());
        let note = already_shown_note(&[7, 9]).unwrap();
        assert!(
            note.contains("#7, #9") && note.contains("already shown"),
            "{note}"
        );
        assert!(already_shown_note(&[]).is_none());
    }

    #[test]
    fn wake_named_ids_reads_the_daemons_pointer_wake() {
        let wake = crate::ui::factory::daemon::runtime::delivery::pointer_wake_payload(
            "supervisor",
            Some(4182255),
        );
        assert_eq!(wake_named_ids(&wake), vec![4182255]);
        assert_eq!(wake_named_ids(&format!("{wake}\n  {wake}")), vec![4182255]);
        let anonymous =
            crate::ui::factory::daemon::runtime::delivery::pointer_wake_payload("supervisor", None);
        assert!(wake_named_ids(&anonymous).is_empty());
        assert!(wake_named_ids("please read CAS wake: message 7 later").is_empty());
    }

    #[test]
    fn rendering_marks_urgent_rows() {
        let mut urgent = row(1, "supervisor", "stop");
        urgent.urgent = true;
        assert!(render_surfaced(&[urgent]).contains("urgent"));
    }

    #[test]
    fn turn_start_does_not_replay_a_transport_delivered_spawn_brief_after_progress() {
        let _guard = crate::hooks::test_env_lock();
        let temp = TempDir::new().unwrap();
        let cas_root = crate::store::init_cas_dir(temp.path()).unwrap();
        let task_store = crate::store::open_task_store(&cas_root).unwrap();
        let mut task = cas_types::Task::new(
            "cas-7c1e".to_string(),
            "spawn brief already acted on".to_string(),
        );
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.assignee = Some("worker-1".to_string());
        task_store.add(&task).unwrap();

        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let prompt = "You were spawned for task cas-7c1e — \"spawn brief already acted on\" — and it is assigned to you now."
            .to_string()
            + "\nStart with `mcp__cs__task action=show id=cas-7c1e`, then `mcp__cs__task action=start id=cas-7c1e` before you change any code.";
        let id = queue
            .enqueue_with_summary(
                "director",
                "worker-1",
                &prompt,
                Some("session"),
                Some("Assigned task: cas-7c1e"),
            )
            .unwrap();
        queue.mark_transport_delivered(id).unwrap();
        queue
            .record_recipient_surfaced(
                id,
                "worker-1",
                cas_store::SurfacingSource::TransportDelivered,
            )
            .unwrap();

        // SAFETY: guarded by the process-wide hook test env lock.
        unsafe {
            std::env::set_var("CAS_AGENT_NAME", "worker-1");
            std::env::set_var("CAS_FACTORY_SESSION", "session");
        }
        let mut input = HookInput {
            hook_event_name: "UserPromptSubmit".to_string(),
            ..Default::default()
        };
        input.agent_role = Some("worker".to_string());

        assert_eq!(
            surface_factory_inbox(Some(&cas_root), &input),
            None,
            "a stale spawn brief must be consumed without replaying task-start boilerplate"
        );

        // SAFETY: guarded by the process-wide hook test env lock.
        unsafe {
            std::env::remove_var("CAS_AGENT_NAME");
            std::env::remove_var("CAS_FACTORY_SESSION");
        }
    }

    /// cas-78d3 (GH #165) — the regression this whole task exists for.
    ///
    /// The payload literal below is the CONTRACT: it is the shape Claude Code
    /// actually sends on `UserPromptSubmit`, submitted text under the key
    /// **`prompt`**. Cassy's `HookInput` matched only `user_prompt`/`userPrompt`,
    /// so on every real turn `user_prompt` deserialized to `None`,
    /// `handle_user_prompt_submit` hit its empty-prompt early return, and the
    /// surfacing block never ran — which is why `acked_via = 'hook_surfaced'`
    /// had zero rows in production while the store-side code that writes it was
    /// shipped, correct and fully unit-tested.
    ///
    /// This asserts the whole seam in one pass: raw JSON in the real shape →
    /// handler → injected context → `acked_via` persisted. Do not rewrite the
    /// literal to use `user_prompt`; that is precisely the assumption that made
    /// the bug invisible for a full release.
    #[test]
    fn stalled_nudge_hooks_skip_routine_alarm_at_turn_and_tool_boundaries() {
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_NAME", Some("worker-stall-hook")),
            ("CAS_FACTORY_SESSION", Some("stall-hook-session")),
        ]);
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let outcome = store.enqueue_worker_stall_nudge("worker-stall-hook", "you are stalled",
            "stall-hook-session", "cas-stall-hook", "worker-attention:hook-test").unwrap();
        let id = match outcome {
            cas_store::EnqueueIdempotentResult::Created(id) | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        let mut input: HookInput = serde_json::from_str(r#"{
            "session_id":"hook-stall", "cwd":"/tmp", "hook_event_name":"UserPromptSubmit", "prompt":"continue"
        }"#).unwrap();
        input.agent_role = Some("worker".into());
        let at_turn = surface_factory_inbox(Some(temp.path()), &input);
        let mut budget = cas_store::HOOK_SURFACE_BUDGET_CHARS;
        let at_tool = surface_factory_inbox_after_tool_result(Some(temp.path()), &input, None, &mut budget)
            .and_then(|outcome| outcome.text);
        let in_turn = surface_factory_inbox_after_tool_result(Some(temp.path()), &input,
            Some(CurrentTurn { started_at: Utc::now(), prompt: "continue" }), &mut budget)
            .and_then(|outcome| outcome.text);
        assert!(at_turn.is_none() && at_tool.is_none() && in_turn.is_none(),
            "automatic hook surfacing must leave premise-sensitive nudges to the daemon");
        let queued = store.queued_prompt(id).unwrap().unwrap();
        assert!(queued.processed_at.is_none() && queued.acked_at.is_none(),
            "skipped nudges must not acquire false delivery receipts");
    }

    #[test]
    fn claude_real_payload_surfaces_mail_and_stamps_hook_surfaced() {
        let _guard = crate::hooks::test_env_lock();
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let id = store
            .enqueue_with_session("supervisor", "worker-78d3", "ship cas-78d3", "sess-78d3")
            .unwrap();

        // SAFETY: guarded by the process-wide hook test env lock.
        unsafe {
            std::env::set_var("CAS_AGENT_NAME", "worker-78d3");
            std::env::set_var("CAS_FACTORY_SESSION", "sess-78d3");
        }

        let mut input: HookInput = serde_json::from_str(
            r#"{
                 "session_id": "s-78d3",
                 "transcript_path": "/tmp/transcript.jsonl",
                 "cwd": "/tmp",
                 "hook_event_name": "UserPromptSubmit",
                 "prompt": "continue"
               }"#,
        )
        .expect("Claude's real UserPromptSubmit payload must deserialize");
        input.agent_role = Some("worker".to_string());

        assert_eq!(
            input.submitted_prompt(),
            Some("continue"),
            "Claude sends the submitted text as `prompt`; if this is None the \
             handler bails before surfacing and GH #165 is back"
        );

        let out = super::super::prompt_capture::handle_user_prompt_submit(&input, Some(temp.path()))
            .expect("handler must not error");
        let context = out
            .user_prompt_context()
            .expect("mail must be injected into the turn");
        assert!(
            context.contains("ship cas-78d3"),
            "surfaced context missing the message body: {context}"
        );

        // SAFETY: guarded by the process-wide hook test env lock.
        unsafe {
            std::env::remove_var("CAS_AGENT_NAME");
            std::env::remove_var("CAS_FACTORY_SESSION");
        }

        let report = store
            .message_delivery_report(id)
            .unwrap()
            .expect("row must exist");
        assert_eq!(
            report.confirmation_source,
            ConfirmationSource::HookSurfaced,
            "a row injected into a live turn must be acked hook_surfaced — an \
             unacked row is the 100%-unreconciled steady state of GH #165"
        );
    }

    /// The empty-prompt early return used to sit ABOVE the surfacing block, so
    /// a blank prompt silently withheld a factory agent's mail. Surfacing must
    /// not depend on a capture precondition it has nothing to do with.
    #[test]
    fn a_blank_prompt_still_surfaces_mail() {
        let _guard = crate::hooks::test_env_lock();
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        store
            .enqueue_with_session("supervisor", "worker-blank", "urgent context", "sess-blank")
            .unwrap();

        // SAFETY: guarded by the process-wide hook test env lock.
        unsafe {
            std::env::set_var("CAS_AGENT_NAME", "worker-blank");
            std::env::set_var("CAS_FACTORY_SESSION", "sess-blank");
        }

        let mut input = HookInput {
            hook_event_name: "UserPromptSubmit".to_string(),
            user_prompt: Some("   ".to_string()),
            ..Default::default()
        };
        input.agent_role = Some("worker".to_string());

        let out = super::super::prompt_capture::handle_user_prompt_submit(&input, Some(temp.path()))
            .expect("handler must not error");

        // SAFETY: guarded by the process-wide hook test env lock.
        unsafe {
            std::env::remove_var("CAS_AGENT_NAME");
            std::env::remove_var("CAS_FACTORY_SESSION");
        }

        assert!(
            out.user_prompt_context()
                .is_some_and(|c| c.contains("urgent context")),
            "a blank prompt is still a turn the recipient is taking; mail \
             withheld from it is mail withheld indefinitely"
        );
    }

    /// AC2/AC3 core: a row surfaced once is receipted at injection time, so a
    /// second turn start never re-injects it. This is the GH #124 storm guard.
    #[test]
    fn a_surfaced_row_is_never_surfaced_twice() {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        store
            .enqueue_with_session("supervisor", "worker-1", "do the thing", "session")
            .unwrap();

        let first = store
            .surface_unseen_for_recipient("worker-1", Some("session"), 10)
            .unwrap();
        assert_eq!(first.len(), 1, "the message must reach the first turn");

        let second = store
            .surface_unseen_for_recipient("worker-1", Some("session"), 10)
            .unwrap();
        assert!(
            second.is_empty(),
            "re-injecting a receipted row into a later turn is the GH #124 storm"
        );
    }
}
