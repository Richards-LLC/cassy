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
    daemon.app.adopt_worker_pane_for_test(pane);
    tokio::time::sleep(Duration::from_millis(5100)).await;
    for pane in daemon.app.mux.panes_mut() {
        pane.drain_output();
    }
    let bytes = daemon.app.mux.pane_bytes_received(WORKER).unwrap();
    daemon.last_pane_output_bytes.insert(WORKER.into(), bytes);
    daemon.pane_silent_since.insert(WORKER.into(), Instant::now());
    assert!(daemon.app.mux.pane_ready_for_injection(WORKER));
}

/// The 22:08:30Z case, through the real `process_prompt_queue`: the wake is
/// declined because the pane is not silent, and the message is still written
/// to the worker's Teams inbox in the same pass. It's held for the tool
/// boundary, not dropped, and the worker's next tool call can surface it.
#[tokio::test]
async fn a_declined_wake_still_writes_the_teams_copy_and_leaves_the_row_for_the_tool_boundary() {
    let root = tempfile::tempdir().unwrap();
    let claude_config = root.path().join("claude-config");
    std::fs::create_dir_all(&claude_config).unwrap();
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
    assert!(
        written.contains(decision),
        "the Teams inbox write must not wait on the silence gate: {written:?}"
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
