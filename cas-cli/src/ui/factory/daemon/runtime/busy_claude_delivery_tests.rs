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
    tokio::time::sleep(Duration::from_millis(5100)).await;
    for pane in daemon.app.mux.panes_mut() {
        pane.drain_output();
    }
    let bytes = daemon.app.mux.pane_bytes_received(WORKER).unwrap();
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
    let root = tempfile::tempdir().unwrap();
    let claude_config = root.path().join("claude-config");
    // The live team tree a running factory has (a missing one falls back to PTY).
    std::fs::create_dir_all(claude_config.join("teams").join(SESSION).join("inboxes")).unwrap();
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
    open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();

    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    busy_claude_worker_pane(&mut daemon).await;
    daemon.teams = Some(super::teams::TeamsManager::new(SESSION));

    let queue = open_prompt_queue_store(&cas_dir).unwrap();
    let decision = "Decision: ONE daemon JobRuntime. Drop FactoryJobs.";
    let turn_started_at = chrono::Utc::now() - chrono::Duration::minutes(20);
    let id = queue
        .enqueue_with_session("supervisor", WORKER, decision, SESSION)
        .unwrap();
    daemon.process_prompt_queue().await.unwrap();

    let row = queue.queued_prompt(id).unwrap().unwrap();
    assert!(row.processed_at.is_none(), "a declined wake holds the row, never drops it");
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

/// cas-d5ab (communication contract v2, P2.3): a wake-gate decline because
/// the recipient is busy is evidence that it is working. In session
/// cassy-swift-gazelle-77 the director raised worker_stalled 16496 for
/// subtle-gopher-35 while the gate was declining 4185905 for that very
/// worker. The decline now records worker activity through cas-7156's
/// per-worker recorder, so the stall detector does not fire.
#[tokio::test]
async fn ct2_p2_gate_decline_counts_as_activity() {
    use cas_store::{EventStore, TaskStore};
    let root = tempfile::tempdir().unwrap();
    let claude_config = root.path().join("claude-config");
    std::fs::create_dir_all(claude_config.join("teams").join(SESSION).join("inboxes")).unwrap();
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
    open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
    // A task in progress whose last recorded activity is 30 minutes old.
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
    assert!(
        !stalled_events(&cas_dir).is_empty(),
        "control: with only stale activity the worker reads as stalled"
    );

    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    busy_claude_worker_pane(&mut daemon).await;
    daemon.teams = Some(super::teams::TeamsManager::new(SESSION));
    // The daemon's director view knows the registered, working worker.
    daemon.app.set_director_data_for_test(
        crate::ui::factory::director::DirectorData::load_with_git(&cas_dir, None, false).unwrap(),
    );
    let queue = open_prompt_queue_store(&cas_dir).unwrap();
    let id = queue
        .enqueue_with_session("supervisor", WORKER, "status check", SESSION)
        .unwrap();
    daemon.process_prompt_queue().await.unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert!(
        report
            .wake_attempt_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("not been silent long enough")),
        "the wake gate declined for a busy pane: {report:?}"
    );

    let stalls = stalled_events(&cas_dir);
    assert!(stalls.is_empty(), "a busy-pane gate decline is activity: {stalls:?}");
}

/// cas-d5ab: only a decline that shows the recipient working is activity.
#[test]
fn only_busy_recipient_declines_count_as_activity() {
    use super::queue_and_events::{ToolCallEvidence, WakeDecision, gate_decline_activity};
    let deny = WakeDecision::deny_for_test;
    assert_eq!(
        gate_decline_activity(deny("recipient has a tool call in flight"), ToolCallEvidence::InFlight),
        Some("tool call in flight")
    );
    assert_eq!(
        gate_decline_activity(deny("pane has not been silent long enough"), ToolCallEvidence::Idle),
        Some("pane not silent")
    );
    assert_eq!(
        gate_decline_activity(
            deny("no transcript evidence and the pane has not been silent long enough"),
            ToolCallEvidence::Unknown
        ),
        Some("pane not silent")
    );
    for reason in [
        "operator has an unsubmitted draft in the composer",
        "pane is not ready for injection yet",
        "no wake slot left this pass (a supervisor wake already fired)",
    ] {
        assert_eq!(gate_decline_activity(deny(reason), ToolCallEvidence::Idle), None, "{reason}");
    }
}
