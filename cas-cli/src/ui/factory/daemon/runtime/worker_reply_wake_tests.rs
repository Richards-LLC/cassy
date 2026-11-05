//! cas-2961: a worker's direct reply reaches an idle supervisor.
//!
//! Modelled on gabber message 2868195: the worker's task was parked at a tip
//! that had already landed, and its `status: ready` named a new tip (PR #4200).
//! CAS bound the reply to the parked anchor, filed it as a supervisor digest,
//! and nothing woke the supervisor for 8 h.
use rmcp::handler::server::wrapper::Parameters;

use crate::ui::factory::daemon::FactoryDaemon;

const WORKER: &str = "patient-cheetah-32";
const TASK: &str = "cas-21f1";
const PARKED_TIP: &str = "30287d77e474647ea8909f9219818cdfcd249e78";

/// A same-session worker whose parked delivery has already landed.
fn worker_with_landed_park(daemon: &FactoryDaemon) {
    let cas_dir = daemon.app.cas_dir();
    let mut worker =
        cas_types::Agent::new_with_role("worker-id".into(), WORKER.into(), cas_types::AgentRole::Worker);
    worker.factory_session = Some(daemon.session_name.clone());
    crate::store::open_agent_store(cas_dir).unwrap().register(&worker).unwrap();
    let mut task = cas_types::Task::new(TASK.into(), "conversation stage guard".into());
    task.status = cas_types::TaskStatus::AwaitingMerge;
    task.assignee = Some(WORKER.into());
    task.deliverables.factory_branch_anchor = Some(PARKED_TIP.into());
    task.deliverables.green_merge_commit = Some("75e0297b77a45ac5190201821a991d9fa61b7133".into());
    crate::store::open_task_store(cas_dir).unwrap().add(&task).unwrap();
}

/// Send `kind=ready` through the real coordination tool, as the worker.
async fn worker_ready(daemon: &FactoryDaemon, body: &str) -> i64 {
    let cas_dir = daemon.app.cas_dir().to_path_buf();
    let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("worker-id".into());
    #[cfg(feature = "mcp-proxy")]
    let service = crate::mcp::tools::CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = crate::mcp::tools::CasService::new(core);
    let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "kind": "ready", "task_id": TASK,
        "summary": "Guard entry fixed: PR #4200", "message": body,
    }))
    .unwrap();
    service.coordination(Parameters(request)).await.unwrap();
    rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap()
        .query_row("SELECT MAX(id) FROM prompt_queue WHERE instr(prompt, ?) > 0", [body], |row| row.get(0))
        .expect("the ready row is queued")
}

fn age(daemon: &FactoryDaemon, id: i64, secs: i64) {
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db")).unwrap().execute(
        "UPDATE prompt_queue SET created_at=?1, next_attempt_at=?1 WHERE id=?2",
        rusqlite::params![(chrono::Utc::now() - chrono::Duration::seconds(secs)).to_rfc3339(), id],
    ).unwrap();
}

async fn wake_attempt(daemon: &mut FactoryDaemon, id: i64) -> cas_store::WakeAttempt {
    daemon.process_prompt_queue().await.unwrap();
    super::supervisor_single_turn_tests::settle_injected_writes(daemon).await;
    crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap()
        .message_delivery_report(id).unwrap().unwrap().wake_attempt
}

#[tokio::test(flavor = "current_thread")]
async fn a_ready_naming_a_new_tip_wakes_the_idle_supervisor_cas_2961() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = super::peer_mailbox_wake_tests::peer_fixture().await;
    worker_with_landed_park(&daemon);
    let id = worker_ready(&daemon, "status: ready\ntip: 0df66f60a6f180bbaa119d0d2b9d0eaebc0a655d on factory/patient-cheetah-32-guard-entry; worktree: clean\nci: PR #4200 to the epic just opened\nneed: merge #4200 into the epic").await;
    // Past the directed-message burst debounce, well inside 30 s.
    age(&daemon, id, 21);
    assert_eq!(wake_attempt(&mut daemon, id).await, cas_store::WakeAttempt::Fired,
        "a new delivery's ready must wake the idle supervisor with the whole message");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stranded_worker_message_raises_one_alarm_wake_and_notice_cas_2961() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = super::peer_mailbox_wake_tests::peer_fixture().await;
    worker_with_landed_park(&daemon);
    // A ready that repeats the landed tip is filed as a digest (row 2868191's
    // delivery); if the supervisor never reads it, the alarm still does.
    let id = worker_ready(&daemon, &format!("status: ready\ntip: {PARKED_TIP} on factory/patient-cheetah-32-cas-21f1")).await;
    age(&daemon, id, 21);
    assert_eq!(wake_attempt(&mut daemon, id).await, cas_store::WakeAttempt::NotAttempted, "a digest has no wake of its own");
    age(&daemon, id, 10 * 60 + 1);
    let cas_dir = daemon.app.cas_dir().to_path_buf();
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let supervisor = daemon.app.supervisor_name().to_string();
    let alarm = || super::queue_and_events::escalate_undelivered_supervisor_relays(
        &cas_dir, queue.as_ref(), &daemon.session_name, &supervisor, "Claude", chrono::Utc::now());
    let alerts = alarm();
    assert_eq!(alerts.iter().map(|alert| alert.relay_id).collect::<Vec<_>>(), vec![id]);
    assert!(alerts[0].summary.contains(WORKER), "{}", alerts[0].summary);
    assert!(alarm().is_empty(), "the alarm is deduplicated");
    assert_eq!(wake_attempt(&mut daemon, id).await, cas_store::WakeAttempt::Fired,
        "the alarm wakes the idle supervisor once with the stranded message");
}

/// cas-922d: a worker's ask is the conversation. Its seventh in an hour must
/// not wait for the sender-pair cap, and it skips the 20 s burst debounce, so
/// an idle supervisor wakes on the first pass.
#[tokio::test(flavor = "current_thread")]
async fn a_capped_worker_ask_wakes_the_idle_supervisor_at_once_cas_922d() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = super::peer_mailbox_wake_tests::peer_fixture().await;
    let cas_dir = daemon.app.cas_dir().to_path_buf();
    let mut worker =
        cas_types::Agent::new_with_role("worker-id".into(), WORKER.into(), cas_types::AgentRole::Worker);
    worker.factory_session = Some(daemon.session_name.clone());
    crate::store::open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
    let spent = chrono::Utc::now() - chrono::Duration::minutes(10);
    for lead in 0..6 {
        crate::supervisor_message_wakes::record(&cas_dir, 900_000 + lead, daemon.app.supervisor_name(),
            &["worker-id".to_string()], spent, spent).unwrap();
    }
    let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("worker-id".into());
    #[cfg(feature = "mcp-proxy")]
    let service = crate::mcp::tools::CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = crate::mcp::tools::CasService::new(core);
    let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "kind": "ask",
        "summary": "Which base?", "message": "Should cas-922d start from origin/main or the parked branch?",
    }))
    .unwrap();
    service.coordination(Parameters(request)).await.unwrap();
    let id: i64 = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap()
        .query_row("SELECT MAX(id) FROM prompt_queue WHERE instr(prompt, 'origin/main or the parked') > 0", [], |row| row.get(0))
        .expect("the ask is queued");
    assert_eq!(wake_attempt(&mut daemon, id).await, cas_store::WakeAttempt::Fired);
}
