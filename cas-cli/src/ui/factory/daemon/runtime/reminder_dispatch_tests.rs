//! cas-f265: real reminder dispatcher -> queue -> supervisor PTY and alarms.
use super::super::supervisor_single_turn_tests::{fixture, fixture_child, teams_messages};
use super::*;

fn child(test: &str) -> bool {
    fixture_child(&format!(
        "{}::{test}",
        module_path!().split_once("::").unwrap().1
    ))
}

fn dispatch(daemon: &mut FactoryDaemon, ci: bool, takeover: bool) -> (i64, String) {
    let cas = daemon.app.cas_dir().to_path_buf();
    let reminders = crate::store::open_reminder_store(&cas).unwrap();
    let owner = if takeover { "old-sup" } else { "single-sup" };
    if takeover {
        let mut old = cas_types::Agent::new_with_role(
            owner.into(),
            "retired-supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        old.factory_session = Some("old-factory".into());
        old.status = cas_types::AgentStatus::Shutdown;
        open_agent_store(&cas).unwrap().register(&old).unwrap();
    }
    let session = if takeover {
        "old-factory"
    } else {
        &daemon.session_name
    };
    let filter =
        serde_json::json!({"sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "workflow": "CI"});
    reminders
        .create_with_scope(
            owner,
            None,
            "Review the completed CI result.",
            if ci {
                cas_store::ReminderTriggerType::Event
            } else {
                cas_store::ReminderTriggerType::Time
            },
            (!ci).then(|| chrono::Utc::now() - chrono::Duration::seconds(1)),
            ci.then_some("ci_completed"),
            ci.then_some(&filter),
            0,
            Some(session),
            None,
            ci || takeover,
            None,
        )
        .unwrap();
    if ci {
        let reminder = reminders.list_pending(owner).unwrap().pop().unwrap();
        let run = serde_json::from_value(serde_json::json!({"id": 42, "name": "CI", "head_branch": "factory/test",
            "head_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "html_url": "https://github.test/runs/42",
            "status": "completed", "conclusion": "success", "event": "push"})).unwrap();
        daemon
            .pending_ci_wakes
            .push(super::super::ci_watch::CiWake {
                reminder,
                condition: super::super::ci_watch::parse_ci_completed(&filter).unwrap(),
                runs: vec![run],
            });
    }
    daemon.process_reminders(&[]);
    daemon.process_reminders(&[]);
    let queue = open_prompt_queue_store(&cas).unwrap();
    let rows = queue.peek_all(20).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "one durable fired edge, no redispatch: {rows:?}"
    );
    assert_eq!(rows[0].origin, Some(cas_store::QueueOrigin::Daemon));
    assert_eq!(rows[0].target, daemon.app.supervisor_name());
    assert_eq!(reminders.list_recently_fired(60).unwrap().len(), 1);
    (rows[0].id, rows[0].prompt.clone())
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_ci_dispatch_wakes_idle_supervisor_once_even_with_recent_checkpoint() {
    if !child("cas_f265_ci_dispatch_wakes_idle_supervisor_once_even_with_recent_checkpoint") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    // The last completed turn can leave fresh registry activity and heartbeats
    // while the pane is now idle. Reminder wakes must not wait for their expiry.
    let mut event = cas_types::Event::new(
        cas_types::EventType::TaskNoteAdded,
        cas_types::EventEntityType::Task,
        "cas-epic",
        "Waiting for CI; last checkpoint",
    )
    .with_session("single-sup");
    event.created_at = chrono::Utc::now() - chrono::Duration::seconds(90);
    crate::store::open_event_store(daemon.app.cas_dir())
        .unwrap()
        .record(&event)
        .unwrap();
    daemon.app.refresh_data().unwrap();
    let agent = daemon
        .app
        .director_data()
        .agents
        .iter()
        .find(|a| a.id == "single-sup")
        .unwrap();
    assert!(agent.latest_activity.is_some());
    assert!(
        agent
            .last_heartbeat
            .is_some_and(|at| (chrono::Utc::now() - at).num_seconds() < 60)
    );
    let (id, body) = dispatch(&mut daemon, true, false);
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    let report = open_prompt_queue_store(daemon.app.cas_dir())
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert!(
        typed.contains(&body),
        "idle reminder must start its own body-bearing turn: {typed:?}; {report:?}"
    );
    assert_eq!(typed.matches('\r').count(), 1);
    assert!(teams_messages(&daemon).is_empty());
    daemon.process_prompt_queue().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("input")).unwrap(),
        typed
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_local_time_reminder_reuses_single_turn_path() {
    if !child("cas_f265_local_time_reminder_reuses_single_turn_path") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let (_, body) = dispatch(&mut daemon, false, false);
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert!(typed.contains(&body));
    assert_eq!(typed.matches('\r').count(), 1);
    assert!(teams_messages(&daemon).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_busy_self_reminder_never_becomes_an_abandoned_relay() {
    if !child("cas_f265_busy_self_reminder_never_becomes_an_abandoned_relay") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let (id, _) = dispatch(&mut daemon, true, false);
    daemon
        .pane_silent_since
        .insert(daemon.app.supervisor_name().to_string(), Instant::now());
    daemon
        .lifecycle_redelivery_counts
        .insert(id, LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    let queue = open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_ne!(
        report.stage,
        cas_store::DeliveryStage::Abandoned,
        "a self reminder is not a failed inter-agent relay: {report:?}"
    );
    assert!(
        queue
            .queued_prompt(id)
            .unwrap()
            .unwrap()
            .processed_at
            .is_none(),
        "retain reminder for next turn"
    );
    assert!(report.confirmed_at.is_none());
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_cross_session_self_reminder_has_no_unread_or_stall_alarm() {
    if !child("cas_f265_cross_session_self_reminder_has_no_unread_or_stall_alarm") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let (id, _) = dispatch(&mut daemon, true, true);
    let queue = open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    queue.mark_transport_delivered(id).unwrap();
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=?1, transport_delivered_at=?2 WHERE id=?3",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::seconds(2100)).to_rfc3339(),
                (chrono::Utc::now() - chrono::Duration::seconds(1800)).to_rfc3339(),
                id
            ],
        )
        .unwrap();
    daemon.enqueue_delivery_stalled_bounces(queue.as_ref());
    daemon.relay_idle_unacked_messages(queue.as_ref());
    let notices: Vec<_> = queue
        .peek_all(100)
        .unwrap()
        .into_iter()
        .filter(|row| {
            row.source == "delivery-watchdog"
                || row.prompt.contains("supervisor_unread")
                || row.prompt.starts_with("UNDELIVERED:")
        })
        .collect();
    assert!(
        notices.is_empty(),
        "semantic self-reminder takeover must not raise alarms: {notices:?}"
    );
    assert!(
        queue
            .message_delivery_report(id)
            .unwrap()
            .unwrap()
            .confirmed_at
            .is_none()
    );
    let other = queue
        .enqueue_with_session(
            "another-agent",
            daemon.app.supervisor_name(),
            "A real unhandled decision",
            &daemon.session_name,
        )
        .unwrap();
    queue.mark_transport_delivered(other).unwrap();
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=?1, transport_delivered_at=?2 WHERE id=?3",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::seconds(2100)).to_rfc3339(),
                (chrono::Utc::now() - chrono::Duration::seconds(1800)).to_rfc3339(),
                other
            ],
        )
        .unwrap();
    daemon.enqueue_delivery_stalled_bounces(queue.as_ref());
    let real_notices: Vec<_> = queue
        .peek_all(100)
        .unwrap()
        .into_iter()
        .filter(|row| row.source == "delivery-watchdog" || row.prompt.contains("supervisor_unread"))
        .collect();
    assert!(
        !real_notices.is_empty(),
        "the same watchdog scan must still flag genuinely unread inter-agent mail"
    );
}
