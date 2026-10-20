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
            nudges: 0,
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
            .any(|r| r.id != row && r.target == daemon.app.supervisor_name())
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
    super::super::delivery_probe_jobs::normal_delivery_turn_evidence_io(
        daemon.app.cas_dir(),
        "worker-a",
        chrono::Utc::now(),
        "original message",
        false,
    );
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
    let replacement_body = String::from("replacement message");
    replacement.prompt = replacement_body.clone();
    replacement.delivered_at = Instant::now();
    replacement.delivered_at_utc = chrono::Utc::now() + chrono::Duration::minutes(1);
    release_tx.send(()).unwrap();
    let progress = loop_watchdog::LoopProgress::new();
    assert!(
        daemon
            .relay_jobs
            .reading(super::super::relay_jobs::Relay::NormalProbes)
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while daemon
        .relay_jobs
        .reading(super::super::relay_jobs::Relay::NormalProbes)
        && Instant::now() < deadline
    {
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
        !daemon
            .relay_jobs
            .reading(super::super::relay_jobs::Relay::NormalProbes),
        "the held result must actually report before asserting it was discarded"
    );
    let current = daemon
        .normal_delivery_probes
        .get(&row)
        .expect("old turn must not retire the replacement");
    assert_eq!(current.prompt, replacement_body);
    assert!(current.nudge_sent_at.is_none());
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[tokio::test]
async fn cas_cc89_whole_poison_sweep_runs_under_enforced_daemon_step() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (temp, mut daemon, queue, row) = fixture(false);
    let inbox = setup_pointer(&mut daemon, queue.as_ref(), row, temp.path(), &mut env).await;
    let root = daemon.app.cas_dir().to_path_buf();
    let old = chrono::Utc::now() - chrono::Duration::minutes(31);
    rusqlite::Connection::open(root.join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=? WHERE id=?",
            rusqlite::params![old.to_rfc3339(), row],
        )
        .unwrap();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    let progress = loop_watchdog::LoopProgress::new();
    let before = cas_store::lane_guard::thread_violations();
    let targets = vec![
        "worker-a".into(),
        daemon.app.supervisor_name().into(),
        "all_workers".into(),
    ];
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        {
            let (_attachment, _enforce) = loop_watchdog::enforced_daemon_step(
                &progress,
                loop_watchdog::LoopPhase::PromptQueue,
            );
            daemon
                .run_prompt_poison_sweep(queue.as_ref(), &targets)
                .await;
        }
        let notices: Vec<_> = queue
            .peek_all(30)
            .unwrap()
            .into_iter()
            .filter(|row| row.source == "delivery-watchdog")
            .collect();
        let body_delivered = std::fs::read_to_string(&inbox)
            .ok()
            .and_then(|text| {
                serde_json::from_str::<Vec<super::super::teams::InboxMessage>>(&text).ok()
            })
            .is_some_and(|messages| {
                messages.iter().any(|message| {
                    message
                        .text
                        .contains(&queue.queued_prompt(row).unwrap().unwrap().prompt)
                })
            });
        if !notices.is_empty() && body_delivered {
            assert_eq!(notices.len(), 1, "one durable sender bounce");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "full sweep never bounced the unread message"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Replay the actual complete step; durable claims/episode markers must
    // leave exactly one body and one bounce after outstanding work settles.
    for _ in 0..5 {
        {
            let (_attachment, _enforce) = loop_watchdog::enforced_daemon_step(
                &progress,
                loop_watchdog::LoopPhase::PromptQueue,
            );
            daemon
                .run_prompt_poison_sweep(queue.as_ref(), &targets)
                .await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        queue
            .peek_all(30)
            .unwrap()
            .iter()
            .filter(|row| row.source == "delivery-watchdog")
            .count(),
        1
    );
    let messages: Vec<super::super::teams::InboxMessage> =
        serde_json::from_str(&std::fs::read_to_string(inbox).unwrap()).unwrap();
    assert_eq!(messages.len(), 2, "one pointer and one full body");
    assert_eq!(cas_store::lane_guard::thread_violations(), before);
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[test]
#[should_panic(expected = "enforce mode: move it to the jobs lane")]
fn cas_cc89_old_completion_read_panics_under_enforcement_control() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, daemon, _queue, _row) = fixture(false);
    let progress = loop_watchdog::LoopProgress::new();
    let (_attachment, _enforce) =
        loop_watchdog::enforced_daemon_step(&progress, loop_watchdog::LoopPhase::PromptQueue);
    super::super::delivery_probe_jobs::recipient_turn_completion_after_io(
        daemon.app.cas_dir(),
        "worker-a",
        chrono::Utc::now(),
    );
}

fn hold_lane(daemon: &mut FactoryDaemon, lane: jobs::Lane) -> std::sync::mpsc::Sender<()> {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    daemon.queue_jobs.submit_external(
        jobs::JobSpec {
            kind: "PoisonHold",
            key: format!("PoisonHold:{lane:?}"),
            lane,
            criticality: jobs::Criticality::Critical,
            generation: 1,
            budget: Duration::from_millis(100),
        },
        move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
            Box::new(())
        },
    );
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    release_tx
}

async fn setup_pointer(
    daemon: &mut FactoryDaemon,
    queue: &dyn cas_store::PromptQueueStore,
    row: i64,
    project: &std::path::Path,
    guard: &mut crate::test_support::TestEnvGuard,
) -> std::path::PathBuf {
    let config_dir = guard.home().join(".claude");
    guard.set("CLAUDE_CONFIG_DIR", &config_dir);
    let teams = super::super::teams::TeamsManager::new(&daemon.session_name);
    teams
        .init_team_config(
            &["worker-a".into()],
            project,
            &std::collections::HashMap::new(),
            "lead-session",
        )
        .unwrap();
    daemon.teams = Some(teams);
    let original = queue.queued_prompt(row).unwrap().unwrap();
    daemon
        .deliver_queued_to_worker(
            "worker-a",
            "supervisor",
            &format!(
                "[cas #{row} supervisor-authored 0s first]\n\n{}",
                original.prompt
            ),
            None,
            row,
            &original.prompt,
        )
        .await
        .unwrap();
    assert!(queue.claim_recipient_transport(row, "worker-a").unwrap());
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue_recipient_seen SET seen_at=? WHERE prompt_id=?",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::minutes(4)).to_rfc3339(),
                row
            ],
        )
        .unwrap();
    config_dir
        .join("teams")
        .join(&daemon.session_name)
        .join("inboxes/worker-a.json")
}

#[tokio::test]
async fn cas_cc89_pointer_failure_releases_claim_and_retry_delivers_once() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (temp, mut daemon, queue, row) = fixture(false);
    let inbox = setup_pointer(&mut daemon, queue.as_ref(), row, temp.path(), &mut env).await;
    // A real filesystem failure at the inbox boundary, not a fake job result.
    std::fs::remove_file(&inbox).unwrap();
    std::fs::create_dir(&inbox).unwrap();
    let root = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    daemon
        .fallback_unsurfaced_teams_pointers(queue.as_ref())
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        daemon.drain_relay_jobs();
        if !daemon
            .relay_jobs
            .reading(super::super::relay_jobs::Relay::TeamsPointers)
            && daemon.queue_jobs.poison_jobs.pointers.is_empty()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "failed pointer phase never released"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!queue.teams_body_fallback_claimed(row, "worker-a").unwrap());
    std::fs::remove_dir(&inbox).unwrap();
    daemon
        .fallback_unsurfaced_teams_pointers(queue.as_ref())
        .await;
    loop {
        daemon.drain_relay_jobs();
        if !daemon
            .relay_jobs
            .reading(super::super::relay_jobs::Relay::TeamsPointers)
            && daemon.queue_jobs.poison_jobs.pointers.is_empty()
        {
            break;
        }
        assert!(Instant::now() < deadline, "retry never delivered");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(queue.teams_body_fallback_claimed(row, "worker-a").unwrap());
    daemon
        .fallback_unsurfaced_teams_pointers(queue.as_ref())
        .await;
    while daemon
        .relay_jobs
        .reading(super::super::relay_jobs::Relay::TeamsPointers)
    {
        daemon.drain_relay_jobs();
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let messages: Vec<super::super::teams::InboxMessage> =
        serde_json::from_str(&std::fs::read_to_string(inbox).unwrap()).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "retry and rescan must not duplicate body"
    );
    assert!(
        messages[0]
            .text
            .contains(&queue.queued_prompt(row).unwrap().unwrap().prompt)
    );
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[tokio::test]
async fn cas_cc89_idle_validation_discards_acked_replaced_and_dead_rows() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    for change in ["ack", "replace", "death"] {
        let (_temp, mut daemon, queue, row) = fixture(false);
        let root = daemon.app.cas_dir().to_path_buf();
        rusqlite::Connection::open(root.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at=? WHERE id=?",
                rusqlite::params![
                    (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339(),
                    row
                ],
            )
            .unwrap();
        daemon
            .pane_silent_since
            .insert("worker-a".into(), Instant::now() - Duration::from_secs(660));
        daemon.queue_jobs =
            prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
        let release = hold_lane(&mut daemon, jobs::Lane::External);
        let deadline = Instant::now() + Duration::from_secs(5);
        daemon.relay_idle_unacked_messages(queue.as_ref());
        while !daemon.relay_jobs.writing(&format!("idle-check:{row}")) {
            daemon.drain_relay_jobs();
            assert!(
                Instant::now() < deadline,
                "validation never reached the held lane"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        match change {
            "ack" => queue.ack(row).unwrap(),
            "replace" => {
                rusqlite::Connection::open(root.join("cas.db"))
                    .unwrap()
                    .execute(
                        "UPDATE prompt_queue SET prompt=? WHERE id=?",
                        rusqlite::params!["replacement message", row],
                    )
                    .unwrap();
            }
            "death" => {
                daemon.dead_workers.insert("worker-a".into());
            }
            _ => unreachable!(),
        }
        release.send(()).unwrap();
        let progress = loop_watchdog::LoopProgress::new();
        loop {
            {
                let (_attachment, _enforce) = loop_watchdog::enforced_daemon_step(
                    &progress,
                    loop_watchdog::LoopPhase::PromptQueue,
                );
                daemon.drain_relay_jobs();
            }
            if !daemon.relay_jobs.writing(&format!("idle-check:{row}"))
                && !daemon.relay_jobs.writing(&format!("idle-unacked:{row}"))
            {
                break;
            }
            assert!(Instant::now() < deadline, "held validation never reported");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            queue
                .peek_all(30)
                .unwrap()
                .iter()
                .filter(|candidate| candidate.id != row)
                .count(),
            0,
            "old idle alarm must not survive {change}"
        );
        assert!(
            daemon
                .queue_jobs
                .drain_for_shutdown(&root, Duration::from_secs(3))
        );
    }
}

#[tokio::test]
async fn cas_cc89_slow_flag_revalidation_does_not_hold_the_store_writer() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, mut daemon, queue, row) = fixture(false);
    let root = daemon.app.cas_dir().to_path_buf();
    daemon
        .normal_delivery_probes
        .get_mut(&row)
        .unwrap()
        .nudge_sent_at = Some(Instant::now() - NORMAL_DELIVERY_OBSERVE_WINDOW);
    daemon.normal_delivery_probes.get_mut(&row).unwrap().nudges =
        NORMAL_DELIVERY_MAX_NUDGES;
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    let release = hold_lane(&mut daemon, jobs::Lane::External);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        daemon.resolve_normal_delivery_probes(queue.as_ref()).await;
        if daemon
            .relay_jobs
            .writing(&format!("normal-flag-validate:{row}"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "flag revalidation never reached the held lane"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (written_tx, written_rx) = std::sync::mpsc::channel();
    let path = root.clone();
    daemon.queue_jobs.submit_external(
        jobs::JobSpec {
            kind: "PoisonWriterControl",
            key: "PoisonWriterControl".into(),
            lane: jobs::Lane::Write,
            criticality: jobs::Criticality::Critical,
            generation: 1,
            budget: Duration::from_millis(100),
        },
        move || {
            let id = crate::store::open_prompt_queue_store(&path)
                .unwrap()
                .enqueue("control", "observer", "writer progress")
                .unwrap();
            let _ = written_tx.send(id);
            Box::new(())
        },
    );
    let id = written_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("store writer was held by validation");
    assert!(queue.queued_prompt(id).unwrap().is_some());
    assert!(
        daemon.normal_delivery_probes.contains_key(&row),
        "validation is still held"
    );
    release.send(()).unwrap();
    while daemon.normal_delivery_probes.contains_key(&row) {
        daemon.drain_relay_jobs();
        assert!(Instant::now() < deadline, "released flag never settled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[tokio::test]
async fn cas_cc89_reset_cancels_a_claimed_pointer_before_file_delivery() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (temp, mut daemon, queue, row) = fixture(false);
    let inbox = setup_pointer(&mut daemon, queue.as_ref(), row, temp.path(), &mut env).await;
    let root = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    let release = hold_lane(&mut daemon, jobs::Lane::External);
    daemon
        .fallback_unsurfaced_teams_pointers(queue.as_ref())
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    let key = format!("teams-fallback:{row}:worker-a:file");
    while !daemon.relay_jobs.writing(&key) {
        daemon.drain_relay_jobs();
        assert!(
            Instant::now() < deadline,
            "pointer never reached held file phase"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(queue.teams_body_fallback_claimed(row, "worker-a").unwrap());
    // Even an in-flight-key refusal must delay the next ownership clone.
    let pointer_key = format!("teams-fallback:{row}:worker-a");
    daemon
        .queue_jobs
        .poison_jobs
        .pointers
        .get_mut(&pointer_key)
        .unwrap()
        .retry_at = Instant::now() - Duration::from_secs(1);
    let retried = Instant::now();
    daemon.drain_relay_jobs();
    assert!(
        daemon.queue_jobs.poison_jobs.pointers[&pointer_key].retry_at
            >= retried + Duration::from_millis(900),
        "an in-flight phase refusal must back off"
    );
    daemon.forget_probe_pane("worker-a");
    release.send(()).unwrap();
    while !daemon.queue_jobs.poison_jobs.pointers.is_empty() {
        daemon.drain_relay_jobs();
        assert!(Instant::now() < deadline, "cancelled claim never released");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!queue.teams_body_fallback_claimed(row, "worker-a").unwrap());
    let messages: Vec<super::super::teams::InboxMessage> =
        serde_json::from_str(&std::fs::read_to_string(inbox).unwrap()).unwrap();
    let original = queue.queued_prompt(row).unwrap().unwrap().prompt;
    assert_eq!(messages.len(), 1, "only the original pointer may remain");
    assert!(
        !messages[0].text.contains(&original),
        "cancelled job must not deliver full body to replacement"
    );
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

#[tokio::test]
async fn cas_cc89_shutdown_releases_a_pointer_claim_waiting_on_the_writer() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (temp, mut daemon, queue, row) = fixture(false);
    let _inbox = setup_pointer(&mut daemon, queue.as_ref(), row, temp.path(), &mut env).await;
    let root = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root, None);
    let release = hold_lane(&mut daemon, jobs::Lane::Write);
    daemon
        .fallback_unsurfaced_teams_pointers(queue.as_ref())
        .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    let key = format!("teams-fallback:{row}:worker-a:claim");
    while !daemon.relay_jobs.writing(&key) {
        daemon.drain_relay_jobs();
        assert!(Instant::now() < deadline, "claim never reached held writer");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        let _ = release.send(());
    });
    let worker = super::super::store_worker::StoreWorker::start();
    assert!(daemon.finish_shutdown_writes(&worker, Duration::from_secs(3)));
    releasing.join().unwrap();
    assert!(
        !queue.teams_body_fallback_claimed(row, "worker-a").unwrap(),
        "shutdown must not leave a durable claim without scheduling its body"
    );
    assert!(daemon.queue_jobs.poison_jobs.pointers.is_empty());
}

#[test]
#[should_panic(expected = "enforce mode: move it to the jobs lane")]
fn cas_cc89_old_sweep_shape_panics_under_enforcement_control() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_temp, daemon, queue, _row) = fixture(false);
    let progress = loop_watchdog::LoopProgress::new();
    let (_attachment, _enforce) =
        loop_watchdog::enforced_daemon_step(&progress, loop_watchdog::LoopPhase::PromptQueue);
    queue
        .delivery_stalled_candidates(&daemon.session_name, 600, 1800, 50)
        .unwrap();
}

#[tokio::test]
async fn cas_cc89_stopped_worker_does_not_starve_pointer_or_bounce() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let (temp, mut daemon, queue, row) = fixture(false);
    let inbox = setup_pointer(&mut daemon, queue.as_ref(), row, temp.path(), &mut env).await;
    let root = daemon.app.cas_dir().to_path_buf();
    let mut stopped = cas_types::Agent::new_with_role(
        "stopped-id".into(),
        "stopped-worker".into(),
        cas_types::AgentRole::Worker,
    );
    stopped.factory_session = Some(daemon.session_name.clone());
    stopped.status = cas_types::AgentStatus::Shutdown;
    crate::store::open_agent_store(&root)
        .unwrap()
        .register(&stopped)
        .unwrap();
    daemon.app.add_worker_name_for_test("stopped-worker");
    daemon.dead_workers.insert("stopped-worker".into());
    daemon.normal_delivery_probes.clear();
    rusqlite::Connection::open(root.join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=? WHERE id=?",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::minutes(31)).to_rfc3339(),
                row
            ],
        )
        .unwrap();
    daemon.queue_jobs = prompt_queue_jobs::QueueJobs::start_with_private_writer(root.clone(), None);
    daemon.queue_jobs.store(&root).unwrap();
    daemon.last_commander_mirror_scan = Some(Instant::now());
    daemon.last_prompt_poison_sweep = Some(Instant::now() - PROMPT_POISON_SWEEP_INTERVAL);
    let request = prompt_queue_jobs::QueueSelectRequest {
        cas_dir: root.clone(),
        session_name: daemon.session_name.clone(),
        supervisor_name: daemon.app.supervisor_name().into(),
        worker_names: daemon.app.worker_names().to_vec(),
        dead_workers: daemon.dead_workers.clone(),
        limit: 0,
        operator_replies: false,
        deferred_operator_replies: Default::default(),
        adopt_operator_notices: false,
    };
    let release = hold_lane(&mut daemon, jobs::Lane::Blocking);
    // Real select -> real delivery: the repeated stopped-recipient list is
    // produced by the same path that caused the production starvation.
    let selected = run_queue_select(&request, 1).unwrap();
    assert!(
        selected
            .shut_down_recipients
            .iter()
            .any(|name| name == "stopped-worker")
    );
    daemon.deliver_queue_selection(selected).await.unwrap();
    assert!(
        daemon
            .relay_jobs
            .reading(super::super::relay_jobs::Relay::TeamsPointers)
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon
        .queue_jobs
        .job_lanes()
        .unwrap()
        .lane_stats()
        .iter()
        .any(|stats| stats.running.is_some() || stats.queued > 0)
    {
        assert!(Instant::now() < deadline, "held scan never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // These passes must apply the completed scan, not invalidate it again.
    // The cadence gate stays closed, so a replacement scan cannot mask loss.
    let original = queue.queued_prompt(row).unwrap().unwrap().prompt;
    loop {
        let selected = run_queue_select(&request, 2).unwrap();
        assert!(
            selected
                .shut_down_recipients
                .iter()
                .any(|name| name == "stopped-worker")
        );
        daemon.deliver_queue_selection(selected).await.unwrap();
        let messages: Vec<super::super::teams::InboxMessage> =
            serde_json::from_str(&std::fs::read_to_string(&inbox).unwrap()).unwrap();
        let bounces = queue
            .peek_all(30)
            .unwrap()
            .iter()
            .filter(|message| message.source == "delivery-watchdog")
            .count();
        if messages
            .iter()
            .any(|message| message.text.contains(&original))
            && bounces > 0
        {
            assert_eq!(messages.len(), 2, "one pointer plus one fallback body");
            assert_eq!(bounces, 1, "one durable sender bounce");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stopped-worker selections starved fallback or bounce"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}
