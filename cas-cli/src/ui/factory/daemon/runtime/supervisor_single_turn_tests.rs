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
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
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
    assert!(
        !report
            .recipient_receipt
            .is_some_and(SurfacingSource::is_surfaced),
        "typing is not proof of rendering"
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
    let row = queue.queued_prompt(id).unwrap().unwrap();
    assert!(
        row.processed_at.is_some(),
        "retire the already-rendered event: {row:?}"
    );
    assert_eq!(
        queue.recipient_receipt(id, "supervisor").unwrap(),
        Some(SurfacingSource::HookSurfaced)
    );
    assert_ne!(row.highest_stage, Some(cas_store::DeliveryStage::Abandoned));
    assert!(teams_messages(&daemon).is_empty());
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
}
