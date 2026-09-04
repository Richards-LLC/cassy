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
    if !crate::harness_policy::is_factory_agent(input) {
        return None;
    }
    let cas_root = cas_root?;
    let aliases = recipient_aliases(input);
    if aliases.is_empty() {
        return None;
    }
    let session = factory_session();
    let queue = crate::store::open_prompt_queue_store(cas_root).ok()?;

    let mut rows: Vec<QueuedPrompt> = Vec::new();
    for alias in &aliases {
        let remaining = SURFACE_LIMIT.saturating_sub(rows.len());
        if remaining == 0 {
            break;
        }
        match queue.surface_unseen_for_recipient(alias, session.as_deref(), remaining) {
            Ok(found) => {
                for row in found {
                    // A supervisor's two aliases are two distinct recipient
                    // keys in the receipt table, so a broadcast row can come
                    // back from both. Injecting it twice into one turn is the
                    // duplicate this task must not create.
                    if rows.iter().any(|existing| existing.id == row.id) {
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

    if rows.is_empty() {
        return None;
    }
    // cas-3bf1 (GH #176): a surfacing retires the row for the WHOLE identity,
    // not just the alias that happened to fetch it. Without this, a row drained
    // under `warm-jaguar-96` keeps no receipt under `supervisor`, so the other
    // reader re-injects it on a later turn — and the in-turn dedupe above only
    // covers duplicates WITHIN one turn, never across them.
    crate::harness_policy::mirror_receipts_across_aliases(&*queue, &rows, &aliases);
    Some(render_surfaced(&rows))
}

/// Render surfaced rows for injection into the turn.
///
/// Pure so the shape is testable without a store. The rendering states the
/// sender and the message id for every row, because the recipient's only way
/// to acknowledge or reply is to name that id back.
pub(crate) fn render_surfaced(rows: &[QueuedPrompt]) -> String {
    let mut out = String::from(
        "[incoming messages]\nThe following message(s) arrived while you were not in a turn. \
         They are delivered here once — act on them now.\n",
    );
    for row in rows {
        out.push_str(&format!(
            "\n--- from {} (message {}{}) ---\n{}\n{}\n",
            row.source,
            row.id,
            if row.urgent { ", urgent" } else { "" },
            crate::mcp::tools::service::agent_search_system::message::queued_message_provenance(row),
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
            rendered.contains("origin=supervisor-authored")
                && rendered.contains("queued_at=")
                && rendered.contains("delivery=first-delivery"),
            "every hook-surfaced message must retain actionable queue provenance: {rendered}"
        );
        assert!(rendered.contains("start cas-7a01"), "{rendered}");
    }

    #[test]
    fn rendering_marks_urgent_rows() {
        let mut urgent = row(1, "supervisor", "stop");
        urgent.urgent = true;
        assert!(render_surfaced(&[urgent]).contains("urgent"));
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
