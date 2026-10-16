//! cas-4491f (factory loop v2 C2c): the relays and the director-prompt
//! inject read and write nothing on the loop.
use super::super::imports::*;
use super::director_refresh::DirectorRefresh;
use cas_store::{lane_guard, wait_budget};

/// A test daemon on real job lanes (the provisioning fixture runs queue jobs
/// inline, which would put every job back on the loop).
fn daemon_on_lanes(cas_dir: &std::path::Path) -> FactoryDaemon {
    let mut daemon = super::provisioning_tests::daemon(cas_dir);
    daemon.queue_jobs = super::prompt_queue_jobs::QueueJobs::start(
        cas_dir.to_path_buf(),
        daemon.app.project_scope(),
    );
    daemon
}

/// The loop's side of one pass: store access is forbidden and the thread is
/// the control lane, so every store access, git command, `/proc` scan and
/// transcript read counts.
pub(super) struct LoopPass {
    store: u64,
    lane: u64,
    _forbid: wait_budget::ForbidStoreGuard,
    _control: lane_guard::LaneGuard,
}

impl LoopPass {
    pub(super) fn start() -> Self {
        Self {
            store: wait_budget::store_access_violations(),
            lane: lane_guard::thread_violations(),
            _forbid: wait_budget::forbid_store_access(),
            _control: lane_guard::enter_lane(lane_guard::Lane::Control),
        }
    }

    pub(super) fn assert_no_io(&self, what: &str) {
        assert_eq!(
            wait_budget::store_access_violations(),
            self.store,
            "{what} read the store on the loop: {:?}",
            wait_budget::last_store_access_violation()
        );
        assert_eq!(
            lane_guard::thread_violations(),
            self.lane,
            "{what} ran blocking work on the loop: {:?}",
            lane_guard::last_thread_violation()
        );
    }
}

/// A project with this factory session's active worker registered, so the
/// relays' reads have a candidate to look at.
fn project_with_worker() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut worker = cas_types::Agent::new("w-1".into(), "relay-worker".into());
    worker.role = cas_types::AgentRole::Worker;
    worker.factory_session = Some("provision-test".into());
    crate::store::open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
    (dir, cas_dir)
}

/// Apply relay results until none is in flight (hang guard: 30 s).
async fn settle_relays(daemon: &mut FactoryDaemon) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        daemon.drain_relay_jobs();
        if daemon.relay_jobs.idle() {
            return;
        }
        assert!(Instant::now() < deadline, "relay jobs never settled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn c4491f_usage_limit_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test("relay-worker");
    {
        let pass = LoopPass::start();
        daemon.relay_usage_limited_workers();
        pass.assert_no_io("the usage-limit relay");
    }
    settle_relays(&mut daemon).await;
}

#[tokio::test]
async fn c4491f_auth_failure_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test("relay-worker");
    {
        let pass = LoopPass::start();
        daemon.relay_auth_failed_workers();
        pass.assert_no_io("the auth-failure relay");
    }
    settle_relays(&mut daemon).await;
}

#[tokio::test]
async fn c4491f_permission_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test("relay-worker");
    {
        let pass = LoopPass::start();
        daemon.relay_worker_permission_requests();
        pass.assert_no_io("the permission relay");
    }
    settle_relays(&mut daemon).await;
}

/// The correlated-deaths relay writes on the store worker, and the group
/// leaves the window once its write reports.
#[tokio::test]
async fn c4491f_correlated_deaths_relay_writes_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    // Older than the ten-minute retry limit, so the group settles whether or
    // not this bare project can persist the relay.
    let at = chrono::Utc::now() - chrono::Duration::minutes(11);
    for worker in ["dead-a", "dead-b"] {
        daemon
            .recent_worker_exits
            .push(super::ObservedWorkerExit {
                at,
                worker: worker.into(),
                status: "exit 1".into(),
            });
    }
    {
        let pass = LoopPass::start();
        daemon.relay_correlated_worker_deaths();
        pass.assert_no_io("the correlated-deaths relay");
    }
    settle_relays(&mut daemon).await;
    assert!(daemon.recent_worker_exits.is_empty(), "the group settled");
}

#[tokio::test]
async fn c4491f_relay_escalation_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    {
        let pass = LoopPass::start();
        daemon.escalate_undelivered_supervisor_relays();
        pass.assert_no_io("the relay escalation");
    }
    settle_relays(&mut daemon).await;
}

/// An assignment wake's task is read with its `RevalidateFresh` re-check,
/// and the assignment row is written on the store worker: neither the
/// delivery step nor the inject reads the task store on the loop.
#[tokio::test]
async fn c4491f_assignment_wake_reads_its_task_off_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut task = cas_types::Task::new("cas-a551".into(), "assigned work".into());
    task.assignee = Some("relay-worker".into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::loop_watchdog::LoopProgress::new();
    let store_worker = super::store_worker::global();
    let mut refresh = DirectorRefresh::new();
    let wake = crate::ui::factory::director::Prompt {
        target: "relay-worker".into(),
        text: "You have been assigned a new task:\nTask ID: cas-a551\nTitle: assigned work".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: None,
        durable_retry: true,
    };
    {
        let pass = LoopPass::start();
        daemon
            .deliver_director_events(
                &mut refresh,
                Vec::new(),
                (Vec::new(), vec![wake], None),
                Instant::now(),
                0.0,
                store_worker,
                &progress,
            )
            .await;
        pass.assert_no_io("the delivery step");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let batch = loop {
        if let Some(batch) = refresh.take_deliverability(&mut daemon.queue_jobs).pop() {
            break batch;
        }
        assert!(Instant::now() < deadline, "the re-check never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(
        matches!(
            batch.assignments.first(),
            Some(crate::ui::factory::app::AssignmentRead::Task(task)) if task.id == "cas-a551"
        ),
        "the job read the assigned task: {:?}",
        batch.assignments
    );
    {
        let pass = LoopPass::start();
        daemon.inject_rechecked_batch(batch, store_worker).await;
        pass.assert_no_io("the assignment inject");
    }
}

/// A transport-boundary read of a task that moved on suppresses the wake;
/// one it cannot read is uncertainty (delivered directly).
#[test]
fn c4491f_assignment_read_names_a_stale_task() {
    use crate::ui::factory::app::AssignmentRead;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut task = cas_types::Task::new("cas-c105".into(), "done".into());
    task.status = cas_types::TaskStatus::Closed;
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let wake = |id: &str| crate::ui::factory::director::Prompt {
        target: "w".into(),
        text: format!("You have been assigned a new task:\nTask ID: {id}"),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: None,
        durable_retry: true,
    };
    assert!(matches!(
        AssignmentRead::read(&cas_dir, None, &wake("cas-c105")),
        AssignmentRead::Stale(id, cas_types::TaskStatus::Closed) if id == "cas-c105"
    ));
    assert!(matches!(
        AssignmentRead::read(&cas_dir, None, &wake("cas-9999")),
        AssignmentRead::Unavailable
    ));
}
