//! cas-7a01 (GH #155): turn-start inbox surfacing, end to end through the hook.
//!
//! The bug these cover: a non-urgent message delivered to an idle Claude worker
//! was written to an inbox file and never surfaced — including across turns the
//! worker took. Two consecutive supervisor messages were lost that way, and
//! only an urgent interrupt (a structurally different, inbox-bypassing path)
//! ever reached the worker.
//!
//! These tests drive the real `UserPromptSubmit` handler against a real store,
//! because the failure was never in one component: delivery worked, the queue
//! worked, and there was simply no code path from the queue back to a turn.

use cas_core::hooks::types::{HookInput, HookSpecificOutput};
use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
use tempfile::TempDir;

use crate::hooks::handlers::handle_user_prompt_submit;

const SESSION: &str = "cas-src-happy-jay-91";
const WORKER: &str = "ready-cheetah-71";

/// Restores every factory identity var this module sets, whatever the test does.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
        let mut saved = Vec::new();
        for (key, value) in vars {
            saved.push((*key, std::env::var(key).ok()));
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn worker_env() -> EnvGuard {
    EnvGuard::set(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_AGENT_NAME", Some(WORKER)),
        ("CAS_FACTORY_SESSION", Some(SESSION)),
    ])
}

fn supervisor_env() -> EnvGuard {
    EnvGuard::set(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("loyal-bear-96")),
        ("CAS_FACTORY_SESSION", Some(SESSION)),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
    ])
}

fn input(role: &str) -> HookInput {
    HookInput {
        session_id: "hook-test-session".to_string(),
        cwd: "/test".to_string(),
        hook_event_name: "UserPromptSubmit".to_string(),
        user_prompt: Some("continuing my work".to_string()),
        agent_role: Some(role.to_string()),
        ..HookInput::default()
    }
}

fn store_at(dir: &TempDir) -> SqlitePromptQueueStore {
    let store = SqlitePromptQueueStore::open(dir.path()).unwrap();
    store.init().unwrap();
    store
}

fn context_of(output: &cas_core::hooks::types::HookOutput) -> String {
    match &output.hook_specific_output {
        Some(HookSpecificOutput::UserPromptSubmit { additional_context }) => {
            additional_context.clone()
        }
        other => panic!("expected UserPromptSubmit additionalContext, got {other:?}"),
    }
}

/// AC3, the headline: a worker taking ANY turn after a transport-delivered
/// non-urgent message sees it at that turn's start.
#[test]
fn a_worker_turn_surfaces_a_delivered_non_urgent_message() {
    let _lock = super::env_lock();
    let _env = worker_env();
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);

    let id = store
        .enqueue_with_session("supervisor", WORKER, "start cas-7a01 now", SESSION)
        .unwrap();
    store.mark_transport_delivered(id).unwrap();

    let output = handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap();
    let context = context_of(&output);
    assert!(
        context.contains("start cas-7a01 now"),
        "a delivered message must open the worker's next turn: {context}"
    );
    assert!(
        context.contains("supervisor"),
        "the surfaced message must name its sender: {context}"
    );
}

/// AC2, the reproduction: the incident's messages landed seconds AFTER the
/// worker drained its inbox to "No unread messages". The drain found nothing
/// because nothing had arrived yet — and before this fix the row that arrived
/// next had no path to the worker at all.
#[test]
fn a_message_arriving_just_after_a_drain_surfaces_at_the_next_turn() {
    let _lock = super::env_lock();
    let _env = worker_env();
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);

    let drained = store
        .poll_unseen_for_recipient(WORKER, Some(SESSION), 10)
        .unwrap();
    assert!(drained.is_empty(), "precondition: the inbox drained empty");

    let id = store
        .enqueue_with_session("supervisor", WORKER, "post-drain instruction", SESSION)
        .unwrap();
    store.mark_transport_delivered(id).unwrap();

    let context =
        context_of(&handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap());
    assert!(
        context.contains("post-drain instruction"),
        "the post-drain race must be covered: {context}"
    );
}

/// The GH #124 / cas-ceae storm guard at the handler level: a message the
/// worker has already been shown must never be injected into a later turn.
#[test]
fn a_surfaced_message_does_not_repeat_on_the_next_turn() {
    let _lock = super::env_lock();
    let _env = worker_env();
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store
        .enqueue_with_session("supervisor", WORKER, "only once", SESSION)
        .unwrap();

    let first =
        context_of(&handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap());
    assert!(first.contains("only once"));

    let second = handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap();
    let repeated = second
        .user_prompt_context()
        .is_some_and(|c| c.contains("only once"));
    assert!(
        !repeated,
        "re-injecting an already-surfaced message every turn is the #124 storm"
    );
}

/// Two messages queued between turns must BOTH arrive — the incident lost two
/// consecutive supervisor messages, so surfacing one of them is not a fix.
#[test]
fn consecutive_messages_all_surface_in_one_turn() {
    let _lock = super::env_lock();
    let _env = worker_env();
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store
        .enqueue_with_session("supervisor", WORKER, "first instruction", SESSION)
        .unwrap();
    store
        .enqueue_with_session("supervisor", WORKER, "second instruction", SESSION)
        .unwrap();

    let context =
        context_of(&handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap());
    assert!(context.contains("first instruction"), "{context}");
    assert!(context.contains("second instruction"), "{context}");
}

/// The supervisor's early return was the quieter half of the same bug: it made
/// the supervisor the one factory role whose mail could never surface here.
/// The reminder must now APPEND to the mail, not replace it.
#[test]
fn the_supervisor_reminder_appends_instead_of_suppressing_mail() {
    let _lock = super::env_lock();
    let _env = supervisor_env();
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store
        .enqueue_with_session(
            "worker-a",
            "supervisor",
            "MERGE REQUIRED for cas-7a01",
            SESSION,
        )
        .unwrap();

    let context =
        context_of(&handle_user_prompt_submit(&input("supervisor"), Some(temp.path())).unwrap());
    assert!(
        context.contains("[supervisor reminder]"),
        "the cas-55ac reminder must survive: {context}"
    );
    assert!(
        context.contains("MERGE REQUIRED for cas-7a01"),
        "supervisor-bound mail must surface alongside the reminder: {context}"
    );
}

/// A supervisor with no mail must still get exactly the reminder it always got.
#[test]
fn the_supervisor_reminder_is_unchanged_when_there_is_no_mail() {
    let _lock = super::env_lock();
    let _env = supervisor_env();
    let temp = TempDir::new().unwrap();
    let _store = store_at(&temp);

    let context =
        context_of(&handle_user_prompt_submit(&input("supervisor"), Some(temp.path())).unwrap());
    assert!(context.contains("[supervisor reminder]"));
    assert!(
        !context.contains("[incoming messages]"),
        "an empty inbox must not announce itself: {context}"
    );
}

/// Session isolation must hold on the surfacing path exactly as it does on the
/// drain: another factory session's mail is not this worker's mail.
#[test]
fn another_sessions_message_is_not_surfaced() {
    let _lock = super::env_lock();
    let _env = worker_env();
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store
        .enqueue_with_session(
            "supervisor",
            WORKER,
            "other lane work",
            "a-different-session",
        )
        .unwrap();

    let output = handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap();
    let surfaced = output
        .user_prompt_context()
        .is_some_and(|c| c.contains("other lane work"));
    assert!(!surfaced, "cross-session leakage on the surfacing path");
}

/// A non-factory session must be untouched: no identity, no queue read, no
/// injected context. This handler also runs for every solo Claude session.
#[test]
fn a_non_factory_session_surfaces_nothing() {
    let _lock = super::env_lock();
    let _env = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", None),
        ("CAS_AGENT_NAME", Some(WORKER)),
        ("CAS_FACTORY_SESSION", Some(SESSION)),
    ]);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store
        .enqueue_with_session("supervisor", WORKER, "factory-only traffic", SESSION)
        .unwrap();

    let mut solo = input("worker");
    solo.agent_role = None;
    let output = handle_user_prompt_submit(&solo, Some(temp.path())).unwrap();
    let surfaced = output
        .user_prompt_context()
        .is_some_and(|c| c.contains("factory-only traffic"));
    assert!(!surfaced, "a solo session must not drain factory queues");
}
