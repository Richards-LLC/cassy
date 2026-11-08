//! A supervisor's own open question to a peer holds the merge it is about
//! (cas-bddf9).
//!
//! The gabber supervisor asked prowl to review PR #4249, then merged the PR
//! to staging without the answer; the review later found two money P1s. The
//! ask is already recorded in the peer outbox and answers are matched there, so
//! no new gate object is needed: `worktree_merge` and a supervisor's raw
//! `gh pr merge` are refused while the merging supervisor has an unanswered
//! explicit ask to a peer that names the task or PR being merged.
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
    /// A PR merge whose number could not be resolved: every open ask holds
    /// it (fail closed).
    pub(crate) any_pr: bool,
}

fn names_word(body: &str, word: &str) -> bool {
    body.match_indices(word).any(|(at, _)| {
        let before = body[..at].chars().next_back();
        let after = body[at + word.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// True when `body` contains `number` with no digit on either side. Any
/// phrasing names the PR ("PRs 4249 and 4250", "PR (4249)"); a false hold is
/// safe because the override exists.
fn names_number(body: &str, number: u64) -> bool {
    let needle = number.to_string();
    body.match_indices(&needle).any(|(at, _)| {
        let digit_before = body[..at].chars().next_back().is_some_and(|c| c.is_ascii_digit());
        let digit_after = body[at + needle.len()..].chars().next().is_some_and(|c| c.is_ascii_digit());
        !digit_before && !digit_after
    })
}

fn names_subject(body: &str, subject: &MergeSubject) -> bool {
    // An unresolvable PR merge is held by every open ask (fail closed).
    subject.any_pr
        || subject.task_id.as_deref().is_some_and(|id| names_word(body, id))
        || subject.pr.is_some_and(|pr| names_number(body, pr))
}

/// The task an ask names, for the override note; else the merged task.
fn task_for_override(cas_root: &Path, ask: &OpenPeerAsk, subject: &MergeSubject) -> String {
    if let Some(task_id) = &subject.task_id {
        return task_id.clone();
    }
    crate::store::open_task_store(cas_root)
        .ok()
        .and_then(|store| store.list(None).ok())
        .and_then(|tasks| {
            tasks
                .into_iter()
                .map(|task| task.id)
                .find(|id| names_word(&ask.body, id))
        })
        .unwrap_or_else(|| "<any task id>".to_string())
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
    if subject.task_id.is_none() && subject.pr.is_none() && !subject.any_pr {
        return None;
    }
    let asks = crate::cloud::peer_mailbox::open_asks(cas_root, supervisor_agent_id, ASK_WINDOW_SECS);
    for ask in asks.iter().filter(|ask| names_subject(&ask.body, subject)) {
        if let Some((task_id, reason)) = override_reason(cas_root, ask) {
            announce_override(cas_root, supervisor_name, operator_session, ask, &task_id, &reason);
            continue;
        }
        let note_task = task_for_override(cas_root, ask, subject);
        let answer_state = match ask.unlinked_answer.as_deref() {
            Some(id) => format!("peer answer {id} arrived but is not linked to this ask"),
            None => "no answer is linked to this ask".to_string(),
        };
        return Some(format!(
            "🚫 UNANSWERED PEER ASK: you asked {} \"{}\" (ask {}) and {answer_state}. \
             Wait for an answer that identifies this ask, or override visibly: \
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

/// A supervisor's raw `gh pr merge`: every merge in `command` resolves to
/// its PR the way the QA merge guard does; an unresolvable merge is held by
/// every open ask.
pub(crate) fn github_merge_refusal(
    cas_root: &Path,
    cwd: &Path,
    command: &str,
    supervisor_agent_id: &str,
    supervisor_name: &str,
    operator_session: Option<&str>,
) -> Option<String> {
    let merges = crate::qa_pass::github_gate::github_merges_in(command);
    if merges.is_empty()
        || crate::cloud::peer_mailbox::open_asks(cas_root, supervisor_agent_id, ASK_WINDOW_SECS).is_empty()
    {
        return None;
    }
    merges
        .iter()
        .find_map(|merge| {
            let subject = match crate::qa_pass::github_gate::merge_pr_number(cwd, merge) {
                Ok(pr) => MergeSubject { task_id: None, pr: Some(pr), any_pr: false },
                Err(_) => MergeSubject { task_id: None, pr: None, any_pr: true },
            };
            merge_refusal(cas_root, supervisor_agent_id, supervisor_name, operator_session, &subject)
        })
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
        "⚠️ {supervisor_name} overrode an unanswered ask to {} on {task_id} (ask {id}) to merge: {reason}",
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
        sent_message_from(conn, sender, cloud_id, body, Some("ask"));
    }

    fn sent_message_from(conn: &rusqlite::Connection, sender: &str, cloud_id: &str, body: &str, kind: Option<&str>) {
        let payload = serde_json::json!({
            "body": crate::cloud::peer_mailbox::attach_message_intent(body, kind, false).unwrap(),
            "recipient_agent_id": "p",
        });
        conn.execute(
            "INSERT INTO peer_message_outbox (sender_agent_id, sender_name, recipient, payload, state, created_at, next_attempt_at, cloud_id)
             VALUES (?3, 'cosmic-shark-29', 'lively-cheetah-57@prowl', ?1, 'sent', strftime('%s','now') - 60, 0, ?2)",
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
                &MergeSubject { task_id: None, pr: Some(pr), any_pr: false })
        };

        let refused = merge(4249).expect("open ask holds its PR");
        assert!(refused.contains("ask-4249"), "{refused}");
        assert!(merge(4250).is_none(), "an ask about another PR holds nothing");
        assert!(
            merge_refusal(&root, "sup-other", "other", None, &MergeSubject { task_id: None, pr: Some(4249), any_pr: false }).is_none(),
            "only the merging supervisor's own asks hold its merge"
        );

        crate::store::open_task_store(&root)
            .unwrap()
            .append_note("cas-0c7f", &format!("{OVERRIDE_MARKER} ask-4249: 3 green gates; prowl silent 7h"))
            .unwrap();
        assert!(merge(4249).is_none(), "a visible override lets the merge through");
        let notices = crate::store::open_supervisor_queue_store(&root)
            .unwrap()
            .list_pending("operator")
            .unwrap();
        assert!(notices.iter().any(|notice| notice.payload.contains("prowl silent 7h")), "{notices:?}");
        let events = cas_store::quality_events_between(&root, None, None).unwrap();
        assert!(events.iter().any(|event| event.task_id.as_deref() == Some("cas-0c7f")), "{events:?}");
        conn.execute(
            "INSERT INTO prompt_queue (source, target, prompt) VALUES ('peer:lively-cheetah-57@prowl', 'cosmic-shark-29',
             '<cas-peer-message v=\"1\" id=\"r1\" from=\"lively-cheetah-57\" from_agent=\"p\" machine=\"prowl\" repo=\"gabber-studio\" in_reply_to=\"ask-4251\" kind=\"answer\"/>')",
            [],
        )
        .unwrap();
        assert!(merge(4251).is_none(), "a peer reply answers the ask");
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

    #[tokio::test]
    async fn unlinked_peer_answer_releases_the_worktree_merge_gate_cas_7435() {
        let (_env, _temp, root, conn) = fixture();
        crate::store::open_task_store(&root).unwrap()
            .add(&cas_types::Task::new("cas-7435".into(), "peer boundary".into())).unwrap();
        let supervisor = cas_types::Agent::new_with_role(
            "sup-soundwave".into(), "cosmic-shark-29".into(), cas_types::AgentRole::Supervisor,
        );
        crate::store::open_agent_store(&root).unwrap().register(&supervisor).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        sent_ask(&conn, "ask-7435", "Please relay the file boundary for cas-7435 before merging.");
        let before = core.worktree_merge("factory/missing-worker", false, Some("cas-7435"), false, Some(false), false, None)
            .await.expect_err("the unanswered ask must hold the real MCP merge path");

        conn.execute(
            "INSERT INTO prompt_queue (source, target, prompt) VALUES (?1, ?2, ?3)",
            rusqlite::params!["peer:lively-cheetah-57@prowl", "cosmic-shark-29",
                "<cas-peer-message v=\"1\" id=\"answer-7435\" from=\"lively-cheetah-57\" from_agent=\"p\" machine=\"prowl\" repo=\"cassy\" kind=\"answer\"/>\nRelayed the file boundary."],
        ).unwrap();
        let after = core.worktree_merge("factory/missing-worker", false, Some("cas-7435"), false, Some(false), false, None)
            .await.expect_err("the missing source remains subject to normal merge guards");
    }

    #[test]
    fn peer_answers_match_only_their_unambiguous_question_cas_7435() {
        use crate::cloud::peer_mailbox::{PeerEnvelope, PeerMessageKind, PeerScope, render_envelope};
        for case in ["single", "named", "explicit", "linked-successor", "ambiguous", "wrong-peer", "wrong-target",
            "wrong-reference", "stale", "receipt", "status", "untyped", "prefix",
            "ambiguous-then-explicit", "duplicate", "queued", "wrong-source"]
        {
            let (_env, _temp, root, conn) = fixture();
            sent_ask(&conn, "ask-7435", "Please review PR #4249.");
            let two_asks = matches!(case, "named" | "explicit" | "linked-successor" | "ambiguous" | "prefix" | "ambiguous-then-explicit" | "duplicate");
            if two_asks {
                sent_ask(&conn, "ask-74350", "Please review PR #4250.");
            }
            conn.execute("UPDATE peer_message_outbox SET payload=json_set(payload, '$.recipient_agent_id', 'peer-agent'), created_at=strftime('%s','now')-10", []).unwrap();
            if case == "queued" {
                conn.execute("UPDATE peer_message_outbox SET cloud_id=NULL, state='pending'", []).unwrap();
            }
            let mut reply = PeerEnvelope {
                id: "reply-7435".into(), sender_name: "lively-cheetah-57".into(),
                sender_agent_id: "peer-agent".into(), machine: "prowl".into(),
                session: None, repo: "cassy".into(), in_reply_to: None,
                kind: Some(PeerMessageKind::Answer), scope: PeerScope::Project,
            };
            match case {
                "explicit" => reply.in_reply_to = Some("ask-7435".into()),
                "linked-successor" => {
                    // Explicit Cloud reply links survive supervisor succession.
                    reply.in_reply_to = Some("ask-7435".into());
                    reply.sender_agent_id = "successor-peer".into();
                    reply.sender_name = "successor-supervisor".into();
                }
                "wrong-reference" => reply.in_reply_to = Some("unrelated-ask".into()),
                "wrong-peer" => reply.sender_agent_id = "different-peer".into(),
                "receipt" => reply.kind = Some(PeerMessageKind::Receipt),
                "status" => reply.kind = Some(PeerMessageKind::Status),
                "untyped" => reply.kind = None,
                _ => {}
            }
            let insert = |reply: &PeerEnvelope, body: &str| {
                // pin: The peer queue source is a protocol identity; a conflicting source cannot authorize an answer.
                let source = if case == "wrong-source" { "peer:someone-else@prowl".into() }
                    else { format!("peer:{}@{}", reply.sender_name, reply.machine) };
                let target = if case == "wrong-target" { "another-supervisor" } else { "cosmic-shark-29" };
                conn.execute("INSERT INTO prompt_queue (source, target, prompt) VALUES (?1, ?2, ?3)",
                    rusqlite::params![source, target, format!("{}\n{body}", render_envelope(reply))]).unwrap();
            };
            insert(&reply, match case {
                "named" => "Reviewed ask-7435; approved.",
                "prefix" | "duplicate" => "Reviewed ask-74350; approved.",
                _ => "Relayed the boundary; approved.",
            });
            if case == "stale" {
                conn.execute("UPDATE prompt_queue SET created_at=datetime('now','-60 seconds')", []).unwrap();
            }
            if case == "ambiguous-then-explicit" {
                reply.id = "explicit-second-answer".into();
                reply.in_reply_to = Some("ask-74350".into());
                insert(&reply, "Second question approved.");
            } else if case == "duplicate" {
                insert(&reply, "A duplicate delivery cannot answer the remaining question.");
            }
            let refusal = pr_merge(&root, "sup-soundwave", 4249);
            assert_eq!(refusal.is_none(), matches!(case, "single" | "named" | "explicit" | "linked-successor"), "{case}: {refusal:?}");
            if matches!(case, "ambiguous" | "ambiguous-then-explicit" | "wrong-reference" | "queued") {
                assert!(refusal.as_deref().unwrap().contains("reply-7435"), "{case}: {refusal:?}");
            }
            if two_asks {
                let other_answered = matches!(case, "prefix" | "ambiguous-then-explicit" | "duplicate");
                assert_eq!(pr_merge(&root, "sup-soundwave", 4250).is_none(), other_answered, "other ask in {case}");
            }
        }
    }

    /// Verifiers ver-37f17bf1f171 and ver-aaa79541a589: any way an ask writes
    /// the PR number holds that PR; a different number does not.
    #[test]
    fn every_pr_reference_form_in_an_ask_holds_its_merge_cas_bddf9() {
        let (_env, _temp, root, conn) = fixture();
        let cases = [
            ("PR#4249", true),
            ("gabber-studio#4249", true),
            ("Richards-LLC/gabber-studio#4249", true),
            ("pull request 4249", true),
            ("PR-4249", true),
            ("PRs 4249 and 4250", true),
            ("PR 4250 and 4249", true),
            ("PR number 4249", true),
            ("pull requests 4249", true),
            ("pull-request 4249", true),
            ("PR (4249)", true),
            ("PR #42490", false),
        ];
        for (index, (form, holds)) in cases.iter().enumerate() {
            let sender = format!("sup-{index}");
            sent_ask_from(&conn, &sender, &format!("ask-{index}"), &format!("Please review {form} tonight."));
            assert_eq!(pr_merge(&root, &sender, 4249).is_some(), *holds, "{form}");
        }
    }

    #[test]
    fn only_explicit_asks_hold_a_merge_cas_5ec1() {
        let (_env, _temp, root, conn) = fixture();
        for (index, kind) in [None, Some("blocker"), Some("status"), Some("ask")].into_iter().enumerate() {
            let sender = format!("sup-kind-{index}");
            sent_message_from(&conn, &sender, &format!("message-{index}"), "Context for PR #4249", kind);
            assert_eq!(pr_merge(&root, &sender, 4249).is_some(), kind == Some("ask"), "kind={kind:?}");
        }
    }

    /// A typed answer can resolve the sole open question without a reply link;
    /// untyped follow-ups, status, receipts and split decisions keep its hold.
    #[test]
    fn only_a_correlated_answer_from_the_same_peer_clears_an_ask_cas_5ec1() {
        use crate::cloud::peer_mailbox::{PeerEnvelope, PeerMessageKind, PeerScope, render_envelope};
        let cases = [
            (None, "p", "cosmic-shark-29", "now", true),
            (Some(PeerMessageKind::Answer), "p", "cosmic-shark-29", "now", false),
            (Some(PeerMessageKind::Status), "p", "cosmic-shark-29", "now", true),
            (Some(PeerMessageKind::Receipt), "p", "cosmic-shark-29", "now", true),
            (Some(PeerMessageKind::EpicSplitDecided), "p", "cosmic-shark-29", "now", true),
            (None, "other-peer", "cosmic-shark-29", "now", true),
            (None, "p", "other-supervisor", "now", true),
            (None, "p", "cosmic-shark-29", "-2 minutes", true),
        ];
        for (kind, peer, target, time, holds) in cases {
            let (_env, _temp, root, conn) = fixture();
            sent_ask(&conn, "ask-review", "Can you review PR #4249?");
            assert!(pr_merge(&root, "sup-soundwave", 4249).is_some());
            let envelope = PeerEnvelope {
                id: "new-message".into(),
                sender_name: "lively-cheetah-57".into(),
                sender_agent_id: peer.into(),
                machine: "prowl".into(),
                session: None,
                repo: "gabber-studio".into(),
                in_reply_to: None,
                kind,
                scope: PeerScope::Project,
            };
            conn.execute(
                "INSERT INTO prompt_queue (source, target, prompt, created_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                rusqlite::params![format!("peer:{}@prowl", envelope.sender_name), target,
                    format!("{}\nThe review or split is decided", render_envelope(&envelope)),
                    if time == "now" { "+0 seconds" } else { time }],
            ).unwrap();
            assert_eq!(pr_merge(&root, "sup-soundwave", 4249).is_some(), holds,
                "kind={kind:?}, peer={peer}, target={target}, time={time}");
        }
    }

    #[test]
    fn an_ask_about_another_task_does_not_hold_a_merge_cas_5ec1() {
        let (_env, _temp, root, conn) = fixture();
        sent_ask(&conn, "ask-task", "Please review cas-task-x before merging it");
        for (task, holds) in [("cas-task-x", true), ("cas-task-y", false), ("cas-task-xy", false)] {
            assert_eq!(merge_refusal(&root, "sup-soundwave", "cosmic-shark-29", None,
                &MergeSubject { task_id: Some(task.into()), ..Default::default() }).is_some(), holds, "task={task}");
        }
    }

    /// cas-6b42 (GH #1634): prowl answered several asks about cas-aa10 in one
    /// reply linked to a sibling ask. That answer clears every earlier open
    /// ask to the same peer about the same tasks; an ask about another task
    /// stays open.
    #[test]
    fn a_linked_answer_clears_sibling_asks_about_the_same_tasks_cas_6b42() {
        let (_env, _temp, root, conn) = fixture();
        sent_ask(&conn, "ask-handover", "Is cas-aa10 yours to merge as train owner?");
        sent_ask(&conn, "ask-other", "Status of cas-bb20?");
        sent_ask(&conn, "ask-queues", "Which worker re-parks cas-aa10?");
        conn.execute(
            "INSERT INTO prompt_queue (source, target, prompt) VALUES ('peer:lively-cheetah-57@prowl', 'cosmic-shark-29',
             '<cas-peer-message v=\"1\" id=\"r1\" from=\"lively-cheetah-57\" from_agent=\"p\" machine=\"prowl\" repo=\"cassy\" in_reply_to=\"ask-queues\" kind=\"answer\"/>
lively-eagle-82 re-parked cas-aa10; yours to merge.')",
            [],
        )
        .unwrap();
        let merge = |task: &str| merge_refusal(&root, "sup-soundwave", "cosmic-shark-29", None,
            &MergeSubject { task_id: Some(task.into()), ..Default::default() });
        assert!(merge("cas-aa10").is_none(), "{:?}", merge("cas-aa10"));
        let refused = merge("cas-bb20").expect("an ask about another task still holds its merge");
        assert!(refused.contains("ask-other"), "{refused}");
    }

    /// Verifier ver-37f17bf1f171 (2): `gh pr merge` with a branch or with no
    /// argument resolves its PR like the QA merge guard and is held too.
    #[test]
    fn a_branch_or_bare_pr_merge_is_held_by_its_ask_cas_bddf9() {
        let (mut env, temp, root, conn) = fixture();
        sent_ask(&conn, "ask-branch", "Please review PR #4249 before staging.");
        let gh = temp.path().join("fake-gh");
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nprintf '%s' '{\"number\":4249}'\n");
        env.set(crate::qa_pass::github_gate::GH_BIN_ENV, &gh);
        for command in ["gh pr merge release/rebuild-batch-22 --merge", "gh pr merge --squash"] {
            let refused = github_merge_refusal(&root, temp.path(), command, "sup-soundwave", "cosmic-shark-29", None)
                .unwrap_or_else(|| panic!("{command} must be held"));
            assert!(refused.contains("ask-branch"), "{refused}");
        }
    }
}
