use super::*;
use cas_store::wait_budget;

#[test]
fn cas_7b6b_spawn_poll_touches_no_store_on_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    // The daemon's actual asynchronous job lanes; fixture defaults are inline.
    daemon.queue_jobs =
        super::super::prompt_queue_jobs::QueueJobs::start(root, daemon.app.project_scope());
    let before = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        for _ in 0..8 {
            daemon.enqueue_spawn_requests().unwrap();
        }
    }
    assert_eq!(
        wait_budget::store_access_violations(),
        before,
        "spawn polling touched the loop store: {:?}",
        wait_budget::last_store_access_violation()
    );
}

#[test]
fn cas_7b6b_prompt_wake_gate_touches_no_store_on_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let tasks = crate::store::open_task_store_cached(&root).unwrap();
    let mut task = cas_types::Task::new("cas-gate".into(), "Waiting on supervisor".into());
    task.status = cas_types::TaskStatus::Blocked;
    task.assignee = Some("worker-a".into());
    tasks.add(&task).unwrap();
    let daemon = super::super::provisioning_tests::daemon(&root);
    let sender = WakeSender::Registered {
        role: cas_types::AgentRole::Supervisor,
        name: daemon.app.supervisor_name().into(),
    };
    let before = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        let _ = daemon.worker_wait_state_for(&sender, "worker-a");
    }
    assert_eq!(
        wait_budget::store_access_violations(),
        before,
        "prompt wake gate touched the loop store: {:?}",
        wait_budget::last_store_access_violation()
    );
}
