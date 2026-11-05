//! A supervisor's own open question to a peer holds the merge it is about
//! (cas-bddf9).
//!
//! The gabber supervisor asked prowl to review PR #4249, then merged the PR
//! to staging without the answer; the review later found two money P1s. The
//! ask is already recorded in the peer outbox and any reply carries its id, so
//! no new gate object is needed: `worktree_merge` and a supervisor's raw
//! `gh pr merge` are refused while the merging supervisor has an unanswered
//! ask (or blocker) to a peer that names the task or PR being merged.
//!
//! Override: a task note `PEER ASK OVERRIDE <ask id>: <reason>`. The merge
//! then proceeds and the operator receives one line, once per ask; the
//! override also counts as an `override_merge` intervention.

use std::path::Path;

use crate::cloud::peer_mailbox::OpenPeerAsk;

/// Asks older than this no longer hold a merge.
const ASK_WINDOW_SECS: i64 = 3 * 24 * 60 * 60;
pub(crate) const OVERRIDE_MARKER: &str = "PEER ASK OVERRIDE";

/// What is being merged.
#[derive(Debug, Default, Clone)]
pub(crate) struct MergeSubject {
    pub(crate) task_id: Option<String>,
    pub(crate) pr: Option<u64>,
}

fn names_word(body: &str, word: &str) -> bool {
    body.match_indices(word).any(|(at, _)| {
        let before = body[..at].chars().next_back();
        let after = body[at + word.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

fn names_subject(body: &str, subject: &MergeSubject) -> bool {
    subject.task_id.as_deref().is_some_and(|id| names_word(body, id))
        || subject.pr.is_some_and(|pr| {
            ["#", "pull/", "PR ", "pr "]
                .iter()
                .any(|prefix| names_word(body, &format!("{prefix}{pr}")))
        })
}

/// The ask id a reply or an override names.
fn ask_id(ask: &OpenPeerAsk) -> &str {
    ask.cloud_id.as_deref().unwrap_or("<queued>")
}

/// Unclosed task notes that override `ask` with a non-empty reason.
fn override_reason(cas_root: &Path, ask: &OpenPeerAsk) -> Option<(String, String)> {
    let id = ask.cloud_id.as_deref()?;
    let tasks = crate::store::open_task_store(cas_root).ok()?.list(None).ok()?;
    let marker = format!("{OVERRIDE_MARKER} {id}:");
    tasks.into_iter().find_map(|task| {
        task.notes.lines().find_map(|line| {
            let reason = line.split_once(&marker)?.1.trim();
            (!reason.is_empty()).then(|| (task.id.clone(), reason.to_string()))
        })
    })
}

fn summary(body: &str) -> String {
    let text = body
        .lines()
        .filter(|line| !line.starts_with("<cas-peer-intent"))
        .collect::<Vec<_>>()
        .join(" ");
    cas_core::hooks::truncate(text.trim(), 120)
}

/// The refusal for merging `subject` while `supervisor_agent_id` waits on a
/// peer. An overridden ask lets the merge through and tells the operator.
pub(crate) fn merge_refusal(
    cas_root: &Path,
    supervisor_agent_id: &str,
    supervisor_name: &str,
    operator_session: Option<&str>,
    subject: &MergeSubject,
) -> Option<String> {
    if subject.task_id.is_none() && subject.pr.is_none() {
        return None;
    }
    let asks = crate::cloud::peer_mailbox::open_asks(cas_root, supervisor_agent_id, ASK_WINDOW_SECS);
    for ask in asks.iter().filter(|ask| names_subject(&ask.body, subject)) {
        if let Some((task_id, reason)) = override_reason(cas_root, ask) {
            announce_override(cas_root, supervisor_name, operator_session, ask, &task_id, &reason);
            continue;
        }
        let note_task = subject.task_id.as_deref().unwrap_or("<task>");
        return Some(format!(
            "🚫 UNANSWERED PEER ASK: you asked {} \"{}\" (ask {}) and no reply has arrived. \
             Wait for the answer, or override visibly: \
             `task action=notes id={note_task} note_type=decision notes=\"{OVERRIDE_MARKER} {}: <reason>\"`, \
             then retry the merge (the operator is told).",
            ask.recipient,
            summary(&ask.body),
            ask_id(ask),
            ask_id(ask),
        ));
    }
    None
}

fn announce_override(
    cas_root: &Path,
    supervisor_name: &str,
    operator_session: Option<&str>,
    ask: &OpenPeerAsk,
    task_id: &str,
    reason: &str,
) {
    let id = ask_id(ask);
    let line = format!(
        "⚠️ {supervisor_name} merged past an unanswered ask to {} on {task_id} (ask {id}): {reason}",
        ask.recipient
    );
    crate::quality_ledger::intervention(
        cas_root,
        cas_store::InterventionKind::OverrideMerge,
        format!("override_peer_ask|{id}"),
        Some(task_id),
        Some(supervisor_name),
        Some(reason),
    );
    let _ = crate::supervisor_notices::enqueue(
        cas_root,
        &crate::supervisor_notices::Recipient::operator(operator_session.unwrap_or("supervisor-recovery")),
        "peer_ask_override",
        &line,
        id,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::SupervisorQueueStore;

    fn sent_ask(conn: &rusqlite::Connection, cloud_id: &str, body: &str) {
        sent_ask_from(conn, "sup-soundwave", cloud_id, body);
    }

    fn sent_ask_from(conn: &rusqlite::Connection, sender: &str, cloud_id: &str, body: &str) {
        let payload = serde_json::json!({
            "body": format!("<cas-peer-intent kind=\"ask\"/>\n{body}"),
        });
        conn.execute(
            "INSERT INTO peer_message_outbox (sender_agent_id, sender_name, recipient, payload, state, created_at, next_attempt_at, cloud_id)
             VALUES (?3, 'cosmic-shark-29', 'lively-cheetah-57@prowl', ?1, 'sent', strftime('%s','now'), 0, ?2)",
            rusqlite::params![payload.to_string(), cloud_id, sender],
        )
        .unwrap();
    }

    /// Replay of gabber PR #4249 (Nov 5): the soundwave supervisor asked
    /// prowl to review the PR, then merged it without an answer. The merge
    /// is held while the ask is open, released by a reply, and an explicit
    /// override lets it through only with a reason the operator sees.
    #[test]
    fn a_merge_waits_on_the_supervisors_unanswered_peer_ask_cas_bddf9() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        crate::store::open_prompt_queue_store(&root).unwrap();
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&cas_types::Task::new("cas-0c7f".into(), "Final review of batch 22".into()))
            .unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        sent_ask(&conn, "ask-4249", "Batch 22 is FROZEN: PR #4249. Please run tender-finch-34's final review (cas-0c7f).");
        sent_ask(&conn, "ask-4251", "Can you review PR #4251?");
        let merge = |pr| {
            merge_refusal(&root, "sup-soundwave", "cosmic-shark-29", Some("gabber-factory"),
                &MergeSubject { task_id: None, pr: Some(pr) })
        };

        let refused = merge(4249).expect("open ask holds its PR");
        assert!(refused.contains("UNANSWERED PEER ASK") && refused.contains("ask-4249"), "{refused}");
        assert!(merge(4250).is_none(), "an ask about another PR holds nothing");
        assert!(
            merge_refusal(&root, "sup-other", "other", None, &MergeSubject { task_id: None, pr: Some(4249) }).is_none(),
            "only the merging supervisor's own asks hold its merge"
        );

        conn.execute(
            "INSERT INTO prompt_queue (source, target, prompt) VALUES ('peer:lively-cheetah-57@prowl', 'cosmic-shark-29',
             '<cas-peer-message v=\"1\" id=\"r1\" from=\"lively-cheetah-57\" from_agent=\"p\" machine=\"prowl\" repo=\"gabber-studio\" in_reply_to=\"ask-4251\" kind=\"answer\"/>')",
            [],
        )
        .unwrap();
        assert!(merge(4251).is_none(), "a peer reply answers the ask");

        crate::store::open_task_store(&root)
            .unwrap()
            .append_note("cas-0c7f", &format!("{OVERRIDE_MARKER} ask-4249: 3 green gates; prowl silent 7h"))
            .unwrap();
        assert!(merge(4249).is_none(), "a visible override lets the merge through");
        let notices = crate::store::open_supervisor_queue_store(&root)
            .unwrap()
            .list_pending("operator")
            .unwrap();
        assert!(
            notices.iter().any(|notice| notice.payload.contains("merged past an unanswered ask")
                && notice.payload.contains("prowl silent 7h")),
            "{notices:?}"
        );
        let events = cas_store::quality_events_between(&root, None, None).unwrap();
        assert!(events.iter().any(|event| event.task_id.as_deref() == Some("cas-0c7f")), "{events:?}");
    }

    fn fixture() -> (crate::test_support::TestEnvGuard, tempfile::TempDir, std::path::PathBuf, rusqlite::Connection) {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        crate::store::open_prompt_queue_store(&root).unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        (env, temp, root, conn)
    }

    fn pr_merge(root: &std::path::Path, sender: &str, pr: u64) -> Option<String> {
        merge_refusal(root, sender, "cosmic-shark-29", None, &MergeSubject { pr: Some(pr), ..Default::default() })
    }

    /// Verifier ver-37f17bf1f171 (1): every way an ask writes the PR holds it.
    #[test]
    fn every_pr_reference_form_in_an_ask_holds_its_merge_cas_bddf9() {
        let (_env, _temp, root, conn) = fixture();
        let forms = ["PR#4249", "gabber-studio#4249", "Richards-LLC/gabber-studio#4249", "pull request 4249", "PR-4249"];
        for (index, form) in forms.iter().enumerate() {
            let sender = format!("sup-{index}");
            sent_ask_from(&conn, &sender, &format!("ask-{index}"), &format!("Please review {form} tonight."));
            assert!(pr_merge(&root, &sender, 4249).is_some(), "{form} must hold PR 4249");
        }
    }

    /// Verifier ver-37f17bf1f171 (3): a bare acknowledgement is not an answer.
    #[test]
    fn a_receipt_reply_does_not_answer_an_ask_cas_bddf9() {
        let (_env, _temp, root, conn) = fixture();
        sent_ask(&conn, "ask-ok", "Can you review PR #4249?");
        conn.execute(
            "INSERT INTO prompt_queue (source, target, prompt) VALUES ('peer:lively-cheetah-57@prowl', 'cosmic-shark-29',
             '<cas-peer-message v=\"1\" id=\"r2\" from=\"lively-cheetah-57\" from_agent=\"p\" machine=\"prowl\" repo=\"gabber-studio\" in_reply_to=\"ask-ok\" kind=\"receipt\"/>\nok')",
            [],
        )
        .unwrap();
        assert!(pr_merge(&root, "sup-soundwave", 4249).is_some(), "an \"ok\" receipt leaves the ask open");
    }
}
