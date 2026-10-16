//! Factory loop v2 C4a (cas-786a6, spec §4.3 and §7): an urgent inject is a
//! command on the loop. Its 1.2 s settle is a per-pane state, so no control
//! pass waits for it.
use std::time::{Duration, Instant};

use crate::store::{open_agent_store, open_prompt_queue_store};
use crate::ui::factory::daemon::FactoryDaemon;

const WORKER: &str = "worker-a";
const SESSION: &str = "provision-test";

/// A Claude worker pane running `cat`, past its injection grace period.
pub(super) async fn ready_claude_worker(daemon: &mut FactoryDaemon) {
    let pty = cas_mux::Pty::spawn(
        WORKER,
        cas_mux::PtyConfig {
            command: "sh".into(),
            args: vec!["-c".into(), "printf ready; exec cat".into()],
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .unwrap();
    let pane = cas_mux::Pane::with_pty(
        WORKER,
        cas_mux::PaneKind::Worker,
        pty,
        24,
        80,
        cas_mux::SupervisorCli::Claude,
    )
    .unwrap();
    #[allow(deprecated)]
    daemon.app.mux.set_worker_cli(cas_mux::SupervisorCli::Claude);
    daemon.app.adopt_worker_pane_for_test(pane);
    tokio::time::sleep(Duration::from_millis(5100)).await;
    drain(daemon);
    assert!(daemon.app.mux.pane_ready_for_injection(WORKER));
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
    let root = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
    let mut worker = cas_types::Agent::new_with_role(
        "worker-id".into(),
        WORKER.into(),
        cas_types::AgentRole::Worker,
    );
    worker.factory_session = Some(SESSION.into());
    open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
    (root, cas_dir)
}

pub(super) fn enqueue_urgent(cas_dir: &std::path::Path, text: &str) -> i64 {
    open_prompt_queue_store(cas_dir)
        .unwrap()
        .enqueue_urgent_with_outcome(
            "supervisor",
            WORKER,
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
        if echoed.contains("C4A-MARKER") {
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

    let lane = lane_guard::enter_lane(Lane::Control);
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
        if echoed.contains("C4A-LANES") {
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
