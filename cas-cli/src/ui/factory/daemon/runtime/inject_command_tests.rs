//! Factory loop v2 C4a (cas-786a6, spec §4.3 and §7): an urgent inject is a
//! command on the loop. Its 1.2 s settle is a per-pane state, so no control
//! pass waits for it.
use std::time::{Duration, Instant};

use crate::store::{open_agent_store, open_prompt_queue_store};
use crate::ui::factory::daemon::FactoryDaemon;

const WORKER: &str = "worker-a";
const SESSION: &str = "provision-test";

/// A Claude worker pane running `cat`, past its injection grace period
/// (backdated, not slept: review L4).
pub(super) async fn ready_claude_worker(daemon: &mut FactoryDaemon) {
    ready_claude_worker_named(daemon, WORKER).await;
}

pub(super) async fn ready_claude_worker_named(daemon: &mut FactoryDaemon, name: &str) {
    ready_claude_worker_running(daemon, name, "printf ready; exec cat").await;
}

/// A ready Claude worker pane running `script` (it must print something).
pub(super) async fn ready_claude_worker_running(daemon: &mut FactoryDaemon, name: &str, script: &str) {
    let pty = cas_mux::Pty::spawn(
        name,
        cas_mux::PtyConfig {
            command: "sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .unwrap();
    let mut pane = cas_mux::Pane::with_pty(
        name,
        cas_mux::PaneKind::Worker,
        pty,
        24,
        80,
        cas_mux::SupervisorCli::Claude,
    )
    .unwrap();
    pane.backdate_spawn_for_test(Duration::from_secs(6));
    #[allow(deprecated)]
    daemon.app.mux.set_worker_cli(cas_mux::SupervisorCli::Claude);
    daemon.app.adopt_worker_pane_for_test(pane);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !daemon.app.mux.pane_ready_for_injection(name) && Instant::now() < deadline {
        drain(daemon);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(daemon.app.mux.pane_ready_for_injection(name));
}

/// What the loop's PTY exchange does each pass: drain pane output.
pub(super) fn drain(daemon: &mut FactoryDaemon) -> String {
    let mut seen = String::new();
    for pane in daemon.app.mux.panes_mut() {
        let (bytes, _) = pane.drain_output();
        seen.push_str(&String::from_utf8_lossy(&bytes));
    }
    seen
}

pub(super) fn project_with_worker() -> (tempfile::TempDir, std::path::PathBuf) {
    project_with_workers(&[WORKER])
}

pub(super) fn project_with_workers(names: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
    for name in names {
        let mut worker = cas_types::Agent::new_with_role(
            format!("{name}-id"),
            (*name).into(),
            cas_types::AgentRole::Worker,
        );
        worker.factory_session = Some(SESSION.into());
        open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
    }
    (root, cas_dir)
}

pub(super) fn enqueue_urgent(cas_dir: &std::path::Path, text: &str) -> i64 {
    enqueue_urgent_to(cas_dir, WORKER, text)
}

pub(super) fn enqueue_urgent_to(cas_dir: &std::path::Path, target: &str, text: &str) -> i64 {
    open_prompt_queue_store(cas_dir)
        .unwrap()
        .enqueue_urgent_with_outcome(
            "supervisor",
            target,
            text,
            Some(SESSION),
            Some(text),
            Some(cas_store::NotificationPriority::High),
            true,
            None,
        )
        .unwrap()
        .id()
}

/// spec §7 `flv2_inject_is_a_command`: an urgent inject with a 1.2 s settle
/// leaves every control pass under 100 ms, and the redirect still reaches
/// the pane after the settle.
#[tokio::test]
async fn flv2_inject_is_a_command() {
    let (_root, cas_dir) = project_with_worker();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    ready_claude_worker(&mut daemon).await;
    assert_eq!(daemon.urgent_settle_duration(WORKER), Duration::from_millis(1200));
    let id = enqueue_urgent(&cas_dir, "urgent redirect C4A-MARKER");

    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut echoed = String::new();
    while started.elapsed() < Duration::from_secs(6) {
        let pass = Instant::now();
        daemon.process_prompt_queue().await.unwrap();
        slowest = slowest.max(pass.elapsed());
        echoed.push_str(&drain(&mut daemon));
        // The payload's write receipt, and so the row's bookkeeping, may
        // land a pass after its echo.
        if echoed.contains("C4A-MARKER") && !daemon.app.mux.has_pending_injects() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        slowest < Duration::from_millis(100),
        "a control pass waited {slowest:?} for an urgent inject's settle"
    );
    assert!(echoed.contains("C4A-MARKER"), "the redirect reached the pane: {echoed:?}");
    assert!(
        started.elapsed() >= Duration::from_millis(1200),
        "the payload still waits out the settle"
    );
    // The outcome came back as an event on a later pass and was recorded as
    // before C4a: the wake fired, and the row waits for (or already has)
    // evidence the pane took the turn rather than being consumed on the
    // keystrokes alone.
    let report = open_prompt_queue_store(&cas_dir)
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert_eq!(report.wake_attempt, cas_store::WakeAttempt::Fired, "{report:?}");
    assert!(
        daemon.urgent_wake_probes.contains_key(&id)
            || open_prompt_queue_store(&cas_dir)
                .unwrap()
                .queued_prompt(id)
                .unwrap()
                .unwrap()
                .processed_at
                .is_some(),
        "the urgent row is held for its wake probe or consumed on an observed wake"
    );
}

/// cas-786a6 (H3 from cas-4272): on the daemon's job lanes the fresh claim is
/// a two-phase `ClaimForInject` (the claim job first, the inject on a later
/// pass), the row's decision reads come with its `QueueSelect` snapshot, and
/// no pass from selection to inject outcome makes a blocking call on the
/// control lane.
#[tokio::test]
async fn flv2_inject_path_claims_in_two_phases_with_no_lane_violation() {
    use cas_store::lane_guard::{self, Lane};

    let (_root, cas_dir) = project_with_worker();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.queue_jobs =
        super::prompt_queue_jobs::QueueJobs::start(cas_dir.clone(), daemon.app.project_scope());
    // Opened once before the loop becomes a lane, as `run` does.
    daemon.queue_jobs.store(&cas_dir).unwrap();
    ready_claude_worker(&mut daemon).await;
    let id = enqueue_urgent(&cas_dir, "urgent redirect C4A-LANES");

    // C6 (cas-9db0b, review M2): the lane is held as `run` holds it for the
    // inject and queue pass: the loop's report-mode attachment with its
    // `enforce_here` scope, enforced violations set to panic.
    let progress = super::loop_watchdog::LoopProgress::new();
    let lane = super::loop_watchdog::enforced_daemon_step(
        &progress,
        super::loop_watchdog::LoopPhase::PromptQueue,
    );
    assert_eq!(lane_guard::current_lane(), Some(Lane::Control));
    assert_eq!(lane_guard::current_mode(), Some(lane_guard::GuardMode::Enforce));
    let before = lane_guard::thread_violations();
    let started = Instant::now();
    let mut pass = 0usize;
    let mut claim_pass = None;
    let mut inject_pass = None;
    let mut slowest = Duration::ZERO;
    let mut echoed = String::new();
    while started.elapsed() < Duration::from_secs(8) {
        pass += 1;
        let at = Instant::now();
        // The pass as `run` drives it inside the enforced scope.
        daemon.app.mux.flush_deferred_injections().await;
        daemon.advance_urgent_injects().await;
        daemon.process_prompt_queue().await.unwrap();
        slowest = slowest.max(at.elapsed());
        if claim_pass.is_none() && daemon.queue_jobs.has_claim(id, WORKER) {
            claim_pass = Some(pass);
            assert!(
                !daemon.app.mux.inject_pending(WORKER),
                "the pass that submits the claim does not inject"
            );
        }
        if inject_pass.is_none() && daemon.app.mux.inject_pending(WORKER) {
            inject_pass = Some(pass);
        }
        echoed.push_str(&drain(&mut daemon));
        if echoed.contains("C4A-LANES") && !daemon.app.mux.has_pending_injects() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let violations = lane_guard::thread_violations() - before;
    let last = lane_guard::last_thread_violation();
    drop(lane);

    assert!(echoed.contains("C4A-LANES"), "the redirect reached the pane: {echoed:?}");
    let (claim_pass, inject_pass) = (claim_pass.expect("a claim job ran"), inject_pass.expect("the inject began"));
    assert!(
        inject_pass > claim_pass,
        "the inject (pass {inject_pass}) follows the claim job (pass {claim_pass}) on a later pass"
    );
    assert_eq!(violations, 0, "blocking calls on the control lane; last: {last:?}");
    assert!(slowest < Duration::from_millis(100), "slowest pass {slowest:?}");
    assert!(
        daemon.queue_jobs.drain_for_shutdown(&cas_dir, Duration::from_secs(5)),
        "every stamp landed"
    );
    let report = open_prompt_queue_store(&cas_dir)
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert_eq!(report.wake_attempt, cas_store::WakeAttempt::Fired, "{report:?}");
}

/// Run passes, draining pane output, until `done` holds over the output or
/// `limit` passes.
async fn passes_until(
    daemon: &mut FactoryDaemon,
    limit: Duration,
    mut done: impl FnMut(&str) -> bool,
) -> String {
    let started = Instant::now();
    let mut echoed = String::new();
    while started.elapsed() < limit {
        daemon.process_prompt_queue().await.unwrap();
        echoed.push_str(&drain(daemon));
        if done(&echoed) && !daemon.app.mux.has_pending_injects() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    echoed
}

/// cas-786a6 review L4: an urgent all-workers broadcast is a command per
/// member; both members get the redirect after the settle and the outcome
/// counts both.
#[tokio::test]
async fn urgent_broadcast_members_settle_as_commands_and_count_once() {
    let (root, cas_dir) = project_with_workers(&[WORKER, "worker-b"]);
    // A broadcast is delivered per member; without Agent Teams the daemon
    // gates `all_workers` like a pane (pre-existing), so run with a team tree.
    let claude_config = root.path().join("claude-config");
    std::fs::create_dir_all(claude_config.join("teams").join(SESSION).join("inboxes")).unwrap();
    let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[(
        "CLAUDE_CONFIG_DIR",
        claude_config.to_str(),
    )]);
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.teams = Some(super::teams::TeamsManager::new(SESSION));
    ready_claude_worker(&mut daemon).await;
    ready_claude_worker_named(&mut daemon, "worker-b").await;
    let id = enqueue_urgent_to(&cas_dir, "all_workers", "everyone C4A-BCAST");

    let echoed = passes_until(&mut daemon, Duration::from_secs(6), |echoed| {
        echoed.matches("C4A-BCAST").count() >= 2
    })
    .await;
    assert_eq!(echoed.matches("C4A-BCAST").count(), 2, "both members typed: {echoed:?}");
    // The outcome is stamped once the last member reports.
    for _ in 0..20 {
        daemon.process_prompt_queue().await.unwrap();
    }
    let report = open_prompt_queue_store(&cas_dir)
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (report.broadcast_attempted, report.broadcast_succeeded, report.broadcast_failed),
        (Some(2), Some(2), Some(0)),
        "the broadcast outcome counts both members once: {report:?}"
    );
    assert!(!daemon.urgent_injects.broadcasts.contains_key(&id));
}

/// cas-786a6 review M2/L4: a daemon that died between the claim and the
/// inject leaves a bare claim. The next daemon releases it at start and
/// delivers the row.
#[tokio::test]
async fn a_restart_between_claim_and_inject_releases_the_claim() {
    let (_root, cas_dir) = project_with_worker();
    let id = enqueue_urgent(&cas_dir, "claimed then crashed C4A-RESTART");
    {
        let mut first = super::provisioning_tests::daemon(&cas_dir);
        assert!(matches!(
            first.queue_jobs.claim_for_inject(&cas_dir, SESSION, id, WORKER, WORKER),
            super::prompt_queue_jobs::ClaimStep::Claimed(_)
        ));
        // The daemon dies here: no inject, no stamp.
    }
    let _ = std::fs::remove_file(cas_dir.join("test.sock"));
    let _ = std::fs::remove_file(cas_dir.join("gui.sock"));
    let mut second = super::provisioning_tests::daemon(&cas_dir);
    second.release_orphaned_claims_at_start();
    ready_claude_worker(&mut second).await;
    let echoed = passes_until(&mut second, Duration::from_secs(6), |echoed| {
        echoed.contains("C4A-RESTART")
    })
    .await;
    assert!(echoed.contains("C4A-RESTART"), "the row is delivered after the restart: {echoed:?}");
}

/// cas-786a6 review M4: an operator urgent inject that arrives while another
/// settles on the same pane waits for it, and both are typed in order.
#[tokio::test]
async fn an_operator_urgent_inject_queues_behind_a_settling_one() {
    let (_root, cas_dir) = project_with_worker();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    ready_claude_worker(&mut daemon).await;
    let settle = Duration::from_millis(100);
    daemon.begin_operator_urgent_inject(WORKER, "first C4A-OP1", settle).await;
    daemon.begin_operator_urgent_inject(WORKER, "second C4A-OP2", settle).await;
    assert_eq!(daemon.urgent_injects.operator_backlog.len(), 1, "the second waits");

    let started = Instant::now();
    let mut echoed = String::new();
    while started.elapsed() < Duration::from_secs(5) && !echoed.contains("C4A-OP2") {
        daemon.advance_urgent_injects().await;
        echoed.push_str(&drain(&mut daemon));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (first, second) = (echoed.find("C4A-OP1"), echoed.find("C4A-OP2"));
    assert!(first.is_some() && second.is_some(), "both typed: {echoed:?}");
    assert!(first < second, "in order");
}

/// cas-786a6 review M5 / round 2 L1: on a daemon lane the provenance envelope
/// is written off the lane, and the pane's next submit CR is held until it
/// is, so the hook that reads it at submit always finds it.
#[test]
fn provenance_is_written_off_the_lane_before_the_submit() {
    use cas_store::lane_guard::{self, Lane};
    let root = tempfile::tempdir().unwrap();
    let payload = {
        let _lane = super::loop_watchdog::enforce_test_lane(Lane::Control);
        super::delivery::prepare_pty_machine_delivery(
            root.path(),
            "gated-worker",
            cas_mux::SupervisorCli::Claude,
            "supervisor",
            "redirect C4A-PROVENANCE",
            Some(4242),
        )
    };
    let gate = cas_mux::submit_gate::pending_submit_gate("gated-worker")
        .expect("the next submit on this pane is held for the envelope");
    assert!(gate.wait(Duration::from_secs(5)), "the envelope write opens the gate");
    let provenance =
        crate::hooks::delivery_provenance::consume(root.path(), "gated-worker", &payload)
            .expect("the envelope is written before the submit is let go");
    assert_eq!(provenance.notification_id, 4242);
}

/// cas-786a6 review M2 / round 2 L2: an urgent row typed by a daemon that
/// died before the pane's reaction was observed holds a bare claim. The next
/// daemon releases it and interrupts again: at-least-once delivery (spec §4.5
/// rule 5), documented in delivery.md.
#[tokio::test]
async fn a_restart_after_an_urgent_inject_reinterrupts_once() {
    // A pane that never reacts: no echo, so the wake probe stays open and
    // the row stays pending under its bare claim.
    const SILENT: &str = "stty -echo; printf ready; exec sleep 60";
    let (_root, cas_dir) = project_with_worker();
    let id = enqueue_urgent(&cas_dir, "urgent before crash C4A-AGAIN");
    {
        let mut first = super::provisioning_tests::daemon(&cas_dir);
        ready_claude_worker_running(&mut first, WORKER, SILENT).await;
        let started = Instant::now();
        while !first.urgent_wake_probes.contains_key(&id) && started.elapsed() < Duration::from_secs(6) {
            first.process_prompt_queue().await.unwrap();
            drain(&mut first);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(first.urgent_wake_probes.contains_key(&id), "the first daemon interrupted");
        // The daemon dies here: its in-memory probe and cadence are lost.
    }
    let row = open_prompt_queue_store(&cas_dir).unwrap().queued_prompt(id).unwrap().unwrap();
    assert!(row.processed_at.is_none(), "the row waits for its wake probe");
    let _ = std::fs::remove_file(cas_dir.join("test.sock"));
    let _ = std::fs::remove_file(cas_dir.join("gui.sock"));
    let mut second = super::provisioning_tests::daemon(&cas_dir);
    second.release_orphaned_claims_at_start();
    ready_claude_worker_running(&mut second, WORKER, SILENT).await;
    let started = Instant::now();
    while !second.urgent_wake_probes.contains_key(&id) && started.elapsed() < Duration::from_secs(6) {
        second.process_prompt_queue().await.unwrap();
        drain(&mut second);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        second.urgent_wake_probes.contains_key(&id),
        "the restarted daemon interrupts once more (at-least-once, spec §4.5 rule 5)"
    );
}

/// cas-786a6 review M2 / round 2 L3: a broadcast member whose row is held by
/// a bare transport claim (never handed off) counts as failed, not
/// succeeded.
#[tokio::test]
async fn a_bare_claimed_broadcast_member_counts_as_failed() {
    let (root, cas_dir) = project_with_workers(&[WORKER, "worker-b"]);
    let claude_config = root.path().join("claude-config");
    std::fs::create_dir_all(claude_config.join("teams").join(SESSION).join("inboxes")).unwrap();
    let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[(
        "CLAUDE_CONFIG_DIR",
        claude_config.to_str(),
    )]);
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.teams = Some(super::teams::TeamsManager::new(SESSION));
    ready_claude_worker(&mut daemon).await;
    ready_claude_worker_named(&mut daemon, "worker-b").await;
    let id = enqueue_urgent_to(&cas_dir, "all_workers", "everyone C4A-BARE");
    open_prompt_queue_store(&cas_dir)
        .unwrap()
        .claim_recipient_transport_token(id, "worker-b")
        .unwrap()
        .expect("a bare claim nobody handed off");

    let echoed = passes_until(&mut daemon, Duration::from_secs(6), |echoed| {
        echoed.contains("C4A-BARE")
    })
    .await;
    assert_eq!(echoed.matches("C4A-BARE").count(), 1, "only worker-a typed: {echoed:?}");
    for _ in 0..20 {
        daemon.process_prompt_queue().await.unwrap();
    }
    let report = open_prompt_queue_store(&cas_dir)
        .unwrap()
        .message_delivery_report(id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (report.broadcast_attempted, report.broadcast_succeeded, report.broadcast_failed),
        (Some(2), Some(1), Some(1)),
        "the bare claim is a failure, not a delivery: {report:?}"
    );
}
