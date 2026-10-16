use super::super::{jobs, loop_watchdog, prompt_queue_jobs};
use super::*;

fn fixture(
    transcript: bool,
) -> (
    tempfile::TempDir,
    FactoryDaemon,
    Arc<dyn cas_store::PromptQueueStore>,
    i64,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    daemon.app.add_worker_name_for_test("worker-a");
    let store = crate::store::open_agent_store(&root).unwrap();
    let mut worker = cas_types::Agent::new_with_role(
        "worker-id".into(),
        "worker-a".into(),
        cas_types::AgentRole::Worker,
    );
    worker.factory_session = Some(daemon.session_name.clone());
    worker.metadata.insert("worker_cli".into(), "codex".into());
    let account = temp.path().join("codex-account");
    let clone = temp.path().join("clone");
    std::fs::create_dir_all(&clone).unwrap();
    worker
        .metadata
        .insert("worker_account_dir".into(), account.display().to_string());
    worker
        .metadata
        .insert("clone_path".into(), clone.display().to_string());
    store.register(&worker).unwrap();
    let mut lead = cas_types::Agent::new_with_role(
        "lead-id".into(),
        daemon.app.supervisor_name().into(),
        cas_types::AgentRole::Supervisor,
    );
    lead.factory_session = Some(daemon.session_name.clone());
    store.register(&lead).unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let row = queue
        .enqueue_with_session(
            "supervisor",
            "worker-a",
            "original message",
            &daemon.session_name,
        )
        .unwrap();
    let delivery = chrono::Utc::now();
    if transcript {
        let path = account.join("sessions/2026/10/16/rollout-cas031e.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let meta = serde_json::json!({"timestamp": delivery.to_rfc3339(), "type":"session_meta",
            "payload":{"session_id":"019f84af-3121-7950-ba14-b01db2da031e", "cwd":clone.display().to_string(), "originator":"codex-tui", "source":"cli", "thread_source":"user"}});
        let turn = serde_json::json!({"timestamp":(delivery+chrono::Duration::seconds(1)).to_rfc3339(),
            "type":"event_msg", "payload":{"type":"task_started", "turn_id":"cas031e-turn"}});
        std::fs::write(path, format!("{meta}\n{turn}\n")).unwrap();
    }
    daemon.normal_delivery_probes.insert(
        row,
        NormalDeliveryProbe {
            pane: "worker-a".into(),
            target: "worker-a".into(),
            bytes_at_delivery: 0,
            delivered_at: Instant::now() - NORMAL_DELIVERY_OBSERVE_WINDOW,
            delivered_at_utc: delivery,
            prompt: "original message".into(),
            nudge_sent_at: None,
            next_check_at: None,
        },
    );
    (temp, daemon, queue, row)
}

#[tokio::test]
async fn cas_031e_normal_probe_runs_under_enforced_daemon_step() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, mut daemon, queue, row) = fixture(true);
    let root = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    let progress = loop_watchdog::LoopProgress::new();
    let before = cas_store::lane_guard::thread_violations();
    let deadline = Instant::now() + Duration::from_secs(3);
    while daemon.normal_delivery_probes.contains_key(&row) && Instant::now() < deadline {
        {
            let (_attachment, _enforce) = loop_watchdog::enforced_daemon_step(
                &progress,
                loop_watchdog::LoopPhase::PromptQueue,
            );
            daemon.resolve_normal_delivery_probes(queue.as_ref()).await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !daemon.normal_delivery_probes.contains_key(&row),
        "real transcript turn must retire its own probe"
    );
    assert_eq!(cas_store::lane_guard::thread_violations(), before);
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[tokio::test]
async fn cas_031e_poison_idle_probe_runs_under_enforced_daemon_step() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, mut daemon, queue, row) = fixture(false);
    let root = daemon.app.cas_dir().to_path_buf();
    let old = chrono::Utc::now() - chrono::Duration::minutes(11);
    rusqlite::Connection::open(root.join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=? WHERE id=?",
            rusqlite::params![old.to_rfc3339(), row],
        )
        .unwrap();
    daemon.pane_silent_since.insert(
        "worker-a".into(),
        Instant::now() - Duration::from_secs(11 * 60),
    );
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    let progress = loop_watchdog::LoopProgress::new();
    let before = cas_store::lane_guard::thread_violations();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        {
            let (_attachment, _enforce) = loop_watchdog::enforced_daemon_step(
                &progress,
                loop_watchdog::LoopPhase::PromptQueue,
            );
            daemon.relay_idle_unacked_messages(queue.as_ref());
        }
        if queue
            .peek_all(20)
            .unwrap()
            .iter()
            .any(|r| r.prompt.contains("Unread message on an idle worker"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "idle probe must persist its message-bound relay"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(cas_store::lane_guard::thread_violations(), before);
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[test]
#[should_panic(expected = "enforce mode: move it to the jobs lane")]
fn cas_031e_old_probe_read_panics_under_enforcement_control() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, daemon, _queue, _row) = fixture(true);
    let progress = loop_watchdog::LoopProgress::new();
    let (_attachment, _enforce) =
        loop_watchdog::enforced_daemon_step(&progress, loop_watchdog::LoopPhase::PromptQueue);
    daemon.normal_delivery_turn_evidence("worker-a", chrono::Utc::now(), "original message", false);
}

#[tokio::test]
async fn cas_031e_superseded_message_discards_its_held_probe_result() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, mut daemon, queue, row) = fixture(true);
    let root = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    daemon.queue_jobs.submit_external(
        jobs::JobSpec {
            kind: "HeldProbe",
            key: "HeldProbe:reader".into(),
            lane: jobs::Lane::Blocking,
            criticality: jobs::Criticality::Critical,
            generation: 1,
            budget: Duration::from_millis(100),
        },
        move || {
            entered_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Box::new(())
        },
    );
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    daemon.resolve_normal_delivery_probes(queue.as_ref()).await;
    assert!(
        daemon.normal_delivery_probes.contains_key(&row),
        "held read must preserve the original probe until its result"
    );
    // Supersede even under the same row and pane: a new delivery phase and body.
    let replacement = daemon.normal_delivery_probes.get_mut(&row).unwrap();
    replacement.prompt = "replacement message".into();
    replacement.delivered_at = Instant::now();
    replacement.delivered_at_utc = chrono::Utc::now() + chrono::Duration::minutes(1);
    release_tx.send(()).unwrap();
    let progress = loop_watchdog::LoopProgress::new();
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        {
            let (_attachment, _enforce) = loop_watchdog::enforced_daemon_step(
                &progress,
                loop_watchdog::LoopPhase::PromptQueue,
            );
            daemon.resolve_normal_delivery_probes(queue.as_ref()).await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let current = daemon
        .normal_delivery_probes
        .get(&row)
        .expect("old turn must not retire the replacement");
    assert_eq!(current.prompt, "replacement message");
    assert!(current.nudge_sent_at.is_none());
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}
