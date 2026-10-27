//! GH #1231: real queue, Teams tree and PTY input, with a private harness.
use crate::ui::factory::daemon::FactoryDaemon;
use cas_store::SurfacingSource;
use std::time::{Duration, Instant};

// PTY preflight reads ambient PATH. Re-exec with a private harmless cas;
// changing PATH globally would race other tests and depend on the host runtime.
pub(super) fn fixture_child(test: &str) -> bool {
    if std::env::var_os("CAS_SINGLE_TURN_CHILD").is_some() {
        return true;
    }
    let cas = crate::test_support::fake_cas_on_path();
    let name = if test.starts_with("ui::") {
        test.to_string()
    } else {
        format!("{}::{test}", module_path!().split_once("::").unwrap().1)
    };
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

pub(super) async fn fixture() -> (tempfile::TempDir, FactoryDaemon) {
    fixture_for(cas_mux::SupervisorCli::Claude).await
}

/// [`fixture`] with the supervisor launched under `cli`.
pub(super) async fn fixture_for(cli: cas_mux::SupervisorCli) -> (tempfile::TempDir, FactoryDaemon) {
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
    let mut pane = cas_mux::Pane::with_pty(
            &name,
            cas_mux::PaneKind::Supervisor,
            pty,
            24,
            80,
            cli,
        )
        .unwrap();
    // This private shell has no harness startup phase to wait through.
    pane.backdate_spawn_for_test(Duration::from_secs(6));
    daemon.app.mux.add_pane(pane);
    daemon.app.set_supervisor_cli_for_test(cli);
    let teams = super::teams::TeamsManager::new(&daemon.session_name);
    teams
        .init_team_config(&[], root.path(), &Default::default(), "single-sup")
        .unwrap();
    daemon.teams = Some(teams);
    let mut banner = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for pane in daemon.app.mux.panes_mut() {
                banner.extend(pane.drain_output().0);
            }
            if banner.windows(b"ready".len()).any(|bytes| bytes == b"ready")
                && root.path().join("input").is_file()
                && daemon.app.mux.pane_ready_for_injection(&name)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("private supervisor must emit ready and open its input file");
    let bytes = daemon.app.mux.pane_bytes_received(&name).unwrap();
    assert!(bytes > 0);
    daemon.last_pane_output_bytes.insert(name.clone(), bytes);
    daemon
        .pane_silent_since
        .insert(name, Instant::now() - Duration::from_secs(600));
    daemon.app.refresh_data().unwrap();
    (root, daemon)
}

fn enqueue(daemon: &FactoryDaemon, source: &str, body: &str, origin: cas_store::QueueOrigin) -> i64 {
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    match queue
        .enqueue_idempotent(source, "supervisor", body, Some(&daemon.session_name), None, None, body, Some(&origin))
        .unwrap()
    {
        cas_store::EnqueueIdempotentResult::Created(id) => id,
        _ => panic!("fresh event"),
    }
}

fn event(daemon: &FactoryDaemon, body: &str) -> i64 {
    enqueue(daemon, "lifecycle-wake:ci-red-run:factory/test:abc", body, cas_store::QueueOrigin::Daemon)
}

/// GH #1400: a protected draft is a recoverable hold, even beyond the
/// transport retry budget. The independent operator lane must still work.
#[tokio::test(flavor = "current_thread")]
async fn codex_dirty_composer_keeps_relay_and_escalates_without_touching_draft() {
    if !fixture_child("codex_dirty_composer_keeps_relay_and_escalates_without_touching_draft") {
        return;
    }
    let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture_for(cas_mux::SupervisorCli::Codex).await;
    env.set("CAS_FACTORY_SESSION", &daemon.session_name);
    let cas = daemon.app.cas_dir().to_path_buf();
    let name = daemon.app.supervisor_name().to_string();
    let draft = "operator draft: keep this text";
    daemon.app.mux.get(&name).unwrap()
        .deliver_user_input(draft.as_bytes(), cas_mux::UserInputKind::KeyStream).await.unwrap();
    let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
    // Use the production typed-relay producer, not a hand-written envelope.
    assert!(matches!(super::lifecycle::enqueue_supervisor_unread_relay(
        &cas, "waiting-worker", 1400, 1,
    ), super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. }));
    let relay = queue.peek_all(10).unwrap().into_iter()
        .find(|row| row.source.starts_with("lifecycle-wake:worker-attention:")).unwrap();
    assert!(relay.prompt.contains("kind=\"supervisor_unread\""));
    let id = relay.id;
    let conn = rusqlite::Connection::open(cas.join("cas.db")).unwrap();
    // Advance durable time, never sleep through a 15-minute retry window.
    conn.execute(
        "UPDATE prompt_queue SET created_at=?1, first_attempt_at=?1, delivery_attempts=119 WHERE id=?2",
        rusqlite::params![(chrono::Utc::now() - chrono::Duration::minutes(16)).to_rfc3339(), id],
    ).unwrap();
    daemon.process_prompt_queue().await.unwrap();
    settle_injected_writes(&mut daemon).await;
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.stage, cas_store::DeliveryStage::Gated, "protected draft must not abandon a relay: {report:?}");
    assert!(report.delivered_at.is_none() && report.confirmed_at.is_none());
    assert!(queue.queued_prompt(id).unwrap().unwrap().processed_at.is_none());
    let (attempts, next): (i64, Option<String>) = conn.query_row(
        "SELECT delivery_attempts,next_attempt_at FROM prompt_queue WHERE id=?", [id],
        |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(attempts, 119, "a pre-write safety hold spends no transport attempts");
    assert!(next.is_some(), "the durable hold must schedule its next check");
    super::queue_and_events::escalate_undelivered_supervisor_relays(
        &cas, queue.as_ref(), &daemon.session_name, &name, "Codex", chrono::Utc::now(),
    );
    let alerts = queue.peek_operator_replies(&daemon.session_name, 10).unwrap();
    assert_eq!(alerts.len(), 1, "one durable escalation outside the blocked supervisor pane");
    let held = queue.list_undelivered_lifecycle_relays(10).unwrap()
        .into_iter().find(|row| row.prompt_id == id).unwrap();
    assert!(held.next_attempt_at.is_some());
    assert_eq!(held.operator_alert_id, Some(alerts[0].id));
    let payload: crate::ui::factory::OperatorReplyPayload = serde_json::from_str(&alerts[0].prompt).unwrap();
    assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Blocker);
    assert!(super::queue_and_events::escalate_undelivered_supervisor_relays(
        &cas, queue.as_ref(), &daemon.session_name, &name, "Codex", chrono::Utc::now(),
    ).is_empty(), "escalation survives repeated sweeps without duplicates");
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(root.path().join("input")).unwrap() != draft {
            tokio::task::yield_now().await;
        }
    }).await.expect("only the operator's draft reaches the PTY while held");
    assert!(daemon.app.mux.get(&name).unwrap().is_composer_dirty());
    // Only an explicit operator action clears the draft. The next durable
    // check can now submit the original body exactly once.
    daemon.app.mux.get(&name).unwrap()
        .deliver_user_input(b"\x15", cas_mux::UserInputKind::KeyStream).await.unwrap();
    conn.execute("UPDATE prompt_queue SET next_attempt_at=?1 WHERE id=?2",
        rusqlite::params![chrono::Utc::now().to_rfc3339(), id]).unwrap();
    typed_once(&root, &mut daemon, "1 message addressed to you has gone unread").await;
    settle_injected_writes(&mut daemon).await;
    let delivered = queue.message_delivery_report(id).unwrap().unwrap();
    assert!(delivered.delivered_at.is_some());
    assert!(delivered.confirmed_at.is_none(), "PTY submit is not an observed Codex model turn");
}

/// A task-lifecycle wake body for a freshly stored task.
fn lifecycle_body(daemon: &FactoryDaemon, status: cas_types::TaskStatus, transition: &str, old: &str, notification: u32, text: &str) -> String {
    let mut task = cas_types::Task::new(format!("cas-{notification}"), text.into());
    task.status = status;
    crate::store::open_task_store_local(daemon.app.cas_dir()).unwrap().add(&task).unwrap();
    format!(
        "<task-lifecycle transition=\"{transition}\" task_id=\"{}\" old=\"{old}\" new=\"{}\" actor=\"worker\" notification_id=\"{notification}\" occurrence=\"{}\">{text}</task-lifecycle>",
        task.id,
        status,
        task.updated_at.to_rfc3339()
    )
}

/// Advance only already-admitted write commands, never select another row.
/// A queued payload is deliberately not a completed transport handoff.
pub(super) async fn settle_injected_writes(daemon: &mut FactoryDaemon) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while daemon.app.mux.has_pending_injects() {
            daemon.advance_urgent_injects().await;
            tokio::task::yield_now().await;
        }
    }).await.expect("admitted payload and submit writes must complete");
    let cas = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs.settle_injects(&cas);
    daemon.queue_jobs.apply_completions();
}

/// The supervisor PTY received exactly one submitted turn containing `needle`
/// and no Teams relay was written.
pub(super) async fn typed_once(root: &tempfile::TempDir, daemon: &mut FactoryDaemon, needle: &str) -> String {
    // cas_f265: queue admission, PTY write completion and the shell consuming
    // those bytes happen on different turns/threads. A single pass followed by
    // a file read can observe an empty/partial turn even after a fixed delay.
    let mut typed = String::new();
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            daemon.process_prompt_queue().await.unwrap();
            typed = std::fs::read_to_string(root.path().join("input")).unwrap();
            if typed.contains(needle) && typed.ends_with('\r')
                && !daemon.app.mux.has_pending_injects()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await;
    assert!(completed.is_ok(), "submitted turn did not reach the consumer: {typed:?}");
    assert_eq!(typed.matches('\r').count(), 1, "one PTY submit: {typed:?}");
    assert!(teams_messages(daemon).is_empty(), "no relay copy");
    typed
}

/// A completed write to the PTY is not a completed read by the fixture's cat.
/// Hold that consumer until the queue receipts submit, then let it run on the
/// next Tokio turn, as a loaded host can naturally do (cas-470e1 / cas_f265).
#[tokio::test(flavor = "current_thread")]
async fn typed_once_waits_for_the_submitted_turn_to_reach_the_consumer() {
    if !fixture_child("typed_once_waits_for_the_submitted_turn_to_reach_the_consumer") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let pid = daemon.app.mux.pane_process_group_id(daemon.app.supervisor_name()).unwrap();
    struct Resume(u32);
    impl Drop for Resume {
        fn drop(&mut self) {
            // This is only the private shell process created by fixture().
            unsafe { libc::kill(self.0 as i32, libc::SIGCONT); }
        }
    }
    let _resume_on_failure = Resume(pid);
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
    let id = event(&daemon, BODY);
    daemon.process_prompt_queue().await.unwrap();
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let resume = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            while queue.queued_prompt(id).unwrap().unwrap().processed_at.is_none() {
                tokio::task::yield_now().await;
            }
            assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGCONT) }, 0);
        }).await.expect("physical submit must be receipted before the consumer resumes");
    });
    typed_once(&root, &mut daemon, BODY).await;
    resume.await.unwrap();
}

fn nothing_typed_or_relayed(root: &tempfile::TempDir, daemon: &FactoryDaemon) {
    assert!(teams_messages(daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
}
const BODY: &str = "<ci-red-run branch=\"factory/test\" head_sha=\"abc\" run_id=\"42\">CI failed; review the lane.</ci-red-run>";

pub(super) fn teams_messages(daemon: &FactoryDaemon) -> Vec<serde_json::Value> {
    let path = daemon
        .teams
        .as_ref()
        .unwrap()
        .teams_dir()
        .join("inboxes/supervisor.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|_| "[]".into())).unwrap()
}

/// cas-979e: on the daemon's job lanes a fresh claim comes back a pass after
/// it is submitted. A fresh Codex supervisor's startup brief must be typed on
/// that pass, not held by the re-nudge cadence its claim-only pass reserved.
#[tokio::test(flavor = "current_thread")]
async fn cas_979e_codex_startup_brief_is_typed_once_on_job_lanes() {
    if !fixture_child("cas_979e_codex_startup_brief_is_typed_once_on_job_lanes") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture_for(cas_mux::SupervisorCli::Codex).await;
    let cas = daemon.app.cas_dir().to_path_buf();
    daemon.queue_jobs = super::prompt_queue_jobs::QueueJobs::start_with_private_writer(
        cas.clone(),
        daemon.app.project_scope(),
    );
    daemon.queue_jobs.store(&cas).unwrap();
    let name = daemon.app.supervisor_name().to_string();
    // The real launch enqueue (init.rs), as `cas codex` runs it.
    crate::ui::factory::queue_supervisor_intro_prompt(
        &cas, &name, cas_mux::SupervisorCli::Codex, &[], None, None,
    );
    let needle = "Codex supervisor startup";
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        daemon.app.mux.flush_deferred_injections().await;
        daemon.process_prompt_queue().await.unwrap();
        if std::fs::read_to_string(root.path().join("input")).unwrap().contains(needle) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    typed_once(&root, &mut daemon, needle).await;
}

/// GH #1269: the asynchronous claim is not a submitted reminder/reply, and a
/// physical submit is not evidence that the real Codex harness accepted a turn.
#[tokio::test(flavor = "current_thread")]
async fn codex_reminder_and_peer_reply_confirm_submit_after_async_claim_cas_353b() {
    if !fixture_child("codex_reminder_and_peer_reply_confirm_submit_after_async_claim_cas_353b") {
        return;
    }
    #[derive(Clone)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    for (source, prompt) in [
        (None, "Reminder #266: Check the pending reply."),
        (Some("peer:remote@soundwave"), "<cas-peer-message v=\"1\" id=\"reply\" from=\"remote\" from_agent=\"remote-id\" machine=\"soundwave\" repo=\"cloud\" in_reply_to=\"request\"/>\nActionable reply."),
    ] {
        let (root, mut daemon) = fixture_for(cas_mux::SupervisorCli::Codex).await;
        let cas = daemon.app.cas_dir().to_path_buf();
        let name = daemon.app.supervisor_name().to_string();
        daemon.queue_jobs = super::prompt_queue_jobs::QueueJobs::start_with_private_writer(
            cas.clone(), daemon.app.project_scope(),
        );
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let id = match queue.enqueue_idempotent(
            source.unwrap_or(&name), &name, prompt, Some(&daemon.session_name),
            None, None, prompt, Some(&cas_store::QueueOrigin::Daemon),
        ).unwrap() {
            cas_store::EnqueueIdempotentResult::Created(id) => id,
            _ => panic!("fresh reminder/reply"),
        };
        // Peer replies share 3.67's 20-second directed-message debounce. Set
        // the fixture's persisted clock past it instead of waiting or bypassing
        // the real selection path (as peer_mailbox_wake_tests does).
        if source.is_some() {
            rusqlite::Connection::open(cas.join("cas.db")).unwrap().execute(
                "UPDATE prompt_queue SET created_at=?1,next_attempt_at=?1 WHERE id=?2",
                rusqlite::params![(chrono::Utc::now()-chrono::Duration::seconds(21)).to_rfc3339(), id],
            ).unwrap();
        }
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = output.clone();
        let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || Capture(sink.clone())).finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let log = || String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !daemon.queue_jobs.has_claim(id, &name) {
            daemon.process_prompt_queue().await.unwrap();
            assert!(Instant::now() < deadline, "claim must be submitted: {prompt}\n{}", log());
            tokio::task::yield_now().await;
        }
        assert_eq!(daemon.lifecycle_redelivery_counts.get(&id).copied().unwrap_or(0), 0,
            "claim-only pass must not spend the wake budget (cas-979e)");
        assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
        while daemon.urgent_injects.owners.is_empty() {
            daemon.process_prompt_queue().await.unwrap();
            assert!(Instant::now() < deadline, "completed claim must reach PTY: {prompt}");
            tokio::task::yield_now().await;
        }
        // pin: this log stage promises physical submit, never queue admission.
        assert!(!log().contains("pty_submit_confirmed"));
        assert!(queue.queued_prompt(id).unwrap().unwrap().processed_at.is_none());
        loop {
            daemon.process_prompt_queue().await.unwrap();
            if queue.queued_prompt(id).unwrap().unwrap().processed_at.is_some()
                && std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") {
                break;
            }
            assert!(Instant::now() < deadline, "payload and submit must complete: {prompt}");
            tokio::task::yield_now().await;
        }
        let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
        assert!(typed.contains(prompt), "original body must reach PTY: {typed:?}");
        assert_eq!(typed.matches('\r').count(), 1, "exactly one physical submit: {typed:?}");
        assert!(teams_messages(&daemon).is_empty(), "Codex never uses the Claude inbox");
        let logs = log();
        // pin: operators join dispatch to physical completion by stage, row and pane.
        let confirmed: Vec<_> = logs.lines().filter(|line| line.contains("pty_submit_confirmed")).collect();
        assert_eq!(confirmed.len(), 1, "one confirmed submit, not just an injection attempt: {logs}");
        assert!(confirmed[0].contains(&format!("message_id={id}")) && confirmed[0].contains(&name), "{logs}");
        assert!(confirmed[0].contains("unconfirmed"), "a shell PTY cannot prove a Codex turn: {logs}");
        // pin: dispatch attempts have a distinct stage from physical completion.
        assert!(logs.contains("prompt_inject_attempt"), "{logs}");
        assert!(daemon.queue_jobs.drain_for_shutdown(&cas, Duration::from_secs(3)));
    }
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
    let typed = typed_once(&root, &mut daemon, BODY).await;
    assert!(typed.contains(&format!("[cas #{id} ")), "row identity must survive: {typed:?}");
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let receipt = || queue.message_delivery_report(id).unwrap().unwrap().recipient_receipt;
    assert!(!receipt().is_some_and(SurfacingSource::is_surfaced), "typing is not proof of rendering");
    // A full-body PTY handoff must never enter the timed Teams-pointer
    // fallback, even if no hook has run and the transport receipt ages.
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue_recipient_seen SET seen_at = ?1 WHERE prompt_id = ?2",
            rusqlite::params![(chrono::Utc::now() - chrono::Duration::minutes(2)).to_rfc3339(), id],
        )
        .unwrap();
    daemon.fallback_unsurfaced_teams_pointers(&*queue).await;
    assert!(teams_messages(&daemon).is_empty(), "inline delivery is not a Teams pointer");
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
        user_prompt: Some(format!("<pasted_content id=\"1\">{}</pasted_content>",
            typed.trim_end_matches('\r').trim_start_matches("\x1b[200~").trim_end_matches("\x1b[201~"))),
        agent_role: Some("supervisor".into()),
        ..Default::default()
    };
    let context =
        crate::hooks::handlers::handle_user_prompt_submit(&hook, Some(daemon.app.cas_dir())).unwrap();
    assert!(
        !context.user_prompt_context().unwrap_or_default().contains(BODY),
        "inline body must not be rendered by the hook again"
    );
    assert_eq!(receipt(), Some(SurfacingSource::HookSurfaced));
    assert!(queue.poll_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20).unwrap().is_empty());
    daemon.process_prompt_queue().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(std::fs::read_to_string(root.path().join("input")).unwrap(), typed, "no second turn");
    assert_eq!(receipt(), Some(SurfacingSource::HookSurfaced));
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
    queue.record_recipient_surfaced(id, "supervisor", SurfacingSource::HookSurfaced).unwrap();
    // A rendered row can outlive an asynchronous delivered stamp. A stale
    // retry budget must never turn that successful read into a failed relay.
    daemon
        .lifecycle_redelivery_counts
        .insert(id, super::queue_and_events::LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    assert_eq!(queue.recipient_receipt(id, "supervisor").unwrap(), Some(SurfacingSource::HookSurfaced));
    assert_ne!(queue.message_delivery_report(id).unwrap().unwrap().stage, cas_store::DeliveryStage::Abandoned);
    nothing_typed_or_relayed(&root, &daemon);
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_busy_event_waits_without_a_held_relay_then_wakes_once() {
    if !fixture_child("cas_3f90_busy_event_waits_without_a_held_relay_then_wakes_once") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let body = BODY.replace("review the lane.", &format!("review the lane.{}", " detail".repeat(180)));
    let id = event(&daemon, &body);
    let name = daemon.app.supervisor_name().to_string();
    daemon.pane_silent_since.insert(name.clone(), Instant::now());
    daemon.process_prompt_queue().await.unwrap();
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert!(queue.queued_prompt(id).unwrap().unwrap().processed_at.is_none());
    assert!(queue.deferred_inbox_state(id).unwrap().is_none(), "no Teams handoff occurred");
    nothing_typed_or_relayed(&root, &daemon);
    assert!(queue.recipient_receipt(id, "supervisor").unwrap().is_none(), "hook can still take it");
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.pending_reason, Some(cas_store::PendingReason::AwaitingBusyRecipient), "busy inline rows leave selection until their bounded recheck/turn end");
    let parked_wake_state = queue.wake_gate_state(id).unwrap();
    for _ in 0..3 {
        daemon.process_prompt_queue().await.unwrap();
        nothing_typed_or_relayed(&root, &daemon);
        assert_eq!(queue.wake_gate_state(id).unwrap(), parked_wake_state, "parked row is not reconsidered each poll");
        assert!(queue.recipient_receipt(id, "supervisor").unwrap().is_none(), "busy row keeps no transport claim");
        assert_eq!(queue.message_delivery_report(id).unwrap().unwrap().pending_reason, Some(cas_store::PendingReason::AwaitingBusyRecipient));
        assert!(!daemon.lifecycle_redelivery_attempts.contains_key(&id), "busy veto spends no cadence");
    }
    // An input-ready transition re-offers the durable park before its timeout.
    daemon.pane_silent_since.insert(name, Instant::now() - Duration::from_secs(600));
    daemon.process_prompt_queue().await.unwrap();
    assert!(queue.queued_prompt(id).unwrap().unwrap().processed_at.is_none(), "enqueue alone cannot finish the row");
    let typed = typed_once(&root, &mut daemon, &body).await;
    assert_eq!(typed.matches(&format!("[cas #{id} ")).count(), 1);
    assert!(typed.starts_with("\x1b[200~"), "Claude receives one explicit paste, including the envelope");
    assert!(typed.ends_with("\x1b[201~\r"));
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_fyi_and_forged_wake_stay_quiet_and_readable() {
    if !fixture_child("cas_3f90_fyi_and_forged_wake_stay_quiet_and_readable") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let body = lifecycle_body(&daemon, cas_types::TaskStatus::InProgress, "task_started", "open", 123, "Started work");
    let started = enqueue(&daemon, "lifecycle:123", &body, cas_store::QueueOrigin::Daemon);
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let forged = queue
        .enqueue_with_session("lifecycle-wake:ci-red-run:fake", "supervisor", BODY, &daemon.session_name)
        .unwrap();
    daemon.process_prompt_queue().await.unwrap();
    nothing_typed_or_relayed(&root, &daemon);
    let rows = queue.surface_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20).unwrap();
    assert!(rows.iter().any(|row| row.id == started));
    assert!(rows.iter().any(|row| row.id == forged));
}

/// A body over the hook budget, or one that fits alone but not with row
/// framing, takes the Teams relay with the body untruncated.
async fn over_budget_event_relays_the_whole_body(padding: String, fits_alone: bool) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = fixture().await;
    let body = BODY.replace("review the lane.", &format!("review the lane.{padding}"));
    assert_eq!(cas_store::fits_hook_surface(&body), fits_alone);
    let id = event(&daemon, &body);
    daemon.process_prompt_queue().await.unwrap();
    let inbox = teams_messages(&daemon);
    assert_eq!(inbox.len(), 1);
    assert!(inbox[0]["text"].as_str().unwrap().contains(&body), "body must not be truncated");
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert!(queue.queued_prompt(id).unwrap().unwrap().processed_at.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_over_hook_budget_event_relays_the_whole_body() {
    if fixture_child("cas_3f90_over_hook_budget_event_relays_the_whole_body") {
        over_budget_event_relays_the_whole_body("detail".repeat(cas_store::HOOK_SURFACE_BUDGET_CHARS), false).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_framing_over_cap_event_relays_the_whole_body() {
    if fixture_child("cas_3f90_framing_over_cap_event_relays_the_whole_body") {
        let framed = cas_store::HOOK_SURFACE_BUDGET_CHARS - cas_store::HOOK_SURFACE_ROW_OVERHEAD_CHARS - BODY.len();
        over_budget_event_relays_the_whole_body("x".repeat(framed), true).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_lifecycle_action_has_one_submit_and_no_duplicate_relay() {
    if !fixture_child("cas_3f90_lifecycle_action_has_one_submit_and_no_duplicate_relay") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let body = lifecycle_body(&daemon, cas_types::TaskStatus::Blocked, "task_blocked", "in_progress", 124, "Need a decision before proceeding.");
    let id = enqueue(&daemon, "lifecycle-wake:124", &body, cas_store::QueueOrigin::Daemon);
    daemon.process_prompt_queue().await.unwrap();
    typed_once(&root, &mut daemon, &body).await;
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert_eq!(queue.message_delivery_report(id).unwrap().unwrap().wake_attempt, cas_store::WakeAttempt::Fired);
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
            .enqueue_with_session("worker", "supervisor", &format!("Work update {n}"), &daemon.session_name)
            .unwrap();
    }
    let action = event(&daemon, BODY);
    daemon.process_prompt_queue().await.unwrap();
    nothing_typed_or_relayed(&root, &daemon);
    daemon.process_prompt_queue().await.unwrap();
    typed_once(&root, &mut daemon, BODY).await;
    assert!(queue.queued_prompt(action).unwrap().unwrap().processed_at.is_some());
    let quiet = queue.surface_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20).unwrap();
    assert_eq!(quiet.len(), 12, "parking spends no wake budget or read receipt");
}

#[tokio::test(flavor = "current_thread")]
async fn cas_3f90_inline_peer_retains_the_shared_inbox_label() {
    if !fixture_child("cas_3f90_inline_peer_retains_the_shared_inbox_label") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let mut peer = cas_types::Agent::new_with_role("peer-id".into(), "far-heron".into(), cas_types::AgentRole::Supervisor);
    peer.factory_session = Some(daemon.session_name.clone());
    crate::store::open_agent_store(daemon.app.cas_dir()).unwrap().register(&peer).unwrap();
    let body = "Please coordinate this delivery with me.";
    enqueue(&daemon, &peer.name, body, cas_store::QueueOrigin::RegisteredAgent { agent_id: peer.id.clone() });
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db")).unwrap().execute(
        "UPDATE prompt_queue SET created_at=?1",[(chrono::Utc::now()-chrono::Duration::seconds(21)).to_rfc3339()]).unwrap();
    daemon.process_prompt_queue().await.unwrap();
    let typed = typed_once(&root, &mut daemon, body).await;
    assert!(typed.contains("[peer supervisor]"), "{typed:?}");
}

/// cas-bf20: on 2026-10-26 a hub-verified phone send to an idle Claude
/// supervisor was parked as a "quiet supervisor event" and never typed; the
/// supervisor ended 67 minutes later without seeing it. The operator's own
/// message must start the idle supervisor's turn.
#[tokio::test(flavor = "current_thread")]
async fn cas_bf20_verified_commander_send_wakes_an_idle_supervisor() {
    if !fixture_child("cas_bf20_verified_commander_send_wakes_an_idle_supervisor") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let body = "Go ahead with the migration";
    let attribution = crate::ui::factory::protocol::MessageAttribution {
        device_id: Some("fc76b6df".into()),
        credential_id: Some("b7cd4206".into()),
        device_label: Some("Android Chrome".into()),
        operator_label: Some("Daniel".into()),
        controller_origin: Some("https://hub.petrastella.io".into()),
        request_id: Some("d6b3978c".into()),
        scopes: vec!["message:send".into()],
        operator_verified: true,
    };
    let supervisor = daemon.app.supervisor_name().to_string();
    daemon.enqueue_attributed_message(&supervisor, body, None, false, None, &attribution).unwrap();
    typed_once(&root, &mut daemon, body).await;
}


#[tokio::test(flavor = "current_thread")]
async fn cas_bc4d_directed_batches_preserve_budget_and_legacy_guards() {
    if !fixture_child("cas_bc4d_directed_batches_preserve_budget_and_legacy_guards") { return; }
    for guard in ["budget", "codex-budget", "held-leader", "held-passenger", "memory-leader", "memory-passenger"] {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let cli = if guard == "codex-budget" { cas_mux::SupervisorCli::Codex } else { cas_mux::SupervisorCli::Claude };
        let budget = guard.ends_with("budget");
        let (root, mut daemon) = fixture_for(cli).await;
        let cas = daemon.app.cas_dir().to_path_buf();
        if cli == cas_mux::SupervisorCli::Codex {
            daemon.queue_jobs = super::prompt_queue_jobs::QueueJobs::start_with_private_writer(cas.clone(), daemon.app.project_scope());
            daemon.queue_jobs.store(&cas).unwrap();
        }
        let peer = cas_types::Agent::new_with_role(
            "guard-peer".into(), "peer".into(), cas_types::AgentRole::Supervisor,
        );
        crate::store::open_agent_store(&cas).unwrap().register(&peer).unwrap();
        let bodies = if budget {
            vec![format!("Budget report A {}", "a".repeat(2600)), format!("Budget report B {}", "b".repeat(2600))]
        } else { vec!["First field report".into(), "Second field report".into()] };
        assert!(bodies.iter().all(|body| cas_store::fits_hook_surface(body)));
        let ids: Vec<_> = bodies.iter().map(|body| enqueue(&daemon, &peer.name, body,
            cas_store::QueueOrigin::RegisteredAgent { agent_id: peer.id.clone() })).collect();
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let held = if guard.ends_with("leader") { 0 } else { 1 };
        if !budget {
            let row = queue.queued_prompt(ids[held]).unwrap().unwrap();
            let legacy = super::delivery::claude_teams_inbox_text(
                &format!("{}\n{}", crate::mcp::tools::service::agent_search_system::message::first_message_provenance(&row), row.prompt),
                &row.prompt, "director", row.id,
            );
            daemon.teams.as_ref().unwrap().write_to_inbox("supervisor", "director", &legacy, None, None).unwrap();
            let pane = daemon.app.supervisor_name().to_string();
            let bytes = daemon.app.mux.pane_bytes_received(&pane).unwrap();
            if guard.starts_with("memory") {
                daemon.inbox_deferred_writes.insert(row.id, super::queue_and_events::InboxDeferredWrite {
                    pane, bytes_at_write: bytes, written_at: Instant::now(), delivered_at: chrono::Utc::now(),
                });
            } else { queue.record_deferred_inbox(row.id, bytes).unwrap(); }
        }
        rusqlite::Connection::open(cas.join("cas.db")).unwrap().execute(
            "UPDATE prompt_queue SET created_at=?1,next_attempt_at=?1",
            [(chrono::Utc::now()-chrono::Duration::seconds(21)).to_rfc3339()],
        ).unwrap();
        daemon.process_prompt_queue().await.unwrap();
        settle_injected_writes(&mut daemon).await;
        let inbox = teams_messages(&daemon);
        if guard == "codex-budget" {
            let typed = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    daemon.process_prompt_queue().await.unwrap();
                    settle_injected_writes(&mut daemon).await;
                    let typed = std::fs::read_to_string(root.path().join("input")).unwrap();
                    if typed.ends_with('\r') && ids.iter().all(|id|
                        queue.message_delivery_report(*id).unwrap().unwrap().stage == cas_store::DeliveryStage::Delivered) { break typed; }
                    tokio::task::yield_now().await;
                }
            }).await.expect("oversized Codex directed batch submitted");
            assert_eq!(typed.matches('\r').count(), 1, "one Codex turn");
            for (id, body) in ids.iter().zip(&bodies) {
                assert_eq!(typed.matches(body).count(), 1, "full body without truncation or duplicate passenger");
                assert!(typed.contains(&format!("[cas #{id} ")));
            }
            assert!(inbox.is_empty(), "Codex must not receive a duplicate Teams copy");
        } else if budget {
            assert_eq!(inbox.len(), 1, "oversized directed burst must use the full-body Teams path");
            let text = inbox[0]["text"].as_str().unwrap();
            for body in &bodies { assert!(text.contains(body), "no truncation or lost passenger"); }
        } else {
            let held_marker = format!("[cas #{} ", ids[held]);
            assert_eq!(inbox.iter().filter(|message| message["text"].as_str().unwrap().contains(&held_marker)).count(), 1,
                "{guard}: the held legacy copy must not be duplicated; headers={:?}",
                inbox.iter().map(|message| message["text"].as_str().unwrap().lines().next()).collect::<Vec<_>>());
            assert!(!std::fs::read_to_string(root.path().join("input")).unwrap().contains(&bodies[held]),
                "{guard}: legacy content must not be typed again as a batch member");
            if guard == "memory-leader" {
                assert_ne!(queue.message_delivery_report(ids[1]).unwrap().unwrap().stage,
                    cas_store::DeliveryStage::Delivered, "a held leader cannot consume its unsent passenger");
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cas_bc4d_staggered_arrivals_share_one_deadline_and_turn() {
    if !fixture_child("cas_bc4d_staggered_arrivals_share_one_deadline_and_turn") { return; }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = fixture().await;
    let cas = daemon.app.cas_dir().to_path_buf();
    let peer = cas_types::Agent::new_with_role(
        "staggered-peer".into(), "peer".into(), cas_types::AgentRole::Supervisor,
    );
    crate::store::open_agent_store(&cas).unwrap().register(&peer).unwrap();
    // Move the durable clock between arrivals without waiting out the debounce.
    let advance = |seconds| {
        let conn = rusqlite::Connection::open(cas.join("cas.db")).unwrap();
        let rows: Vec<(i64, String, String)> = conn.prepare(
            "SELECT id,created_at,next_attempt_at FROM prompt_queue WHERE processed_at IS NULL"
        ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap().collect::<Result<_,_>>().unwrap();
        for (id, created, deadline) in rows {
            let shift = |value: &str| (chrono::DateTime::parse_from_rfc3339(value).unwrap()
                - chrono::Duration::seconds(seconds)).to_rfc3339();
            conn.execute("UPDATE prompt_queue SET created_at=?1,next_attempt_at=?2 WHERE id=?3",
                rusqlite::params![shift(&created),shift(&deadline),id]).unwrap();
        }
    };
    let mut ids = Vec::new();
    let bodies = ["Staggered report one", "Staggered report two", "Staggered report three"];
    for body in bodies {
        if !ids.is_empty() { advance(5); }
        ids.push(enqueue(&daemon, &peer.name, body,
            cas_store::QueueOrigin::RegisteredAgent { agent_id: peer.id.clone() }));
        daemon.process_prompt_queue().await.unwrap();
        nothing_typed_or_relayed(&root, &daemon);
        let conn = rusqlite::Connection::open(cas.join("cas.db")).unwrap();
        let deadlines: Vec<_> = ids.iter().map(|id| {
            let value: String = conn.query_row("SELECT next_attempt_at FROM prompt_queue WHERE id=?1",
                [id], |row| row.get(0)).unwrap();
            chrono::DateTime::parse_from_rfc3339(&value).unwrap()
        }).collect();
        assert!(deadlines.iter().all(|deadline| *deadline == deadlines[0]),
            "arrivals within the window must keep the leader's deadline: {deadlines:?}");
    }
    advance(21);
    daemon.process_prompt_queue().await.unwrap();
    settle_injected_writes(&mut daemon).await;
    let typed = typed_once(&root, &mut daemon, bodies[0]).await;
    let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
    for (id, body) in ids.iter().zip(bodies) {
        assert_eq!(typed.matches(body).count(), 1);
        assert!(typed.contains(&format!("[cas #{id} ")));
        assert_eq!(queue.message_delivery_report(*id).unwrap().unwrap().stage,
            cas_store::DeliveryStage::Delivered);
    }
    daemon.process_prompt_queue().await.unwrap();
    assert_eq!(std::fs::read_to_string(root.path().join("input")).unwrap(), typed,
        "no separate turn for staggered passengers");
}

#[tokio::test(flavor = "current_thread")]
async fn cas_1e7e2_directed_burst_debounces_into_one_body_turn() {
    if !fixture_child("cas_1e7e2_directed_burst_debounces_into_one_body_turn") {
        return;
    }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    // Both harnesses use the same body, claim and completion path.
    for cli in [
        cas_mux::SupervisorCli::Claude,
        cas_mux::SupervisorCli::Codex,
    ] {
        let (root, mut daemon) = fixture_for(cli).await;
        let cas = daemon.app.cas_dir().to_path_buf();
        if cli == cas_mux::SupervisorCli::Codex {
            daemon.queue_jobs = super::prompt_queue_jobs::QueueJobs::start_with_private_writer(cas.clone(), daemon.app.project_scope());
            daemon.queue_jobs.store(&cas).unwrap();
        }
        let peer = cas_types::Agent::new_with_role(
            "burst-peer".into(),
            "peer".into(),
            cas_types::AgentRole::Supervisor,
        );
        crate::store::open_agent_store(&cas)
            .unwrap()
            .register(&peer)
            .unwrap();
        let bodies = [
            "First field report",
            "Second field report",
            "Third field report",
        ];
        let ids: Vec<_> = bodies
            .iter()
            .map(|body| {
                enqueue(
                    &daemon,
                    "peer",
                    body,
                    cas_store::QueueOrigin::RegisteredAgent {
                        agent_id: peer.id.clone(),
                    },
                )
            })
            .collect();
        let queue=crate::store::open_prompt_queue_store(&cas).unwrap();
        tokio::time::timeout(Duration::from_secs(5),async {
            loop {
                daemon.process_prompt_queue().await.unwrap();
                if ids.iter().all(|id|queue.message_delivery_report(*id).unwrap().unwrap().pending_detail.as_deref()==Some("directed supervisor message burst debounce")) { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("fresh burst reaches debounce without typing");
        nothing_typed_or_relayed(&root, &daemon);
        // Advance the durable queue clock, without sleeping for the debounce.
        rusqlite::Connection::open(cas.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at=?1,next_attempt_at=?1",
                [(chrono::Utc::now() - chrono::Duration::seconds(21)).to_rfc3339()],
            )
            .unwrap();
        let typed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                daemon.process_prompt_queue().await.unwrap();
                settle_injected_writes(&mut daemon).await;
                let text = std::fs::read_to_string(root.path().join("input")).unwrap();
                if text.ends_with('\r') && ids.iter().all(|id|
                    queue.message_delivery_report(*id).unwrap().unwrap().stage == cas_store::DeliveryStage::Delivered) {
                    break text;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("one submitted burst body");
        assert_eq!(typed.matches('\r').count(), 1, "one turn: {typed:?}");
        assert!(typed.contains("From: peer"),"coalesced bodies retain sender attribution: {typed:?}");
        for body in bodies {
            assert_eq!(typed.matches(body).count(), 1, "{typed:?}");
        }
        assert!(
            teams_messages(&daemon).is_empty(),
            "no duplicate Teams copy"
        );
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        for id in &ids {
            assert!(
                typed.contains(&format!("[cas #{id} ")),
                "each row retains its identity"
            );
            assert_eq!(
                queue.message_delivery_report(*id).unwrap().unwrap().stage,
                cas_store::DeliveryStage::Delivered
            );
        }
        assert!(
            crate::supervisor_message_wakes::report(&cas, daemon.app.supervisor_name())
                .unwrap()
                .contains("directed_message_wakes_last_hour: 1")
        );
        daemon.process_prompt_queue().await.unwrap();
        settle_injected_writes(&mut daemon).await;
        assert_eq!(
            std::fs::read_to_string(root.path().join("input")).unwrap(),
            typed,
            "no second turn for batch passengers"
        );
    }
}
