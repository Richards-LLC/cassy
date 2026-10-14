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
use cas_store::QueuedPrompt;

fn assignment_is_stale_for_recipient(
    task_store: &dyn crate::store::TaskStore,
    row: &QueuedPrompt,
    recipient: &str,
) -> bool {
    let Some(task_id) = crate::prompt_revalidation::assignment_solicited_task_id(&row.prompt) else {
        return false;
    };
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
pub(crate) fn owns_factory_inbox(input: &HookInput) -> bool {
    crate::harness_policy::is_factory_agent(input)
        && input.agent_id.is_none()
        && input.agent_type.is_none()
        && input.subagent_type.is_none()
        && !input.transcript_path.as_deref().is_some_and(|path| {
            Path::new(path).components().any(|part| part.as_os_str() == "subagents")
        })
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
pub fn surface_factory_inbox(cas_root: Option<&Path>, input: &HookInput) -> Option<String> {
    surface_factory_inbox_with_transport_delivery(cas_root, input, Surfacing::TurnStart)
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
/// eligible.
pub(crate) fn surface_factory_inbox_after_tool_result(
    cas_root: Option<&Path>,
    input: &HookInput,
    turn: Option<CurrentTurn<'_>>,
) -> Option<String> {
    surface_factory_inbox_with_transport_delivery(cas_root, input, Surfacing::ToolBoundary(turn))
}

enum Surfacing<'a> {
    TurnStart,
    ToolBoundary(Option<CurrentTurn<'a>>),
}

fn surface_factory_inbox_with_transport_delivery(
    cas_root: Option<&Path>,
    input: &HookInput,
    surfacing: Surfacing<'_>,
) -> Option<String> {
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

    let wake_ids = match &surfacing {
        Surfacing::TurnStart => wake_named_ids(input.submitted_prompt().unwrap_or_default()),
        Surfacing::ToolBoundary(_) => Vec::new(),
    };
    // cas-7ce4: read before this turn's drain, so only an earlier surfacing
    // counts as "already shown".
    let already_shown = match &surfacing {
        Surfacing::TurnStart => already_surfaced_envelopes(
            &*queue,
            input.submitted_prompt().unwrap_or_default(),
            &aliases,
        ),
        Surfacing::ToolBoundary(_) => Vec::new(),
    };
    let mut rows: Vec<QueuedPrompt> = Vec::new();
    for alias in &aliases {
        let remaining = SURFACE_LIMIT.saturating_sub(rows.len());
        if remaining == 0 {
            break;
        }
        let found = match &surfacing {
            // cas-ad92: a pointer wake names the row whose body the harness
            // holds back until the next turn boundary; surface it into the
            // wake turn first, over the daemon's transport claim.
            Surfacing::TurnStart => queue
                .surface_wake_named_for_recipient(alias, session.as_deref(), &wake_ids)
                .and_then(|mut named| {
                    let left = remaining.saturating_sub(named.len());
                    named.extend(queue.surface_unseen_for_recipient(
                        alias,
                        session.as_deref(),
                        left,
                    )?);
                    Ok(named)
                }),
            Surfacing::ToolBoundary(Some(turn)) => queue
                .surface_unseen_for_recipient_delivered_after(
                    alias,
                    session.as_deref(),
                    remaining,
                    turn.started_at,
                ),
            Surfacing::ToolBoundary(None) => queue
                .surface_unseen_for_recipient_without_transport_delivery(
                    alias,
                    session.as_deref(),
                    remaining,
                ),
        };
        match found {
            Ok(found) => {
                for row in found {
                    // Transport-delivered rows remain eligible for this hook
                    // so a wake the harness dropped can recover. Revalidate
                    // assignment/start boilerplate before rendering it: the
                    // task may have moved beyond Open while the row waited.
                    if task_store.as_ref().is_some_and(|store| {
                        assignment_is_stale_for_recipient(store.as_ref(), &row, alias)
                    }) {
                        tracing::info!(
                            target: "cas::coordination",
                            stage = "suppress_stale_assignment_at_hook_surface",
                            prompt_id = row.id,
                            recipient = %alias,
                            "cas-7c1a: withheld stale assignment boilerplate from turn-start surfacing"
                        );
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
                    rows.push(row);
                }
            }
            Err(error) => {
                tracing::debug!(
                    target: "cas::coordination",
                    recipient = %alias,
                    %error,
                    "cas-7a01: turn-start inbox surfacing failed"
                );
            }
        }
    }

    let repeat_note = already_shown_note(&already_shown);
    if rows.is_empty() {
        return repeat_note;
    }
    // cas-3bf1 (GH #176): a surfacing retires the row for the WHOLE identity,
    // not just the alias that happened to fetch it. Without this, a row drained
    // under `warm-jaguar-96` keeps no receipt under `supervisor`, so the other
    // reader re-injects it on a later turn — and the in-turn dedupe above only
    // covers duplicates WITHIN one turn, never across them.
    crate::harness_policy::mirror_receipts_across_aliases_with_source(
        &*queue,
        &rows,
        &aliases,
        cas_store::SurfacingSource::HookSurfaced,
    );
    let rendered = match surfacing {
        Surfacing::TurnStart => render_surfaced(&rows),
        Surfacing::ToolBoundary(_) => render_surfaced_with_header(
            &rows,
            "The following message(s) arrived while you were working. \
             They are delivered here once — read them before your next step.",
        ),
    };
    Some(match repeat_note {
        Some(note) => format!("{note}\n\n{rendered}"),
        None => rendered,
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

/// cas-7ce4: messages this turn's prompt repeats that a hook already put in
/// front of this agent in an earlier turn. After cas-b5ad a busy Claude worker
/// sees a claimed message at a tool boundary, and Claude Code later renders
/// the teams copy it held; the harness copy cannot be withdrawn (the teams
/// layer drains its inbox file at once), so the turn says it is a repeat.
fn already_surfaced_envelopes(
    queue: &dyn cas_store::PromptQueueStore,
    prompt: &str,
    aliases: &[String],
) -> Vec<i64> {
    envelope_ids(prompt)
        .into_iter()
        .filter(|id| {
            queue
                .message_delivery_report(*id)
                .ok()
                .flatten()
                .is_some_and(|report| {
                    report.recipient_receipt == Some(cas_store::SurfacingSource::HookSurfaced)
                        && aliases
                            .iter()
                            .any(|alias| alias.eq_ignore_ascii_case(&report.target))
                })
        })
        .collect()
}

fn already_shown_note(ids: &[i64]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let named = ids
        .iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "[already shown] Message {named} in this turn's prompt was already shown to you in an \
         earlier turn, at a tool call. This is the harness's held copy of the same message, not a \
         new one: act on it only if you have not already."
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
pub(crate) fn render_surfaced(rows: &[QueuedPrompt]) -> String {
    render_surfaced_with_header(
        rows,
        "The following message(s) arrived while you were not in a turn. \
         They are delivered here once — act on them now.",
    )
}

fn render_surfaced_with_header(rows: &[QueuedPrompt], header: &str) -> String {
    let mut out = format!("[incoming messages]\n{header}\n");
    for row in rows {
        let reply_hint = crate::mcp::tools::service::agent_search_system::message::commander_reply_command(row)
            .map(|command| format!("Reply with: `{command}`\n"))
            .unwrap_or_default();
        out.push_str(&format!(
            "\n--- from {} (message {}{}) ---\n{}\n{}{}\n",
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
