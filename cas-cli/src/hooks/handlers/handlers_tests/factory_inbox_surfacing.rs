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

use cas_core::hooks::types::{
    HookInput, HookSpecificOutput, MachinePromptOrigin, MachinePromptProvenance,
};
use cas_mux::SupervisorCli;
use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
use tempfile::TempDir;

use crate::hooks::handlers::handle_user_prompt_submit;
use crate::test_support::TestEnvGuard;
use crate::types::{Entry, EntryType, Task, TaskStatus};

const SESSION: &str = "cas-src-happy-jay-91";
const WORKER: &str = "ready-cheetah-71";

fn worker_env(env: &mut TestEnvGuard) {
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("CAS_AGENT_NAME", WORKER);
    env.set("CAS_FACTORY_SESSION", SESSION);
}

fn supervisor_env(env: &mut TestEnvGuard) {
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_AGENT_NAME", "loyal-bear-96");
    env.set("CAS_FACTORY_SESSION", SESSION);
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
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

/// The receipt ledger is the authoritative answer to whether a particular
/// recipient alias has seen a row.  Check it without draining: a drain would
/// create the very receipt this wiring test needs to prove already exists.
fn assert_receipted_for_every_alias(store: &SqlitePromptQueueStore, aliases: &[String]) {
    for alias in aliases {
        assert_eq!(
            store
                .count_unseen_for_recipient(alias, Some(SESSION))
                .unwrap(),
            0,
            "the hook must write a receipt for supervisor alias {alias}"
        );
    }
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
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
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
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
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
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
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
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
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
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
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

/// cas-53a7: receipt mirroring is a production-path obligation, not merely a
/// helper contract.  The hook surfaces this broadcast through the pane-name
/// alias first; it must then retire the same row for the logical `supervisor`
/// alias too, so the other inbox reader cannot re-inject it on a later turn.
#[test]
fn supervisor_turn_writes_receipts_for_every_inbox_alias() {
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    let aliases = crate::harness_policy::inbox_aliases("loyal-bear-96", true);
    assert!(
        aliases.len() > 1,
        "precondition: this must exercise the supervisor's full alias set"
    );

    // Fill the first alias's surface limit. This is the live shape in which a
    // broadcast never reaches the second alias reader in this turn, so only
    // the hook's explicit mirroring can write that alias's receipts.
    for index in 0..10 {
        store
            .enqueue_with_session(
                "director",
                "all_workers",
                &format!("receipt must mirror through the turn-start hook #{index}"),
                SESSION,
            )
            .unwrap();
    }

    let context =
        context_of(&handle_user_prompt_submit(&input("supervisor"), Some(temp.path())).unwrap());
    assert!(
        context.contains("receipt must mirror through the turn-start hook #0"),
        "precondition: the real hook must surface the broadcast: {context}"
    );
    assert_receipted_for_every_alias(&store, &aliases);
}

/// A supervisor with no mail must still get exactly the reminder it always got.
#[test]
fn the_supervisor_reminder_is_unchanged_when_there_is_no_mail() {
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
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

#[test]
fn supervisor_terminal_input_enters_history_but_harness_envelopes_do_not() {
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    let mut typed = input("supervisor");
    typed.user_prompt = Some("Please check the CAS wake: logs and the desktop reply".into());
    handle_user_prompt_submit(&typed, Some(temp.path())).unwrap();
    for harness_prompt in [
        "[cas #123 operator Daniel@Pixel verified 0s first] relay",
        "[supervisor reminder] generated",
        "[lifecycle wake] generated",
        "[system-reminder] generated",
        "<task-notification>\n<task-id>bqa23xyz</task-id>\n<status>completed</status>\n</task-notification>",
        "Another Claude session sent a message:\n<teammate-message teammate_id=\"director\">\n[cas #21 operator Daniel@Pixel 10 verified 0s first] relay\n</teammate-message>",
        "<teammate-message teammate_id=\"director\">relay</teammate-message>",
        "CAS wake: message 26297 from supervisor is in your inbox",
        "CAS provenance: machine delivery from supervisor",
        "<system-reminder>generated context</system-reminder>",
    ] {
        typed.user_prompt = Some(harness_prompt.into());
        handle_user_prompt_submit(&typed, Some(temp.path())).unwrap();
    }
    typed.user_prompt = Some("A machine relay with an ordinary looking first line\n[cas #22 operator Daniel@Pixel verified 0s first] relay".into());
    handle_user_prompt_submit(&typed, Some(temp.path())).unwrap();
    typed.user_prompt = Some("Looks like an ordinary line".into());
    typed.machine_prompt_provenance = Some(MachinePromptProvenance {
        notification_id: 9,
        origin: MachinePromptOrigin::AgentAuthored,
        queued_at: "2026-09-29T16:00:00Z".into(),
        delivery: "first-delivery".into(),
    });
    handle_user_prompt_submit(&typed, Some(temp.path())).unwrap();
    let history = store
        .conversation_history(SESSION, "paired-device", None, 10)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source, "terminal");
    assert_eq!(
        history[0].prompt,
        "Please check the CAS wake: logs and the desktop reply"
    );
    assert!(history[0].processed_at.is_some());
}

/// cas-0337: this is the actual factory supervisor turn path, not a direct
/// retriever unit seam. A long active-task title must not hide a same-day
/// Learning whose vocabulary is present in the submitted turn.
#[test]
fn supervisor_turn_surfaces_matching_same_day_learnings() {
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
    let project = TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    let entries = crate::store::open_store_local(&cas_root).unwrap();
    let mut stale_refs = Entry::new(
        "2026-08-14-5".into(),
        "epic_status compares child branches against stale local main; fast-forward origin/main before trusting its unmerged report".into(),
    );
    stale_refs.entry_type = EntryType::Learning;
    stale_refs.importance = 0.85;
    entries.add(&stale_refs).unwrap();
    let mut hermetic_ci = Entry::new(
        "2026-08-14-3".into(),
        "Local green is insufficient merge evidence: a populated developer HOME masks inherited git identity and CAS_AGENT_NAME failures that clean CI exposes".into(),
    );
    hermetic_ci.entry_type = EntryType::Learning;
    hermetic_ci.importance = 0.90;
    entries.add(&hermetic_ci).unwrap();

    let tasks = crate::store::open_task_store_local(&cas_root).unwrap();
    let mut task = Task::new(
        "cas-ambient".into(),
        "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima".into(),
    );
    task.status = TaskStatus::InProgress;
    tasks.add(&task).unwrap();

    let mut stale_input = input("supervisor");
    stale_input.cwd = project.path().to_string_lossy().into_owned();
    stale_input.user_prompt =
        Some("Investigate epic_status before trusting stale refs on main".into());
    let stale_context =
        context_of(&handle_user_prompt_submit(&stale_input, Some(&cas_root)).unwrap());
    assert!(stale_context.contains("2026-08-14-5"), "{stale_context}");

    let mut ci_input = stale_input;
    ci_input.user_prompt =
        Some("Local green diverged from clean CI; audit inherited HOME, git identity, and CAS_AGENT_NAME".into());
    let ci_context = context_of(&handle_user_prompt_submit(&ci_input, Some(&cas_root)).unwrap());
    assert!(ci_context.contains("2026-08-14-3"), "{ci_context}");
}

/// cas-b657 (GH #375): the real UserPromptSubmit handler must preserve useful
/// operator context in a factory worker session while rejecting every known
/// machine origin. The test parses the actual hook wire shape and delivers it
/// through the factory authority: hook dispatch enriches the raw prompt from
/// typed sidecar metadata, never a marker parsed out of rendered display text.
#[test]
fn factory_worker_captures_operator_context_but_not_typed_machine_relays() {
    let mut env = TestEnvGuard::new();
    let project = TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();

    {
        worker_env(&mut env);
        env.set("CAS_ROOT", &cas_root);
        for (source, origin) in [
            ("supervisor", MachinePromptOrigin::AgentAuthored),
            (
                "lifecycle-wake:worker-idle",
                MachinePromptOrigin::LifecycleRelay,
            ),
            ("director", MachinePromptOrigin::DirectorGenerated),
        ] {
            assert_eq!(
                crate::hooks::delivery_provenance::origin_for_source(source),
                origin
            );
            let rendered = format!(
                "[cas #123 {} 0s first]\nGitHub check-run updatedAt changes only on state transitions; use status/conclusion and a known wall-clock bound before declaring CI stalled.",
                match origin {
                    MachinePromptOrigin::AgentAuthored => "agent-authored",
                    MachinePromptOrigin::LifecycleRelay => "lifecycle-relay",
                    MachinePromptOrigin::DirectorGenerated => "director-generated",
                }
            );
            let payload =
                crate::ui::factory::daemon::runtime::delivery::prepare_pty_machine_delivery(
                    &cas_root,
                    WORKER,
                    SupervisorCli::Codex,
                    source,
                    &rendered,
                    Some(123),
                );
            let input: HookInput = serde_json::from_value(serde_json::json!({
                "session_id": "hook-test-session",
                "cwd": project.path(),
                "hook_event_name": "UserPromptSubmit",
                "prompt": payload,
                "agent_role": "worker"
            }))
            .expect("factory machine delivery hook payload must deserialize");
            crate::hooks::handle_hook("UserPromptSubmit", input).unwrap();
        }

        let operator: HookInput = serde_json::from_value(serde_json::json!({
            "session_id": "hook-test-session",
            "cwd": project.path(),
            "hook_event_name": "UserPromptSubmit",
            "prompt": "Please investigate why factory relay provenance vanishes before prompt capture and preserve the operator instruction as durable context.",
            "agent_role": "worker"
        }))
        .expect("operator hook payload must deserialize without provenance");
        assert!(
            operator.machine_prompt_provenance.is_none(),
            "absence is the deliberate operator signal"
        );
        crate::hooks::handle_hook("UserPromptSubmit", operator).unwrap();
    }

    let prompt_store = crate::store::open_prompt_store(&cas_root).unwrap();
    assert_eq!(
        prompt_store
            .list_by_session("hook-test-session", 10)
            .unwrap()
            .len(),
        4,
        "machine relays and operator turns remain complete for attribution"
    );

    let entries = crate::store::open_store_local(&cas_root)
        .unwrap()
        .list()
        .unwrap();
    assert!(
        entries.iter().any(|entry| {
            entry.entry_type == EntryType::Context
                && entry.content.contains("preserve the operator instruction")
        }),
        "an absent typed provenance field is the explicit operator case: {entries:#?}"
    );
    assert!(
        entries
            .iter()
            .all(|entry| !entry.content.contains("updatedAt changes only")),
        "all three typed machine origins must be excluded without parsing their text: {entries:#?}"
    );
}

/// Session isolation must hold on the surfacing path exactly as it does on the
/// drain: another factory session's mail is not this worker's mail.
#[test]
fn another_sessions_message_is_not_surfaced() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
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
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    env.remove("CAS_AGENT_ROLE");
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

/// cas-b8f6: read-first turns recover mail even when UserPromptSubmit is absent.
#[test]
fn post_tool_read_recovers_mail_once_without_prompt_hook() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store
        .enqueue_with_session("supervisor", WORKER, "recover missing hook mail", SESSION)
        .unwrap();
    let mut hook = input("worker");
    hook.hook_event_name = "PostToolUse".into();
    hook.tool_name = Some("Read".into());
    let transcript = temp.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        r#"{"type":"user","promptId":"p","message":{"content":"continue"}}"#,
    )
    .unwrap();
    hook.transcript_path = Some(transcript.to_string_lossy().into_owned());
    let output = crate::hooks::handle_post_tool_use(&hook, Some(temp.path())).unwrap();
    let Some(HookSpecificOutput::PostToolUse {
        additional_context: Some(additional_context),
    }) = output.hook_specific_output
    else {
        panic!("read-first PostToolUse must surface queued mail");
    };
    assert!(additional_context.contains("recover missing hook mail"));
    let second = crate::hooks::handle_post_tool_use(&hook, Some(temp.path())).unwrap();
    assert!(second.hook_specific_output.is_none());
    assert_receipted_for_every_alias(&store, &[WORKER.into()]);
}

/// Exact 2.1.265 headless payload shape captured during the live investigation.
#[test]
fn captured_265_prompt_payload_reaches_handler_and_records_turn() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let temp = TempDir::new().unwrap();
    env.set("CAS_ROOT", temp.path());
    let store = store_at(&temp);
    store
        .enqueue_with_session("supervisor", WORKER, "captured payload mail", SESSION)
        .unwrap();
    let payload = include_str!(
        "../../../../../crates/cas-core/src/hooks/fixtures/claude-2.1.265-user-prompt-submit.json"
    );
    let input: HookInput = serde_json::from_str(payload).unwrap();
    assert_eq!(
        input.prompt_id.as_deref(),
        Some("695ddcaa-d5b2-4d95-9bdd-4f20a4ed0923")
    );
    let output = crate::hooks::handle_hook("UserPromptSubmit", input).unwrap();
    assert!(context_of(&output).contains("captured payload mail"));
    assert_eq!(
        crate::hooks::turn_context::silent_prompt_count(temp.path()),
        0
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("turn-context"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn post_tool_second_prompt_recovers_recall_and_skips_repeated_tools() {
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
    let project = TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    let entries = crate::store::open_store_local(&cas_root).unwrap();
    let mut memory = Entry::new("recall-hook-recovery".into(), "epic_status compares child branches against stale local main; fast-forward origin/main before trusting its unmerged report".into());
    memory.entry_type = EntryType::Learning;
    memory.importance = 0.95;
    entries.add(&memory).unwrap();
    let transcript = project.path().join("session.jsonl");
    let mut hook = input("supervisor");
    hook.cwd = project.path().to_string_lossy().into_owned();
    hook.transcript_path = Some(transcript.to_string_lossy().into_owned());
    hook.prompt_id = Some("first".into());
    crate::hooks::turn_context::record_prompt_hook(&cas_root, &hook);
    std::fs::write(&transcript, format!("{{\"type\":\"user\",\"sessionId\":\"{}\",\"promptId\":\"second\",\"message\":{{\"content\":\"Investigate epic_status before trusting stale refs on main\"}}}}\n", hook.session_id)).unwrap();
    hook.hook_event_name = "PostToolUse".into();
    hook.tool_name = Some("Read".into());
    let output = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    let Some(HookSpecificOutput::PostToolUse {
        additional_context: Some(additional_context),
    }) = output.hook_specific_output
    else {
        panic!("missing recovered recall");
    };
    assert!(
        additional_context.contains("recall-hook-recovery"),
        "{additional_context}"
    );
    assert_eq!(
        crate::hooks::turn_context::silent_prompt_count(&cas_root),
        1
    );
    let repeated = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    assert!(repeated.hook_specific_output.is_none());
    assert_eq!(
        crate::hooks::turn_context::silent_prompt_count(&cas_root),
        1
    );
    hook.prompt_id = Some("second".into());
    crate::hooks::turn_context::record_prompt_hook(&cas_root, &hook);
    assert_eq!(
        crate::hooks::turn_context::silent_prompt_count(&cas_root),
        0
    );
}

/// cas-b5e4 (GH #989): a peer reply reached a busy worker mid-turn and sat
/// delivered while the worker reported "no answer" — turn-start surfacing had
/// already run and the after-tool path ran at most once per turn. The next
/// tool boundary must now put it in the worker's context, exactly once, and
/// never replay the message that started the turn.
#[test]
fn busy_worker_sees_a_mid_turn_message_at_the_next_tool_boundary_cas_b5e4() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let project = TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    let store = store_at_root(&cas_root);

    // The turn was started by an injected supervisor message, before which
    // its row was handed off.
    let turn_prompt = "please coordinate with gold-fox on the schema";
    let injected = store
        .enqueue_with_session("supervisor", WORKER, turn_prompt, SESSION)
        .unwrap();
    store.mark_transport_delivered(injected).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let turn_started = chrono::Utc::now();
    let transcript = project.path().join("session.jsonl");
    let mut hook = input("worker");
    hook.cwd = project.path().to_string_lossy().into_owned();
    hook.transcript_path = Some(transcript.to_string_lossy().into_owned());
    hook.prompt_id = Some("turn-1".into());
    std::fs::write(
        &transcript,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"{}\",\"promptId\":\"turn-1\",\"timestamp\":\"{}\",\"message\":{{\"content\":\"{turn_prompt}\"}}}}\n",
            hook.session_id,
            turn_started.to_rfc3339(),
        ),
    )
    .unwrap();
    // The normal prompt hook served this turn.
    crate::hooks::turn_context::record_prompt_hook(&cas_root, &hook);
    hook.hook_event_name = "PostToolUse".into();
    hook.tool_name = Some("Read".into());

    let quiet = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    assert!(
        quiet.hook_specific_output.is_none(),
        "the turn's own injected prompt is never replayed: {:?}",
        quiet.hook_specific_output
    );

    // Mid-turn, the peer's reply is enqueued and handed off to the worker.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let reply = store
        .enqueue_with_session("gold-fox", WORKER, "schema agreed: use v2 columns", SESSION)
        .unwrap();
    store.mark_transport_delivered(reply).unwrap();

    let output = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    let Some(HookSpecificOutput::PostToolUse {
        additional_context: Some(context),
    }) = output.hook_specific_output
    else {
        panic!("the mid-turn reply must surface at the next tool boundary");
    };
    assert!(
        context.contains("schema agreed: use v2 columns"),
        "{context}"
    );
    assert!(
        context.contains("arrived while you were working"),
        "{context}"
    );
    assert!(!context.contains(turn_prompt), "{context}");

    let again = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    assert!(
        again.hook_specific_output.is_none(),
        "a surfaced message is delivered once"
    );
}

fn store_at_root(cas_root: &std::path::Path) -> SqlitePromptQueueStore {
    let store = SqlitePromptQueueStore::open(cas_root).unwrap();
    store.init().unwrap();
    store
}

#[test]
fn cas_27ad_subagent_post_tool_does_not_consume_parent_mail() {
    let mut env = TestEnvGuard::new();
    supervisor_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    store.enqueue_with_session("worker", "supervisor", "parent-only mail", SESSION).unwrap();
    for child_identity in [true, false] {
        let mut hook = input("supervisor");
        hook.hook_event_name = "PostToolUse".into();
        hook.tool_name = Some("Bash".into());
        if child_identity {
            hook.agent_id = Some("child-123".into());
        } else {
            hook.transcript_path = Some("/tmp/session/subagents/agent-child.jsonl".into());
        }
        let surfaced = super::super::handlers_middle::factory_inbox::surface_factory_inbox_after_tool_result(
            Some(temp.path()), &hook, None);
        assert!(surfaced.is_none(), "a child consumed the parent's mail: {surfaced:?}");
        let output = crate::hooks::handle_post_tool_use(&hook, Some(temp.path())).unwrap();
        let rendered = serde_json::to_string(&output).unwrap();
        assert!(!rendered.contains("parent-only mail"), "{rendered}");
    }
    assert_eq!(store.count_unseen_for_recipient("supervisor", Some(SESSION)).unwrap(), 1);
    assert!(super::super::handlers_middle::factory_inbox::surface_factory_inbox(
        Some(temp.path()), &input("supervisor")).unwrap().contains("parent-only mail"));
}

#[test]
fn cas_27ad_first_hook_claim_ignores_queue_processing_timestamp() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    let id = store.enqueue_with_session("supervisor", WORKER, "legacy handoff", SESSION).unwrap();
    store.mark_transport_delivered(id).unwrap();
    let output = handle_user_prompt_submit(&input("worker"), Some(temp.path())).unwrap();
    let rendered = context_of(&output);
    assert!(rendered.contains("s first]"), "{rendered}");
    assert!(!rendered.contains("replay]"), "{rendered}");
    assert!(store.poll_unseen_for_recipient(WORKER, Some(SESSION), 10).unwrap().is_empty());
}

/// cas-ad92: a Claude-harness worker double. The daemon's Claude+teams
/// delivery writes the body to the teams inbox file and types a pointer wake
/// into the idle pane; Claude Code queues the teams copy and shows it only at
/// the next turn boundary, so the wake turn comes first. `take_turn` returns
/// everything the model sees in that turn: the submitted prompt plus the
/// hook's additional context.
struct ClaudeTeamsWorkerDouble<'a> {
    cas_root: &'a std::path::Path,
    teams_inbox: std::collections::VecDeque<String>,
}

impl ClaudeTeamsWorkerDouble<'_> {
    fn take_turn(&mut self, prompt: &str) -> String {
        let mut turn = input("worker");
        turn.user_prompt = Some(prompt.to_string());
        let output = handle_user_prompt_submit(&turn, Some(self.cas_root)).unwrap();
        format!("{prompt}\n{}", output.user_prompt_context().unwrap_or_default())
    }

    /// The turn boundary after the wake: Claude renders the queued copy.
    fn next_teammate_turn(&mut self) -> Option<String> {
        let body = self.teams_inbox.pop_front()?;
        Some(self.take_turn(&format!("<teammate-message teammate_id=\"supervisor\">\n{body}\n</teammate-message>")))
    }
}

/// The daemon's successful Claude+teams handoff, in its real order: claim the
/// row (cas-27ad), write the teams copy, record the post-handoff receipt and
/// the delivered stage, then type the pointer wake (never the body, cas-cdf9).
fn deliver_to_claude_worker(
    store: &SqlitePromptQueueStore,
    worker: &mut ClaudeTeamsWorkerDouble<'_>,
    id: i64,
    body: &str,
) -> String {
    assert!(store.claim_recipient_transport(id, WORKER).unwrap());
    worker.teams_inbox.push_back(body.to_string());
    store
        .record_recipient_surfaced(id, WORKER, cas_store::SurfacingSource::TransportClaimed)
        .unwrap();
    store.mark_transport_delivered(id).unwrap();
    crate::ui::factory::daemon::runtime::delivery::pointer_wake_payload("supervisor", Some(id))
}

/// cas-ad92 reproduction: the wake turn of a Claude worker carried no body.
/// The claim hid the row from the turn-start hook and from `inbox_poll`, and
/// the harness held the teams copy back, so the worker woke to "see inbox",
/// polled "No unread messages", and got the body a turn later.
#[test]
fn a_pointer_wake_turn_carries_the_body_it_names() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    let mut worker = ClaudeTeamsWorkerDouble { cas_root: temp.path(), teams_inbox: Default::default() };
    let body = "Make cas-e4e3 deterministic: assert the concurrency itself.";
    let id = store.enqueue_with_session("supervisor", WORKER, body, SESSION).unwrap();

    let wake = deliver_to_claude_worker(&store, &mut worker, id, body);
    let report = store.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.recipient_receipt, Some(cas_store::SurfacingSource::TransportClaimed));
    assert_eq!(report.wake, cas_store::ObservationStatus::Unobserved, "claimed is not rendered");

    let wake_turn = worker.take_turn(&wake);
    assert!(wake_turn.contains(body), "the wake turn must carry the body it names: {wake_turn}");

    let report = store.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.recipient_receipt, Some(cas_store::SurfacingSource::HookSurfaced));
    assert_eq!(report.wake, cas_store::ObservationStatus::Observed);

    // The teams copy still renders at the next boundary; the hook must not
    // add a second copy to it.
    let teammate_turn = worker.next_teammate_turn().unwrap();
    assert_eq!(teammate_turn.matches(body).count(), 1, "{teammate_turn}");
}

/// cas-ad92 boundaries: an ordinary turn still leaves a claimed row to its
/// transport (cas-27ad), and a wake line naming a row addressed to another
/// recipient surfaces nothing.
#[test]
fn only_a_wake_naming_this_recipients_row_takes_the_claim() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let temp = TempDir::new().unwrap();
    let store = store_at(&temp);
    let mut worker = ClaudeTeamsWorkerDouble { cas_root: temp.path(), teams_inbox: Default::default() };
    let mine = store.enqueue_with_session("supervisor", WORKER, "mine to read", SESSION).unwrap();
    let theirs = store.enqueue_with_session("supervisor", "other-worker", "not mine", SESSION).unwrap();
    assert!(store.claim_recipient_transport(theirs, "other-worker").unwrap());
    deliver_to_claude_worker(&store, &mut worker, mine, "mine to read");

    let ordinary = worker.take_turn("continuing my work");
    assert!(!ordinary.contains("mine to read"), "{ordinary}");
    let forged = worker.take_turn(&format!("CAS wake: message {theirs} from supervisor is in your inbox"));
    assert!(!forged.contains("not mine"), "{forged}");
    assert_eq!(
        store.message_delivery_report(theirs).unwrap().unwrap().recipient_receipt,
        Some(cas_store::SurfacingSource::TransportClaimed)
    );
}

/// cas-b5ad reproduction (message 4184454): a worker in one long turn keeps
/// its pane busy, so the daemon claims a supervisor decision for the Claude
/// teams transport and every wake is declined ("pane has not been silent long
/// enough"). Claude Code holds the teams copy until the turn ends. The claim
/// hid the row from each tool boundary, so the worker finished the work, and
/// sent its merge request, without ever seeing the decision. The next tool
/// boundary after the claim must surface it, once.
#[test]
fn a_never_silent_worker_sees_a_claimed_message_at_its_next_tool_boundary_cas_b5ad() {
    let mut env = TestEnvGuard::new();
    worker_env(&mut env);
    let project = TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    let store = store_at_root(&cas_root);

    let turn_prompt = "start cas-0d4f0";
    let turn_started = chrono::Utc::now();
    let transcript = project.path().join("session.jsonl");
    let mut hook = input("worker");
    hook.cwd = project.path().to_string_lossy().into_owned();
    hook.transcript_path = Some(transcript.to_string_lossy().into_owned());
    hook.prompt_id = Some("turn-1".into());
    std::fs::write(
        &transcript,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"{}\",\"promptId\":\"turn-1\",\"timestamp\":\"{}\",\"message\":{{\"content\":\"{turn_prompt}\"}}}}\n",
            hook.session_id,
            turn_started.to_rfc3339(),
        ),
    )
    .unwrap();
    crate::hooks::turn_context::record_prompt_hook(&cas_root, &hook);
    hook.hook_event_name = "PostToolUse".into();
    hook.tool_name = Some("Bash".into());
    assert!(crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap().hook_specific_output.is_none());

    // Mid-turn: the supervisor decides, and the daemon claims the row for
    // the teams transport; its wake is then declined, so nothing else moves.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let decision = "Take (B), but make verification.enabled operator-only.";
    let id = store
        .enqueue_with_session("supervisor", WORKER, decision, SESSION)
        .unwrap();
    assert!(store.claim_recipient_transport(id, WORKER).unwrap());
    store
        .record_wake_gate_decline(id, "pane has not been silent long enough")
        .unwrap();

    let output = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    let Some(HookSpecificOutput::PostToolUse {
        additional_context: Some(context),
    }) = output.hook_specific_output
    else {
        panic!("the claimed decision must surface at the worker's next tool boundary");
    };
    assert!(context.contains(decision), "{context}");
    let report = store.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.recipient_receipt, Some(cas_store::SurfacingSource::HookSurfaced));

    let again = crate::hooks::handle_post_tool_use(&hook, Some(&cas_root)).unwrap();
    assert!(again.hook_specific_output.is_none(), "a surfaced message is delivered once");
}
