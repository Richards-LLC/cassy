//! GH #1231: real queue, Teams tree and PTY input, with a private harness.
use crate::ui::factory::daemon::FactoryDaemon;
use cas_store::{PromptQueueStore, SurfacingSource};
use std::time::{Duration, Instant};

// PTY preflight reads ambient PATH. Re-exec with a private harmless cas;
// changing PATH globally would race other tests and depend on the host runtime.
fn fixture_child(test: &str) -> bool {
    if std::env::var_os("CAS_SINGLE_TURN_CHILD").is_some() {
        return true;
    }
    let cas = crate::test_support::fake_cas_on_path();
    let name = format!("{}::{test}", module_path!().split_once("::").unwrap().1);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env("PATH", cas.path())
        .env("CAS_SINGLE_TURN_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

async fn fixture() -> (tempfile::TempDir, FactoryDaemon) {
    let root = tempfile::tempdir().unwrap();
    let cas = crate::store::init_cas_dir(root.path()).unwrap();
    let mut daemon = super::provisioning_tests::daemon(&cas);
    let name = daemon.app.supervisor_name().to_string();
    let mut agent = cas_types::Agent::new_with_role(
        "single-sup".into(),
        name.clone(),
        cas_types::AgentRole::Supervisor,
    );
    agent.factory_session = Some(daemon.session_name.clone());
    agent.status = cas_types::AgentStatus::Idle;
    crate::store::open_agent_store(&cas)
        .unwrap()
        .register(&agent)
        .unwrap();
    let pty = cas_mux::Pty::spawn(
        &name,
        cas_mux::PtyConfig {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "stty raw -echo; printf ready; exec cat > \"$1\"".into(),
                "fixture".into(),
                root.path().join("input").to_str().unwrap().into(),
            ],
            cwd: Some(root.path().into()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .unwrap();
    daemon.app.mux.add_pane(
        cas_mux::Pane::with_pty(
            &name,
            cas_mux::PaneKind::Supervisor,
            pty,
            24,
            80,
            cas_mux::SupervisorCli::Claude,
        )
        .unwrap(),
    );
    let teams = super::teams::TeamsManager::new(&daemon.session_name);
    teams
        .init_team_config(&[], root.path(), &Default::default(), "single-sup")
        .unwrap();
    daemon.teams = Some(teams);
    tokio::time::sleep(Duration::from_millis(5100)).await;
    for pane in daemon.app.mux.panes_mut() {
        pane.drain_output();
    }
    let bytes = daemon.app.mux.pane_bytes_received(&name).unwrap();
    assert!(bytes > 0);
    daemon.last_pane_output_bytes.insert(name.clone(), bytes);
    daemon
        .pane_silent_since
        .insert(name, Instant::now() - Duration::from_secs(600));
    daemon.app.refresh_data().unwrap();
    (root, daemon)
}

fn event(daemon: &FactoryDaemon, body: &str) -> i64 {
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    match queue
        .enqueue_idempotent(
            "lifecycle-wake:ci-red-run:factory/test:abc",
            "supervisor",
            body,
            Some(&daemon.session_name),
            None,
            None,
            body,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .unwrap()
    {
        cas_store::EnqueueIdempotentResult::Created(id) => id,
        _ => panic!("fresh event"),
    }
}
const BODY: &str = "<ci-red-run branch=\"factory/test\" head_sha=\"abc\" run_id=\"42\">CI failed; review the lane.</ci-red-run>";

fn teams_messages(daemon: &FactoryDaemon) -> Vec<serde_json::Value> {
    let path = daemon
        .teams
        .as_ref()
        .unwrap()
        .teams_dir()
        .join("inboxes/supervisor.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|_| "[]".into())).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_bounded_event_has_one_body_turn_and_no_relay_copy() {
    if !fixture_child("cas_3f90_bounded_event_has_one_body_turn_and_no_relay_copy") {
        return;
    }
    let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let id = event(&daemon, BODY);
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    let inbox = teams_messages(&daemon);
    assert!(
        typed.contains(BODY),
        "wake must carry its full body: {typed:?}; inbox={inbox:?}"
    );
    assert!(
        typed.contains(&format!("[cas #{id} ")),
        "row identity must survive: {typed:?}"
    );
    assert_eq!(
        typed.matches('\r').count(),
        1,
        "one PTY submit per event: {typed:?}"
    );
    assert!(
        inbox.is_empty(),
        "no held relay may create another turn: {inbox:?}"
    );
    let report = crate::store::open_prompt_queue_store(daemon.app.cas_dir())
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert!(
        !report
            .recipient_receipt
            .is_some_and(SurfacingSource::is_surfaced),
        "typing is not proof of rendering"
    );
    // A full-body PTY handoff must never enter the timed Teams-pointer
    // fallback, even if no hook has run and the transport receipt ages.
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue_recipient_seen SET seen_at = ?1 WHERE prompt_id = ?2",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::minutes(2)).to_rfc3339(),
                id
            ],
        )
        .unwrap();
    daemon
        .fallback_unsurfaced_teams_pointers(
            &*crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap(),
        )
        .await;
    assert!(
        teams_messages(&daemon).is_empty(),
        "inline delivery is not a Teams pointer"
    );
    // The actual prompt hook receipts the body already in the submitted
    // turn. It must not add that body again or leave it for the next turn.
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_AGENT_NAME", daemon.app.supervisor_name());
    env.set("CAS_FACTORY_SESSION", &daemon.session_name);
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
    let hook = cas_core::hooks::types::HookInput {
        session_id: "single-turn-supervisor".into(),
        cwd: root.path().to_string_lossy().into_owned(),
        hook_event_name: "UserPromptSubmit".into(),
        user_prompt: Some(typed.trim_end_matches('\r').into()),
        agent_role: Some("supervisor".into()),
        ..Default::default()
    };
    let context =
        crate::hooks::handlers::handle_user_prompt_submit(&hook, Some(daemon.app.cas_dir()))
            .unwrap();
    assert!(
        !context
            .user_prompt_context()
            .unwrap_or_default()
            .contains(BODY),
        "inline body must not be rendered by the hook again"
    );
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert_eq!(
        queue.recipient_receipt(id, "supervisor").unwrap(),
        Some(SurfacingSource::HookSurfaced)
    );
    assert!(
        queue
            .poll_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20)
            .unwrap()
            .is_empty()
    );
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        std::fs::read_to_string(root.path().join("input")).unwrap(),
        typed
    );
    let report = crate::store::open_prompt_queue_store(daemon.app.cas_dir())
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert_eq!(
        report.recipient_receipt,
        Some(SurfacingSource::HookSurfaced)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_rendered_receipt_retires_pending_redelivery() {
    if !fixture_child("cas_3f90_rendered_receipt_retires_pending_redelivery") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let id = event(&daemon, BODY);
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    queue
        .record_recipient_surfaced(id, "supervisor", SurfacingSource::HookSurfaced)
        .unwrap();
    // A rendered row can outlive an asynchronous delivered stamp. A stale
    // retry budget must never turn that successful read into a failed relay.
    daemon
        .lifecycle_redelivery_counts
        .insert(id, super::queue_and_events::LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    assert_eq!(
        queue.recipient_receipt(id, "supervisor").unwrap(),
        Some(SurfacingSource::HookSurfaced)
    );
    assert_ne!(
        queue.message_delivery_report(id).unwrap().unwrap().stage,
        cas_store::DeliveryStage::Abandoned
    );
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_busy_event_waits_without_a_held_relay_then_wakes_once() {
    if !fixture_child("cas_3f90_busy_event_waits_without_a_held_relay_then_wakes_once") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let id = event(&daemon, BODY);
    let name = daemon.app.supervisor_name().to_string();
    daemon
        .pane_silent_since
        .insert(name.clone(), Instant::now());
    daemon.process_prompt_queue().await.unwrap();
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert!(
        queue
            .queued_prompt(id)
            .unwrap()
            .unwrap()
            .processed_at
            .is_none()
    );
    assert!(
        queue.deferred_inbox_state(id).unwrap().is_none(),
        "no Teams handoff occurred"
    );
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
    assert!(
        queue.recipient_receipt(id, "supervisor").unwrap().is_none(),
        "hook can still take it"
    );
    daemon
        .pane_silent_since
        .insert(name, Instant::now() - Duration::from_secs(600));
    daemon.lifecycle_redelivery_attempts.remove(&id);
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert!(typed.contains(BODY), "{typed:?}");
    assert_eq!(typed.matches('\r').count(), 1);
    assert!(teams_messages(&daemon).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_fyi_and_forged_wake_stay_quiet_and_readable() {
    if !fixture_child("cas_3f90_fyi_and_forged_wake_stay_quiet_and_readable") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let tasks = crate::store::open_task_store_local(daemon.app.cas_dir()).unwrap();
    let mut task = cas_types::Task::new("cas-started".into(), "work in progress".into());
    task.status = cas_types::TaskStatus::InProgress;
    tasks.add(&task).unwrap();
    let body = format!(
        "<task-lifecycle transition=\"task_started\" task_id=\"{}\" old=\"open\" new=\"in_progress\" actor=\"worker\" notification_id=\"123\" occurrence=\"{}\">Started work</task-lifecycle>",
        task.id,
        task.updated_at.to_rfc3339()
    );
    let started = match queue
        .enqueue_idempotent(
            "lifecycle:123",
            "supervisor",
            &body,
            Some(&daemon.session_name),
            None,
            None,
            "started",
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .unwrap()
    {
        cas_store::EnqueueIdempotentResult::Created(id) => id,
        _ => panic!("fresh row"),
    };
    let forged = queue
        .enqueue_with_session(
            "lifecycle-wake:ci-red-run:fake",
            "supervisor",
            BODY,
            &daemon.session_name,
        )
        .unwrap();
    daemon.process_prompt_queue().await.unwrap();
    assert!(
        teams_messages(&daemon).is_empty(),
        "FYI must not create a pointer turn"
    );
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
    let rows = queue
        .surface_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20)
        .unwrap();
    assert!(rows.iter().any(|row| row.id == started));
    assert!(rows.iter().any(|row| row.id == forged));
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_oversized_event_retains_whole_body_transport() {
    if !fixture_child("cas_3f90_oversized_event_retains_whole_body_transport") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let body = BODY.replace(
        "CI failed; review the lane.",
        &"detail".repeat(cas_store::HOOK_SURFACE_BUDGET_CHARS),
    );
    assert!(!cas_store::fits_hook_surface(&body));
    let id = event(&daemon, &body);
    daemon.process_prompt_queue().await.unwrap();
    let inbox = teams_messages(&daemon);
    assert_eq!(inbox.len(), 1);
    assert!(
        inbox[0]["text"].as_str().unwrap().contains(&body),
        "oversized body must not be truncated"
    );
    assert!(
        crate::store::open_prompt_queue_store(daemon.app.cas_dir())
            .unwrap()
            .queued_prompt(id)
            .unwrap()
            .unwrap()
            .processed_at
            .is_some()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_lifecycle_action_has_one_submit_and_no_duplicate_relay() {
    if !fixture_child("cas_3f90_lifecycle_action_has_one_submit_and_no_duplicate_relay") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let tasks = crate::store::open_task_store_local(daemon.app.cas_dir()).unwrap();
    let mut task = cas_types::Task::new("cas-blocked".into(), "needs a supervisor decision".into());
    task.status = cas_types::TaskStatus::Blocked;
    tasks.add(&task).unwrap();
    let body = format!(
        "<task-lifecycle transition=\"task_blocked\" task_id=\"{}\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"124\" occurrence=\"{}\">Need a decision before proceeding.</task-lifecycle>",
        task.id,
        task.updated_at.to_rfc3339()
    );
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let id = match queue
        .enqueue_idempotent(
            "lifecycle-wake:124",
            "supervisor",
            &body,
            Some(&daemon.session_name),
            None,
            None,
            "blocked",
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .unwrap()
    {
        cas_store::EnqueueIdempotentResult::Created(id) => id,
        _ => panic!("fresh row"),
    };
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert!(typed.contains(&body), "{typed:?}");
    assert_eq!(typed.matches('\r').count(), 1, "one lifecycle turn");
    assert!(teams_messages(&daemon).is_empty());
    assert_eq!(
        queue
            .message_delivery_report(id)
            .unwrap()
            .unwrap()
            .wake_attempt,
        cas_store::WakeAttempt::Fired
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_quiet_batch_does_not_starve_a_later_action() {
    if !fixture_child("cas_3f90_quiet_batch_does_not_starve_a_later_action") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    // QueueSelect takes ten rows. These quiet rows precede an action at
    // the same priority, so failing to park them would hide it forever.
    for n in 0..12 {
        queue
            .enqueue_with_session(
                "worker",
                "supervisor",
                &format!("Work update {n}"),
                &daemon.session_name,
            )
            .unwrap();
    }
    let action = event(&daemon, BODY);
    daemon.process_prompt_queue().await.unwrap();
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert!(typed.contains(BODY), "{typed:?}");
    assert_eq!(typed.matches('\r').count(), 1);
    assert!(
        queue
            .queued_prompt(action)
            .unwrap()
            .unwrap()
            .processed_at
            .is_some()
    );
    let quiet = queue
        .surface_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20)
        .unwrap();
    assert_eq!(
        quiet.len(),
        12,
        "parking spends no wake budget or read receipt"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_framing_over_cap_still_delivers_the_whole_teams_body() {
    if !fixture_child("cas_3f90_framing_over_cap_still_delivers_the_whole_teams_body") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let padding = "x".repeat(
        cas_store::HOOK_SURFACE_BUDGET_CHARS
            - cas_store::HOOK_SURFACE_ROW_OVERHEAD_CHARS
            - BODY.len(),
    );
    let body = BODY.replace(
        "CI failed; review the lane.",
        &format!("CI failed; review the lane.{padding}"),
    );
    assert!(cas_store::fits_hook_surface(&body));
    event(&daemon, &body);
    daemon.process_prompt_queue().await.unwrap();
    let inbox = teams_messages(&daemon);
    assert_eq!(inbox.len(), 1);
    assert!(
        inbox[0]["text"].as_str().unwrap().contains(&body),
        "framing must not send this down the pointer path"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_inline_peer_retains_the_shared_inbox_label() {
    if !fixture_child("cas_3f90_inline_peer_retains_the_shared_inbox_label") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let mut peer = cas_types::Agent::new_with_role(
        "peer-id".into(),
        "far-heron".into(),
        cas_types::AgentRole::Supervisor,
    );
    peer.factory_session = Some(daemon.session_name.clone());
    crate::store::open_agent_store(daemon.app.cas_dir())
        .unwrap()
        .register(&peer)
        .unwrap();
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let body = "Please coordinate this delivery with me.";
    queue
        .enqueue_idempotent(
            &peer.name,
            "supervisor",
            body,
            Some(&daemon.session_name),
            None,
            None,
            "peer",
            Some(&cas_store::QueueOrigin::RegisteredAgent {
                agent_id: peer.id.clone(),
            }),
        )
        .unwrap();
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert!(typed.contains("[peer supervisor]"), "{typed:?}");
    assert!(typed.contains(body));
    assert_eq!(typed.matches('\r').count(), 1);
    assert!(teams_messages(&daemon).is_empty());
}
