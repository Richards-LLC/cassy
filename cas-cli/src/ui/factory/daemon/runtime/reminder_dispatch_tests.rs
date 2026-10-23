//! cas-f265: real reminder dispatcher -> queue -> supervisor PTY and alarms.
use super::super::supervisor_single_turn_tests::{fixture, fixture_child, teams_messages, typed_once};
use super::*;

fn child(test: &str) -> bool {
    fixture_child(&format!(
        "{}::{test}",
        module_path!().split_once("::").unwrap().1
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn epic_split_decided_reminder_survives_late_registration_cas_ae81() {
    if !child("epic_split_decided_reminder_survives_late_registration_cas_ae81") { return }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let cas = daemon.app.cas_dir().to_path_buf();
    let now = chrono::Utc::now();
    cas_store::insert_epic_split(&cas, &cas_store::EpicSplit {
        id: "split-decided".into(), epic_id: "cas-e".into(), local_agent_id: "single-sup".into(),
        peer_agent_id: "peer".into(), peer_name: "peer".into(), peer_machine: "other-host".into(),
        direction: cas_store::SplitDirection::Outgoing, mine: vec![], theirs: vec!["cas-child".into()],
        state: cas_store::SplitState::Accepted, reason: None, message_id: None,
        expires_at: now + chrono::Duration::minutes(30), created_at: now, decided_at: Some(now),
        decided_by: Some("peer".into()), cleanup_pending: false, expiry_notice_pending: false,
    }).unwrap();
    let reminders = crate::store::open_reminder_store(&cas).unwrap();
    for id in ["split-other", "split-decided"] {
        reminders.create_with_scope("single-sup", None, "continue split work",
            cas_store::ReminderTriggerType::Event, None, Some("epic_split_decided"),
            Some(&serde_json::json!({"split_id": id})), 0, Some(&daemon.session_name),
            None, false, None).unwrap();
    }
    daemon.process_reminders(&[]);
    daemon.process_reminders(&[]);
    let fired = reminders.list_recently_fired(60).unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(reminders.list_pending("single-sup").unwrap().len(), 1);
    let event = fired[0].fired_event.as_ref().unwrap();
    assert_eq!(event["data"]["split_id"], "split-decided");
    assert_eq!(event["data"]["state"], "accepted");
    let rows = open_prompt_queue_store(&cas).unwrap().peek_all(20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].origin, Some(cas_store::QueueOrigin::Daemon));
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
    assert_eq!(
        rows[0].source, rows[0].target,
        "self identity survives takeover"
    );
    let fired = reminders.list_recently_fired(60).unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(
        fired[0].owner_id, owner,
        "retain original durable provenance"
    );
    (rows[0].id, rows[0].prompt.clone())
}

/// Age a transport-delivered row past the unread and stall thresholds.
fn age_delivered(daemon: &FactoryDaemon, queue: &dyn cas_store::PromptQueueStore, id: i64) {
    queue.mark_transport_delivered(id).unwrap();
    let ago = |secs| (chrono::Utc::now() - chrono::Duration::seconds(secs)).to_rfc3339();
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=?1, transport_delivered_at=?2 WHERE id=?3",
            rusqlite::params![ago(2100), ago(1800), id],
        )
        .unwrap();
}

/// CI-event and local time reminders take the same single-turn path.
async fn idle_supervisor_reminder_wakes_once(ci: bool) {
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
    crate::store::open_event_store(daemon.app.cas_dir()).unwrap().record(&event).unwrap();
    daemon.app.refresh_data().unwrap();
    let agent = daemon.app.director_data().agents.iter().find(|a| a.id == "single-sup").unwrap();
    assert!(agent.latest_activity.is_some());
    assert!(agent.last_heartbeat.is_some_and(|at| (chrono::Utc::now() - at).num_seconds() < 60));
    let (id, body) = dispatch(&mut daemon, ci, false);
    if ci {
        // cas-6e355: a busy pane holds the CI wake with nothing delivered...
        let name = daemon.app.supervisor_name().to_string();
        daemon.pane_silent_since.insert(name.clone(), Instant::now());
        daemon.process_prompt_queue().await.unwrap();
        let queue = open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
        let held = queue.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(held.stage, cas_store::DeliveryStage::Gated, "{held:?}");
        assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
        assert!(teams_messages(&daemon).is_empty());
        // ...and becoming idle wakes it without waiting for the re-nudge
        // interval or spending the bounded delivery budget.
        daemon.pane_silent_since.insert(name, Instant::now() - Duration::from_secs(600));
    }
    daemon.process_prompt_queue().await.unwrap();
    let typed = typed_once(&root, &mut daemon, &body).await;
    daemon.process_prompt_queue().await.unwrap();
    assert_eq!(std::fs::read_to_string(root.path().join("input")).unwrap(), typed, "ci={ci}");
}

#[tokio::test(flavor = "current_thread")]
async fn cas_6e355_ci_reminder_wakes_once_when_supervisor_becomes_idle() {
    if child("cas_6e355_ci_reminder_wakes_once_when_supervisor_becomes_idle") {
        idle_supervisor_reminder_wakes_once(true).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_idle_supervisor_time_reminder_wakes_once_even_with_recent_checkpoint() {
    if child("cas_f265_idle_supervisor_time_reminder_wakes_once_even_with_recent_checkpoint") {
        idle_supervisor_reminder_wakes_once(false).await;
    }
}

/// cas-3866: give the fixture's idle Claude supervisor a real transcript whose
/// last turn ran one tool call to completion and ended. Claude Code replays the
/// call's `tool_use` in an `api-request-blob` record after its `tool_result`;
/// the transcript has been quiet for ten minutes, as an idle pane's is.
fn idle_transcript_replaying_a_finished_call(root: &tempfile::TempDir, daemon: &FactoryDaemon) {
    let agents = open_agent_store(daemon.app.cas_dir()).unwrap();
    let mut supervisor = agents.get("single-sup").unwrap();
    let clone = root.path().to_str().unwrap().to_string();
    supervisor.metadata.insert("clone_path".into(), clone.clone());
    // A live Claude supervisor stays registered Active between turns; the wake
    // read only consults the transcript of an Active (or Stale) registration.
    supervisor.status = cas_types::AgentStatus::Active;
    agents.register(&supervisor).unwrap();
    let slug: String = clone.chars().map(|c| if matches!(c, '/' | '.') { '-' } else { c }).collect();
    let dir = dirs::home_dir().unwrap().join(".claude/projects").join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    let call = r#"{"type":"tool_use","id":"toolu_3866","name":"Bash","input":{"command":"gh run view 42"}}"#;
    let lines = [
        format!(r#"{{"type":"assistant","timestamp":"2026-10-19T22:39:34Z","message":{{"role":"assistant","content":[{call}],"stop_reason":"tool_use"}}}}"#),
        r#"{"type":"user","timestamp":"2026-10-19T22:39:35Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_3866","content":"completed"}]}}"#.to_string(),
        format!(r#"{{"type":"api-request-blob","hash":"h","sessionId":"single-sup","message":{{"role":"assistant","content":[{call}]}}}}"#),
        r#"{"type":"assistant","timestamp":"2026-10-19T22:41:28Z","message":{"role":"assistant","content":[{"type":"text","text":"Waiting on the merge queue."}],"stop_reason":"end_turn"}}"#.to_string(),
    ];
    let path = dir.join("single-sup.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(600))
        .unwrap();
    // The wake gate reads this file, not an unknown transcript.
    let registered = agents.get("single-sup").unwrap();
    assert_eq!(
        crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(
            daemon.app.cas_dir(),
            &registered,
        ),
        Some(path),
        "{registered:?}"
    );
}

/// cas-3866: an idle Claude supervisor is woken by a fired `ci_completed`
/// reminder or a `supervisor_stalled` relay although its transcript replays a
/// finished tool call. Before the fix the replay read as "recipient has a tool
/// call in flight" and the gate declined both kinds for 12.6 h.
async fn idle_supervisor_with_replayed_call_wakes(stalled: bool) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    idle_transcript_replaying_a_finished_call(&root, &daemon);
    let body = if stalled {
        let body = crate::prompt_revalidation::attach_blocker_envelope(
            "<worker-attention kind=\"supervisor_stalled\" worker=\"supervisor\" notification_id=\"21387\">\n\
             Supervisor actionable-idle for 10m. Merge cas-675a.\n\
             Run `factory action=worker_status` and reassign or recover the worker as needed.\n\
             </worker-attention>",
            "ready work",
            None,
        );
        open_prompt_queue_store(daemon.app.cas_dir())
            .unwrap()
            .enqueue_idempotent(
                "lifecycle-wake:worker-attention:21387",
                daemon.app.supervisor_name(),
                &body,
                Some(&daemon.session_name),
                Some("supervisor_stalled: supervisor"),
                None,
                "worker-attention:21387",
                Some(&cas_store::QueueOrigin::Daemon),
            )
            .unwrap();
        body
    } else {
        dispatch(&mut daemon, true, false).1
    };
    daemon.process_prompt_queue().await.unwrap();
    typed_once(&root, &mut daemon, &body).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3866_idle_claude_supervisor_wakes_for_ci_reminder_past_replayed_tool_call() {
    if child("cas_3866_idle_claude_supervisor_wakes_for_ci_reminder_past_replayed_tool_call") {
        idle_supervisor_with_replayed_call_wakes(false).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3866_idle_claude_supervisor_wakes_for_stalled_relay_past_replayed_tool_call() {
    if child("cas_3866_idle_claude_supervisor_wakes_for_stalled_relay_past_replayed_tool_call") {
        idle_supervisor_with_replayed_call_wakes(true).await;
    }
}

/// cas-c0d0 (GH #1246): an idle Claude supervisor with a running background
/// task keeps repainting, so its pane is never 2 s silent. In the live cas-ca26 probe its
/// own fired reminder sat gated 98.5 s until an unrelated turn started. With
/// the transcript showing the turn ended, the first pass must type it once.
#[tokio::test(flavor = "current_thread")]
async fn cas_c0d0_repainting_idle_supervisor_wakes_once_for_its_own_reminder() {
    if !child("cas_c0d0_repainting_idle_supervisor_wakes_once_for_its_own_reminder") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    idle_transcript_replaying_a_finished_call(&root, &daemon);
    let (_, body) = dispatch(&mut daemon, false, false);
    let name = daemon.app.supervisor_name().to_string();
    daemon.pane_silent_since.insert(name.clone(), Instant::now());
    daemon.process_prompt_queue().await.unwrap();
    daemon.pane_silent_since.insert(name, Instant::now());
    let typed = typed_once(&root, &mut daemon, &body).await;
    daemon.process_prompt_queue().await.unwrap();
    assert_eq!(std::fs::read_to_string(root.path().join("input")).unwrap(), typed, "no second injection");
}

/// Also when the reminder's owner is a replaced supervisor (takeover).
async fn busy_self_reminder_is_never_abandoned(takeover: bool) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let (id, body) = dispatch(&mut daemon, true, takeover);
    let name = daemon.app.supervisor_name().to_string();
    daemon.pane_silent_since.insert(name.clone(), Instant::now());
    daemon.lifecycle_redelivery_counts.insert(id, LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    let queue = open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_ne!(
        report.stage,
        cas_store::DeliveryStage::Abandoned,
        "takeover={takeover}: a self reminder is not a failed inter-agent relay: {report:?}"
    );
    assert!(queue.queued_prompt(id).unwrap().unwrap().processed_at.is_none(), "retain reminder for next turn");
    assert!(report.confirmed_at.is_none());
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
    assert_eq!(queue.peek_all(20).unwrap().len(), 1, "no abandon/escalation notice");
    // Advance the existing durable park without sleeping for its two minutes.
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute("UPDATE prompt_queue SET next_attempt_at=NULL WHERE id=?", [id])
        .unwrap();
    daemon.pane_silent_since.insert(name, Instant::now() - Duration::from_secs(600));
    daemon.process_prompt_queue().await.unwrap();
    typed_once(&root, &mut daemon, &body).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_busy_self_reminder_never_becomes_an_abandoned_relay() {
    if child("cas_f265_busy_self_reminder_never_becomes_an_abandoned_relay") {
        busy_self_reminder_is_never_abandoned(false).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_replaced_owner_self_reminder_never_becomes_an_abandoned_relay() {
    if child("cas_f265_replaced_owner_self_reminder_never_becomes_an_abandoned_relay") {
        busy_self_reminder_is_never_abandoned(true).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_delegated_reminder_retains_sender_and_failure_accounting() {
    if !child("cas_f265_delegated_reminder_retains_sender_and_failure_accounting") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let mut owner = cas_types::Agent::new("other-id".into(), "another-agent".into());
    owner.factory_session = Some(daemon.session_name.clone());
    open_agent_store(daemon.app.cas_dir())
        .unwrap()
        .register(&owner)
        .unwrap();
    daemon.app.refresh_data().unwrap();
    crate::store::open_reminder_store(daemon.app.cas_dir())
        .unwrap()
        .create_with_scope(
            &owner.id,
            Some("single-sup"),
            "Review a delegated decision",
            cas_store::ReminderTriggerType::Time,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
            None,
            None,
            0,
            Some(&daemon.session_name),
            None,
            false,
            None,
        )
        .unwrap();
    daemon.process_reminders(&[]);
    let queue = open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let rows = queue.peek_all(20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source, "another-agent");
    assert_ne!(rows[0].source, rows[0].target);
    daemon
        .pane_silent_since
        .insert(daemon.app.supervisor_name().to_string(), Instant::now());
    daemon
        .lifecycle_redelivery_counts
        .insert(rows[0].id, LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    // cas-af23: the inter-agent wake failure stays visible, but a spent
    // budget cannot prove a delegated reminder's premise expired.
    let report = queue.message_delivery_report(rows[0].id).unwrap().unwrap();
    assert_ne!(report.stage, cas_store::DeliveryStage::Abandoned, "{report:?}");
    let detail = report.pending_detail.unwrap();
    assert!(detail.contains(&format!("notification_id={}", rows[0].id)), "{detail}");
    assert!(detail.contains("wake retries exhausted"), "{detail}");
    assert!(!detail.contains("expired premise"), "{detail}");
    assert!(queue.poll_unseen_for_recipient(&rows[0].target, Some(&daemon.session_name), 20)
        .unwrap().iter().any(|row| row.id == rows[0].id));
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
    age_delivered(&daemon, queue.as_ref(), id);
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
    let mut sender = cas_types::Agent::new_with_role(
        "other-id".into(),
        "another-agent".into(),
        cas_types::AgentRole::Worker,
    );
    sender.factory_session = Some(daemon.session_name.clone());
    open_agent_store(daemon.app.cas_dir())
        .unwrap()
        .register(&sender)
        .unwrap();
    let other = queue
        .enqueue_with_session(
            "another-agent",
            daemon.app.supervisor_name(),
            "A real unhandled decision",
            &daemon.session_name,
        )
        .unwrap();
    age_delivered(&daemon, queue.as_ref(), other);
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

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_busy_supervisor_reads_reminder_once_at_next_turn() {
    if !child("cas_f265_busy_supervisor_reads_reminder_once_at_next_turn") {
        return;
    }
    let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let (id, body) = dispatch(&mut daemon, true, false);
    daemon
        .pane_silent_since
        .insert(daemon.app.supervisor_name().to_string(), Instant::now());
    daemon.process_prompt_queue().await.unwrap();
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_AGENT_NAME", daemon.app.supervisor_name());
    env.set("CAS_FACTORY_SESSION", &daemon.session_name);
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
    let mut hook = cas_core::hooks::types::HookInput {
        session_id: "busy-reminder-supervisor".into(),
        cwd: root.path().to_string_lossy().into_owned(),
        hook_event_name: "UserPromptSubmit".into(),
        user_prompt: Some("Next turn after completing work".into()),
        prompt_id: Some("next-turn".into()),
        agent_role: Some("supervisor".into()),
        ..Default::default()
    };
    let first =
        crate::hooks::handlers::handle_user_prompt_submit(&hook, Some(daemon.app.cas_dir()))
            .unwrap();
    assert_eq!(
        first
            .user_prompt_context()
            .unwrap_or_default()
            .matches(&body)
            .count(),
        1
    );
    hook.prompt_id = Some("later-turn".into());
    let later =
        crate::hooks::handlers::handle_user_prompt_submit(&hook, Some(daemon.app.cas_dir()))
            .unwrap();
    assert!(
        !later
            .user_prompt_context()
            .unwrap_or_default()
            .contains(&body)
    );
    let queue = open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert_eq!(
        queue
            .recipient_receipt(id, daemon.app.supervisor_name())
            .unwrap(),
        Some(cas_store::SurfacingSource::HookSurfaced)
    );
    daemon.process_prompt_queue().await.unwrap();
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cas_f265_reminder_keeps_every_pane_veto_and_requires_known_recipient() {
    if !child("cas_f265_reminder_keeps_every_pane_veto_and_requires_known_recipient") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let (_, body) = dispatch(&mut daemon, true, false);
    let name = daemon.app.supervisor_name();
    let mut data = daemon.app.director_data().clone();
    let agent = data
        .agents
        .iter_mut()
        .find(|agent| agent.name == name)
        .unwrap();
    agent.last_heartbeat = Some(chrono::Utc::now());
    agent.latest_activity = Some(("last checkpoint".into(), chrono::Utc::now()));
    let safe = PaneWakeState {
        composer_dirty: false,
        ready_for_injection: true,
        silent_for: Some(Duration::from_secs(600)),
        tool_call: ToolCallEvidence::Idle,
    };
    let decide = |data: &crate::ui::factory::director::DirectorData, pane, sender| {
        FactoryDaemon::supervisor_wake_decision(
            data,
            name,
            name,
            sender,
            name,
            &body,
            pane,
            chrono::Utc::now(),
        )
    };
    assert!(decide(&data, safe, &WakeSender::Daemon).allowed);
    for pane in [
        PaneWakeState {
            composer_dirty: true,
            ..safe
        },
        PaneWakeState {
            ready_for_injection: false,
            ..safe
        },
        PaneWakeState {
            tool_call: ToolCallEvidence::InFlight,
            ..safe
        },
        PaneWakeState {
            silent_for: Some(Duration::ZERO),
            ..safe
        },
    ] {
        assert!(
            !decide(&data, pane, &WakeSender::Daemon).allowed,
            "{pane:?}"
        );
    }
    assert!(!decide(&data, safe, &WakeSender::Unattributed).allowed);
    data.agents.clear();
    assert!(
        !decide(&data, safe, &WakeSender::Daemon).allowed,
        "missing registry state is not proof of idle"
    );
}
