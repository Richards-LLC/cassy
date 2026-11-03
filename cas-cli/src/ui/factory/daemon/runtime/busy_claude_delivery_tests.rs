//! cas-a1ed: a supervisor message to a busy Claude Teams worker.
//!
//! The pane-silence gate decides only whether the daemon types a PTY nudge.
//! The Teams inbox write, and the row's visibility at the worker's next tool
//! boundary, must not wait on it.
use std::time::{Duration, Instant};

use cas_store::PromptQueueStore;

use crate::store::{open_agent_store, open_prompt_queue_store};
use crate::ui::factory::daemon::FactoryDaemon;

const WORKER: &str = "worker-a";
const SESSION: &str = "provision-test";

/// A Claude worker pane that is ready for input but has just printed output,
/// as a pane does throughout a working turn.
async fn busy_claude_worker_pane(daemon: &mut FactoryDaemon) {
    let pty = cas_mux::Pty::spawn(
        WORKER,
        cas_mux::PtyConfig {
            command: "sh".into(),
            args: vec!["-c".into(), "printf ready; sleep 30".into()],
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .unwrap();
    let pane = cas_mux::Pane::with_pty(
        WORKER,
        cas_mux::PaneKind::Worker,
        pty,
        24,
        80,
        cas_mux::SupervisorCli::Claude,
    )
    .unwrap();
    daemon.app.mux.set_worker_cli(cas_mux::SupervisorCli::Claude);
    daemon.app.adopt_worker_pane_for_test(pane);
    let bytes = super::provisioning_tests::await_ready_fixture_pane(daemon, WORKER).await;
    daemon.last_pane_output_bytes.insert(WORKER.into(), bytes);
    daemon.pane_silent_since.insert(WORKER.into(), Instant::now());
    assert!(daemon.app.mux.pane_ready_for_injection(WORKER));
    assert_eq!(daemon.app.harness_for(WORKER), cas_mux::SupervisorCli::Claude);
}

/// The 22:08:30Z case, through the real `process_prompt_queue`: the wake is
/// declined because the pane is not silent, and the message is still written
/// to the worker's Teams inbox in the same pass. It's held for the tool
/// boundary, not dropped, and the worker's next tool call can surface it.
#[tokio::test]
async fn a_declined_wake_still_writes_the_teams_copy_and_leaves_the_row_for_the_tool_boundary() {
    exercise_held_copy(false, false).await;
}

#[tokio::test]
async fn cas_1ba0_failed_deferral_stamp_warns_supervisor_and_keeps_the_teams_copy() {
    exercise_held_copy(true, false).await;
}

#[tokio::test]
async fn cas_1ba0_busy_worker_missing_team_folder_keeps_pty_delivery_out_of_hook_drains() {
    exercise_held_copy(false, true).await;
}

async fn exercise_held_copy(fail_stamp: bool, missing_team_folder: bool) {
    let root = tempfile::tempdir().unwrap();
    let claude_config = root.path().join("claude-config");
    // The live team tree a running factory has (a missing one falls back to PTY).
    if !missing_team_folder {
        std::fs::create_dir_all(claude_config.join("teams").join(SESSION).join("inboxes")).unwrap();
    }
    let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[(
        "CLAUDE_CONFIG_DIR",
        claude_config.to_str(),
    )]);
    let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
    let mut worker = cas_types::Agent::new_with_role(
        "worker-id".into(),
        WORKER.into(),
        cas_types::AgentRole::Worker,
    );
    worker.factory_session = Some(SESSION.into());
    open_agent_store(&cas_dir)
        .unwrap()
        .register(&worker)
        .unwrap();

    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    busy_claude_worker_pane(&mut daemon).await;
    daemon.teams = Some(super::teams::TeamsManager::new(SESSION));

    let queue = open_prompt_queue_store(&cas_dir).unwrap();
    let decision = "Decision: ONE daemon JobRuntime. Drop FactoryJobs.";
    let turn_started_at = chrono::Utc::now() - chrono::Duration::minutes(20);
    let id = queue
        .enqueue_with_session("supervisor", WORKER, decision, SESSION)
        .unwrap();
    if fail_stamp {
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_deferral BEFORE UPDATE OF deferred_inbox_at ON prompt_queue
            WHEN NEW.deferred_inbox_at IS NOT NULL
            BEGIN SELECT RAISE(FAIL, 'deferral stamp fixture failure'); END;",
        )
        .unwrap();
    }
    daemon.process_prompt_queue().await.unwrap();

    let row = queue.queued_prompt(id).unwrap().unwrap();
    if missing_team_folder {
        assert!(
            queue.deferred_inbox_state(id).unwrap().is_none(),
            "a PTY fallback must never become a held Teams copy in the same attempt"
        );
        assert!(
            row.processed_at.is_some(),
            "successful PTY fallback completes transport"
        );
        assert!(!claude_config.join("teams").join(SESSION).exists());
        assert!(
            queue
                .surface_unseen_for_recipient_without_transport_delivery(WORKER, Some(SESSION), 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            queue
                .surface_unseen_for_recipient_delivered_after(
                    WORKER,
                    Some(SESSION),
                    10,
                    turn_started_at
                )
                .unwrap()
                .is_empty()
        );
        return;
    }
    assert!(
        row.processed_at.is_none(),
        "a declined wake holds the row, never drops it"
    );
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_ne!(
        report.wake_attempt,
        cas_store::WakeAttempt::Fired,
        "the busy pane must not be typed into: {report:?}"
    );
    let inbox = claude_config
        .join("teams")
        .join(SESSION)
        .join("inboxes")
        .join(format!("{WORKER}.json"));
    let written = std::fs::read_to_string(&inbox).unwrap_or_default();
    let tree: Vec<_> = walk(&claude_config);
    assert!(
        // The Teams copy is the body or, since cas-1f0c, a pointer naming it.
        written.contains(&format!("[cas #{id} ")),
        "the Teams inbox write must not wait on the silence gate: {written:?}\nfiles: {tree:?}\nreport: {report:?}"
    );

    if fail_stamp {
        assert!(queue.deferred_inbox_state(id).unwrap().is_none());
        let notices = queue
            .peek_all(20)
            .unwrap()
            .into_iter()
            .filter(|row| row.target == daemon.app.supervisor_name())
            .collect::<Vec<_>>();
        assert_eq!(
            notices.len(),
            1,
            "a failed stamp must be supervisor-visible"
        );
        assert!(notices[0].prompt.contains(&format!("notification_id={id}")));
        assert!(
            notices[0]
                .prompt
                .contains("tool-boundary surfacing unavailable")
        );
        assert!(notices[0].prompt.contains("turn boundary"));
        assert_eq!(notices[0].factory_session.as_deref(), Some(SESSION));
        assert!(
            queue
                .surface_unseen_for_recipient_delivered_after(
                    WORKER,
                    Some(SESSION),
                    10,
                    turn_started_at
                )
                .unwrap()
                .is_empty(),
            "failed deferral remains safe: only the Teams copy can surface"
        );
        return;
    }

    let surfaced = queue
        .surface_unseen_for_recipient_delivered_after(WORKER, Some(SESSION), 10, turn_started_at)
        .unwrap();
    assert_eq!(
        surfaced.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![id],
        "the worker's next tool boundary takes the held row"
    );
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn stalled_events(cas_dir: &std::path::Path) -> Vec<crate::ui::factory::director::DirectorEvent> {
    use crate::ui::factory::director::{DirectorData, DirectorEvent, DirectorEventDetector};
    let data = DirectorData::load_with_git(cas_dir, None, false).unwrap();
    let mut detector = DirectorEventDetector::new(vec![WORKER.into()], "supervisor".into());
    detector.set_stall_threshold_secs(300);
    detector.initialize(&data);
    detector
        .detect_changes_at(&data, None, Instant::now(), chrono::Utc::now())
        .into_iter()
        .filter(|event| matches!(event, DirectorEvent::WorkerStalled { .. }))
        .collect()
}

/// `worker_activity` events the wake gate recorded for the worker.
fn gate_activity_events(cas_dir: &std::path::Path) -> i64 {
    rusqlite::Connection::open(cas_dir.join("cas.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM events WHERE event_type = 'worker_activity' AND session_id = 'worker-id'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// A factory with one busy Claude worker on a 30-minute-old task whose last
/// recorded activity is that old, so the stall detector reports it.
struct BusyWorkerFactory {
    _root: tempfile::TempDir,
    claude_config: std::path::PathBuf,
    cas_dir: std::path::PathBuf,
    clone_path: std::path::PathBuf,
    daemon: FactoryDaemon,
    _env: crate::test_support::TestEnvGuard,
}

impl BusyWorkerFactory {
    async fn new(env: &[(&str, Option<&str>)]) -> Self {
        use cas_store::EventStore;
        let root = tempfile::tempdir().unwrap();
        let claude_config = root.path().join("claude-config");
        std::fs::create_dir_all(claude_config.join("teams").join(SESSION).join("inboxes")).unwrap();
        let mut vars = vec![("CLAUDE_CONFIG_DIR", claude_config.to_str())];
        vars.extend_from_slice(env);
        let env = crate::test_support::TestEnvGuard::with_optional_vars(&vars);
        let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
        let clone_path = root.path().join("worker-clone");
        std::fs::create_dir_all(&clone_path).unwrap();
        let mut worker = cas_types::Agent::new_with_role(
            "worker-id".into(),
            WORKER.into(),
            cas_types::AgentRole::Worker,
        );
        worker.factory_session = Some(SESSION.into());
        worker
            .metadata
            .insert("clone_path".into(), clone_path.to_string_lossy().into_owned());
        let agents = open_agent_store(&cas_dir).unwrap();
        agents.register(&worker).unwrap();
        let mut supervisor = cas_types::Agent::new_with_role(
            "supervisor-id".into(),
            "supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        supervisor.factory_session = Some(SESSION.into());
        agents.register(&supervisor).unwrap();
        let mut task = cas_types::Task::new("cas-busy".into(), "long build".into());
        task.status = cas_types::TaskStatus::InProgress;
        // Task assignees are agent names.
        task.assignee = Some(WORKER.into());
        crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
        let mut note = cas_types::Event::new(
            cas_types::EventType::TaskNoteAdded,
            cas_types::EventEntityType::Task,
            "cas-busy",
            "started the build",
        )
        .with_session(&worker.id);
        note.created_at = chrono::Utc::now() - chrono::Duration::minutes(30);
        cas_store::SqliteEventStore::open(&cas_dir).unwrap().record(&note).unwrap();

        let mut daemon = super::provisioning_tests::daemon(&cas_dir);
        busy_claude_worker_pane(&mut daemon).await;
        daemon.teams = Some(super::teams::TeamsManager::new(SESSION));
        // The daemon's director view knows the registered, working worker.
        daemon.app.set_director_data_for_test(
            crate::ui::factory::director::DirectorData::load_with_git(&cas_dir, None, false).unwrap(),
        );
        Self { _root: root, claude_config, cas_dir, clone_path, daemon, _env: env }
    }

    /// The worker's transcript shows a tool call that has not come back.
    fn tool_call_in_flight(&self) {
        let slug = crate::factory_context_reset::claude_project_slug(&self.clone_path.to_string_lossy());
        let dir = self.claude_config.join("projects").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("worker-id.jsonl"),
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"build it\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_build\",\"name\":\"Bash\",\"input\":{}}]}}\n",
            ),
        )
        .unwrap();
    }

    fn queue(&self) -> std::sync::Arc<dyn PromptQueueStore> {
        open_prompt_queue_store(&self.cas_dir).unwrap()
    }
}

/// cas-d5ab (communication contract v2, P2.3): a wake-gate decline for a tool
/// call in flight is evidence that the worker is working. In session
/// cassy-swift-gazelle-77 the director raised worker_stalled 16496 for
/// subtle-gopher-35 while the gate was declining 4185905 for that very
/// worker. The decline now records worker activity through cas-7156's
/// per-worker recorder, so the stall detector does not fire.
#[tokio::test]
async fn ct2_p2_gate_decline_counts_as_activity() {
    let mut factory = BusyWorkerFactory::new(&[]).await;
    factory.tool_call_in_flight();
    assert!(
        !stalled_events(&factory.cas_dir).is_empty(),
        "control: with only stale activity the worker reads as stalled"
    );
    let queue = factory.queue();
    let id = queue
        .enqueue_with_session("supervisor", WORKER, "status check", SESSION)
        .unwrap();
    factory.daemon.process_prompt_queue().await.unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert!(
        report
            .wake_attempt_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("tool call in flight")),
        "the wake gate declined for a tool call in flight: {report:?}"
    );
    assert_eq!(gate_activity_events(&factory.cas_dir), 1);
    let stalls = stalled_events(&factory.cas_dir);
    assert!(stalls.is_empty(), "a tool-call gate decline is activity: {stalls:?}");
}

/// cas-d5ab review H1: a pane that is merely not silent is not activity. A
/// spinner keeps a hung worker's pane noisy, and counting it would hide the
/// stall cas-9bf6 exists to report.
#[tokio::test]
async fn a_noisy_pane_decline_is_not_activity_and_the_stall_still_reports() {
    let mut factory = BusyWorkerFactory::new(&[]).await;
    let queue = factory.queue();
    let id = queue
        .enqueue_with_session("supervisor", WORKER, "status check", SESSION)
        .unwrap();
    factory.daemon.process_prompt_queue().await.unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert!(
        report
            .wake_attempt_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("not been silent long enough")),
        "{report:?}"
    );
    assert_eq!(gate_activity_events(&factory.cas_dir), 0);
    assert!(!stalled_events(&factory.cas_dir).is_empty(), "the hung worker is still reported");
}

/// cas-d5ab review H1: the stall nudge's own gate decline is never activity,
/// or `withdraw_resumed_stall_nudge` would supersede the nudge as "activity
/// resumed" and the stall would never reach the supervisor.
#[tokio::test]
async fn a_stall_nudges_own_decline_never_withdraws_it() {
    let mut factory = BusyWorkerFactory::new(&[]).await;
    factory.tool_call_in_flight();
    let nudge = super::delivery::enqueue_worker_stall_nudge(
        &factory.cas_dir,
        SESSION,
        WORKER,
        "cas-busy",
        "quiet for 30 minutes",
        chrono::Utc::now() - chrono::Duration::minutes(30),
    )
    .unwrap();
    factory.daemon.process_prompt_queue().await.unwrap();
    assert_eq!(gate_activity_events(&factory.cas_dir), 0);
    let row = factory.queue().queued_prompt(nudge).unwrap().unwrap();
    assert!(row.processed_at.is_none(), "the busy hold keeps the nudge, never supersedes it");
}

/// cas-d5ab (communication contract v2, P1.6): a send to a busy recipient
/// returns the deferral reason in one line, through the real daemon's stamps.
/// In session cassy-swift-gazelle-77, 102 of 183 supervisor→worker messages
/// were deferred within 0.05 s of the send, yet every send result read
/// "queued … (enqueued)".
#[tokio::test(flavor = "current_thread")]
async fn ct2_p1_send_returns_named_outcome() {
    use rmcp::handler::server::wrapper::Parameters;
    let mut factory = BusyWorkerFactory::new(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("supervisor")),
        ("CAS_FACTORY_SESSION", Some(SESSION)),
    ])
    .await;
    factory.tool_call_in_flight();
    // Review N1: unread rows elsewhere in the project (other sessions, other
    // recipients) never stop the send from waiting for its own decision.
    let queue = factory.queue();
    for n in 0..15 {
        queue
            .enqueue_with_session("supervisor", &format!("other-{n}"), "elsewhere", "another-session")
            .unwrap();
        queue.enqueue_with_session("supervisor", "unregistered", "backlog", SESSION).unwrap();
    }
    assert!(queue.pending_count().unwrap() > 10);
    let core = crate::mcp::CasCore::with_daemon(factory.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("supervisor-id".into());
    #[cfg(feature = "mcp-proxy")]
    let service = crate::mcp::tools::CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = crate::mcp::tools::CasService::new(core);
    // The running daemon listens on its notify socket.
    let _socket = cas_factory::DaemonNotifier::bind(&factory.cas_dir).unwrap();
    let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "message",
        "target": WORKER,
        "summary": "decision",
        "message": "Decision: ONE daemon JobRuntime. Drop FactoryJobs.",
    }))
    .unwrap();

    let send = service.coordination(Parameters(request));
    let daemon = &mut factory.daemon;
    let passes = async {
        for _ in 0..200 {
            daemon.process_prompt_queue().await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::pin!(send);
    tokio::pin!(passes);
    let result = tokio::select! {
        result = &mut send => result,
        _ = &mut passes => panic!("the send never returned"),
    };
    let text: String = result
        .unwrap()
        .content
        .into_iter()
        .filter_map(|content| match content.raw {
            rmcp::model::RawContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.starts_with("deferred #"), "{text}");
    assert!(
        text.ends_with("recipient mid tool call; surfaces at its next tool call or turn start"),
        "{text}"
    );
}

/// cas-d5ab: only an observed tool call in flight counts as activity.
#[test]
fn only_a_tool_call_in_flight_counts_as_activity() {
    use super::queue_and_events::{ToolCallEvidence, WakeDecision, gate_decline_activity};
    let deny = WakeDecision::deny_for_test;
    assert_eq!(
        gate_decline_activity(deny("recipient has a tool call in flight"), ToolCallEvidence::InFlight),
        Some("tool call in flight")
    );
    for (reason, evidence) in [
        ("pane has not been silent long enough", ToolCallEvidence::Idle),
        ("no transcript evidence and the pane has not been silent long enough", ToolCallEvidence::Unknown),
        ("operator has an unsubmitted draft in the composer", ToolCallEvidence::Idle),
        ("pane is not ready for injection yet", ToolCallEvidence::Unknown),
        ("no wake slot left this pass (a supervisor wake already fired)", ToolCallEvidence::Idle),
    ] {
        assert_eq!(gate_decline_activity(deny(reason), evidence), None, "{reason}");
    }
}
