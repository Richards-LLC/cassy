//! cas-00d5 (GH #1190 Codex, GH #1195 Claude): `shutdown_workers` marked its
//! queue row processed, then left idle workers running. The daemon applies a
//! shutdown inside a loop pass whose store-wait budget (GH #1165) is often
//! already spent. Retiring the worker's Cassy identity then failed fast with
//! "this thread's store wait budget is spent", and the shutdown bailed before
//! it signalled the pane. These tests drive the real queue row, the real
//! pending-spawn consumer and real PTY processes under that condition.
use super::super::imports::*;
use super::provisioning_tests::daemon;
use cas_store::{SpawnLifecycleState, SpawnQueueStore};

const SESSION: &str = "provision-test";

fn idle_worker_pane(name: &str, harness: cas_mux::SupervisorCli) -> cas_mux::Pane {
    let pty = cas_mux::Pty::spawn(
        name,
        cas_mux::PtyConfig {
            command: "sleep".to_string(),
            args: vec!["300".to_string()],
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .expect("spawn an idle worker stand-in");
    cas_mux::Pane::with_pty(name, cas_mux::PaneKind::Worker, pty, 24, 80, harness).unwrap()
}

/// Alive means the process exists and is not a zombie awaiting reaping.
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
        })
        .is_some_and(|state| state != "Z" && state != "X")
}

fn register_idle_worker(cas_dir: &std::path::Path, name: &str) {
    let mut agent = cas_types::Agent::new(format!("agent-{name}"), name.to_string());
    agent.role = cas_types::AgentRole::Worker;
    agent.agent_type = cas_types::AgentType::Worker;
    open_agent_store(cas_dir).unwrap().register(&agent).unwrap();
}

/// Hold a foreign SQLite write lock for `hold`, the contention a busy factory
/// has. Returns once the lock is held.
fn hold_write_lock(cas_dir: &std::path::Path, hold: Duration) -> std::thread::JoinHandle<()> {
    let db = cas_dir.join("cas.db");
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let blocker = rusqlite::Connection::open(db).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        held_tx.send(()).unwrap();
        std::thread::sleep(hold);
        blocker.execute_batch("COMMIT").unwrap();
    });
    held_rx.recv().unwrap();
    handle
}

struct Fixture {
    _dir: tempfile::TempDir,
    daemon: FactoryDaemon,
    request_id: i64,
    pids: Vec<(String, u32)>,
}

/// Two idle, clean workers (one Claude, one Codex) and a shutdown row the
/// daemon has already dequeued: `processed_at` is set, nothing has run yet.
fn processed_but_unexecuted_shutdown() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon(&cas_dir);
    let mut pids = Vec::new();
    let mut names = Vec::new();
    for (name, harness) in [
        ("idle-claude", cas_mux::SupervisorCli::Claude),
        ("idle-codex", cas_mux::SupervisorCli::Codex),
    ] {
        register_idle_worker(&cas_dir, name);
        let pane = idle_worker_pane(name, harness);
        let pid = pane
            .process_group_id()
            .expect("the PTY child leads its group");
        daemon.app.adopt_worker_pane_for_test(pane);
        assert!(alive(pid));
        pids.push((name.to_string(), pid));
        names.push(name.to_string());
    }
    let queue = open_spawn_queue_store(&cas_dir).unwrap();
    let request_id = queue
        .enqueue_shutdown(None, &names, false, Some(SESSION))
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();
    assert!(
        queue
            .peek(10)
            .unwrap()
            .iter()
            .all(|row| row.id != request_id),
        "the row is processed before the shutdown runs"
    );
    Fixture {
        _dir: dir,
        daemon,
        request_id,
        pids,
    }
}

fn shutdown_row_state(cas_dir: &std::path::Path, id: i64) -> (Option<String>, Option<String>) {
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    conn.query_row(
        "SELECT spawn_state, spawn_detail FROM spawn_queue WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    done()
}

/// The incident: idle, clean Claude and Codex workers, a processed shutdown
/// row, a pass whose store budget is spent, and brief write contention.
#[tokio::test(flavor = "current_thread")]
async fn processed_shutdown_stops_idle_claude_and_codex_workers_under_a_spent_budget_cas_00d5() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let Fixture {
        _dir,
        mut daemon,
        request_id,
        pids,
    } = processed_but_unexecuted_shutdown();
    let cas_dir = daemon.app.cas_dir().to_path_buf();

    let lock = hold_write_lock(&cas_dir, Duration::from_millis(300));
    {
        let _spent = cas_store::wait_budget::bound_waits_until(Instant::now());
        daemon.process_pending_spawns().await;
    }
    lock.join().unwrap();

    for (name, pid) in &pids {
        assert!(
            wait_until(Duration::from_secs(5), || !alive(*pid)),
            "{name} (pid {pid}) must stop"
        );
        assert!(
            !daemon.app.worker_names().contains(name),
            "{name} is no longer tracked"
        );
        let agent = open_agent_store(&cas_dir)
            .unwrap()
            .get(&format!("agent-{name}"))
            .unwrap();
        assert_eq!(
            agent.status,
            cas_types::AgentStatus::Shutdown,
            "{name} unregistered"
        );
    }
    let (state, detail) = shutdown_row_state(&cas_dir, request_id);
    assert_eq!(
        state.as_deref(),
        Some(SpawnLifecycleState::Stopped.as_str()),
        "the processed row reaches a terminal state: {detail:?}"
    );
}

/// Contention that outlasts even the shutdown's own budget must not leave
/// the worker running: the process stops now, its retirement lands once the
/// store frees up, and the row still reaches a terminal state.
#[tokio::test(flavor = "current_thread")]
async fn shutdown_stops_the_worker_even_when_retirement_must_wait_cas_00d5() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let Fixture {
        _dir,
        mut daemon,
        request_id,
        pids,
    } = processed_but_unexecuted_shutdown();
    let cas_dir = daemon.app.cas_dir().to_path_buf();

    let hold = super::store_worker::SHUTDOWN_STORE_WAIT_BUDGET * 2 + Duration::from_millis(500);
    let lock = hold_write_lock(&cas_dir, hold);
    {
        let _spent = cas_store::wait_budget::bound_waits_until(Instant::now());
        daemon.process_pending_spawns().await;
    }
    for (name, pid) in &pids {
        assert!(
            wait_until(Duration::from_secs(5), || !alive(*pid)),
            "{name} (pid {pid}) must stop while the store is still locked"
        );
    }
    lock.join().unwrap();

    let agents = open_agent_store(&cas_dir).unwrap();
    for (name, _) in &pids {
        assert!(
            wait_until(Duration::from_secs(15), || {
                agents
                    .get(&format!("agent-{name}"))
                    .is_ok_and(|agent| agent.status == cas_types::AgentStatus::Shutdown)
            }),
            "{name}'s deferred retirement lands once the lock clears"
        );
    }
    assert!(
        wait_until(Duration::from_secs(15), || shutdown_row_state(
            &cas_dir, request_id
        )
        .0
        .is_some_and(
            |state| SpawnLifecycleState::from_str(&state).is_some_and(|state| state.is_terminal())
        )),
        "the row reaches a terminal state: {:?}",
        shutdown_row_state(&cas_dir, request_id)
    );
}

/// cas-981e (GH #1185): a `clear_context` recycle runs the same shutdown, so
/// under a spent pass budget it used to abort before stopping the worker and
/// reported nothing: the worker kept its old context and the supervisor never
/// heard back. The old process must stop, and an outcome must reach both the
/// queue row and the supervisor. The recipe here is refused by routing policy
/// after the old worker stops, so the test exercises the failure report
/// without launching a real harness.
#[tokio::test(flavor = "current_thread")]
async fn recycle_under_a_spent_budget_stops_the_old_worker_and_reports_its_outcome_cas_981e() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon(&cas_dir);
    let name = "idle-codex";
    register_idle_worker(&cas_dir, name);
    let pane = idle_worker_pane(name, cas_mux::SupervisorCli::Codex);
    let pid = pane
        .process_group_id()
        .expect("the PTY child leads its group");
    daemon.app.adopt_worker_pane_for_test(pane);

    let refused = cas_mux::WorkerSpec {
        cli: cas_mux::SupervisorCli::Claude,
        model: Some("claude-haiku-4-5".to_string()),
        ..cas_mux::WorkerSpec::builtin_default()
    };
    let queue = open_spawn_queue_store(&cas_dir).unwrap();
    let request_id = queue
        .enqueue_recycle(
            name,
            Some(&serde_json::to_string(&refused).unwrap()),
            Some(SESSION),
        )
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();

    let lock = hold_write_lock(&cas_dir, Duration::from_millis(300));
    {
        let _spent = cas_store::wait_budget::bound_waits_until(Instant::now());
        daemon.process_pending_spawns().await;
    }
    lock.join().unwrap();

    assert!(
        wait_until(Duration::from_secs(5), || !alive(pid)),
        "the recycle's shutdown half must stop the old worker"
    );
    assert!(
        wait_until(Duration::from_secs(15), || shutdown_row_state(
            &cas_dir, request_id
        )
        .0
        .as_deref()
            == Some(SpawnLifecycleState::Failed.as_str())),
        "the recycle row names its failure: {:?}",
        shutdown_row_state(&cas_dir, request_id)
    );
    let prompts = open_prompt_queue_store(&cas_dir).unwrap();
    assert!(
        wait_until(Duration::from_secs(15), || prompts
            .peek_all(50)
            .unwrap()
            .iter()
            .any(|row| row.target == daemon.app.supervisor_name()
                && row.prompt.contains(&format!("request {request_id}"))
                && row.prompt.contains(name))),
        "the supervisor is told the recycle failed"
    );
}

fn register_supervisor(cas_dir: &std::path::Path) -> String {
    let mut agent = cas_types::Agent::new("agent-test-supervisor".into(), "test-supervisor".into());
    agent.role = cas_types::AgentRole::Supervisor;
    open_agent_store(cas_dir).unwrap().register(&agent).unwrap();
    agent.id
}

/// cas-b06f (spec-6194 P2.6, GH #197): a shutdown the supervisor asked for is
/// not N deaths. Two workers stopped on request yield one receipt naming each
/// worker's outcome and the task it released, and no Critical `worker_died`
/// relay. The held task is still released, as GH #197 requires.
#[tokio::test(flavor = "current_thread")]
async fn ct2_p2_requested_shutdown_one_receipt_cas_b06f() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let Fixture {
        _dir,
        mut daemon,
        request_id,
        pids,
    } = processed_but_unexecuted_shutdown();
    let cas_dir = daemon.app.cas_dir().to_path_buf();
    let supervisor_id = register_supervisor(&cas_dir);
    let tasks = crate::store::open_task_store_cached(&cas_dir).unwrap();
    let mut held = cas_types::Task::new("cas-held1".into(), "held by idle-claude".into());
    held.assignee = Some("idle-claude".into());
    held.status = cas_types::TaskStatus::InProgress;
    tasks.add(&held).unwrap();

    daemon.process_pending_spawns().await;

    for (name, pid) in &pids {
        assert!(
            wait_until(Duration::from_secs(5), || !alive(*pid)),
            "{name} stops"
        );
    }
    assert_ne!(
        tasks.get("cas-held1").unwrap().assignee.as_deref(),
        Some("idle-claude"),
        "GH #197: the stopped worker's task is released"
    );
    let notices = crate::store::open_supervisor_queue_store(&cas_dir)
        .unwrap()
        .peek(&supervisor_id, 50)
        .unwrap();
    assert!(
        notices
            .iter()
            .all(|notice| notice.event_type != "worker_died"),
        "a requested shutdown raises no death alarm: {:?}",
        notices
            .iter()
            .map(|notice| &notice.event_type)
            .collect::<Vec<_>>()
    );
    let prompts = open_prompt_queue_store(&cas_dir).unwrap();
    assert!(wait_until(Duration::from_secs(10), || !receipts(
        &prompts, request_id
    )
    .is_empty()));
    let rows = prompts.peek_all(100).unwrap();
    assert!(
        rows.iter().all(|row| !row.source.contains("worker-died")),
        "no worker_died prompt relay: {rows:#?}"
    );
    let receipts = receipts(&prompts, request_id);
    assert_eq!(receipts.len(), 1, "one receipt: {rows:#?}");
    let receipt = &receipts[0];
    for (name, _) in &pids {
        assert!(
            receipt.contains(name.as_str()),
            "{name} is listed: {receipt}"
        );
    }
    assert!(
        receipt.contains("cas-held1"),
        "the released task is listed: {receipt}"
    );
}

fn receipts(
    prompts: &std::sync::Arc<dyn cas_store::PromptQueueStore>,
    request_id: i64,
) -> Vec<String> {
    prompts
        .peek_all(100)
        .unwrap()
        .into_iter()
        .filter(|row| {
            row.prompt
                .contains(&format!("shutdown request {request_id}"))
        })
        .map(|row| row.prompt)
        .collect()
}

/// cas-c1a2 (GH #1178): a recycle relaunches the SAME pane name, and the
/// daemon's pane buffers are keyed by name and only ever appended to
/// (relay.rs `buffer_pane_output`). The replacement's register-timeout
/// diagnostic therefore quoted the previous session's transcript ("the pane
/// still showed the old session's transcript"), so the operator could not see
/// what the relaunched Codex actually printed. Stopping the old worker must
/// drop its buffer, so whatever the diagnostic quotes is the new process.
#[tokio::test(flavor = "current_thread")]
async fn recycle_drops_the_old_sessions_pane_buffer_cas_c1a2() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon(&cas_dir);
    let name = "idle-codex";
    register_idle_worker(&cas_dir, name);
    daemon
        .app
        .adopt_worker_pane_for_test(idle_worker_pane(name, cas_mux::SupervisorCli::Codex));
    daemon.buffer_pane_output(name, b"OLD SESSION TRANSCRIPT: every prior delivery\r\n");

    // A recipe routing refuses after the old worker stops, so no real
    // harness launches; the shutdown half is what this test pins.
    let refused = cas_mux::WorkerSpec {
        cli: cas_mux::SupervisorCli::Claude,
        model: Some("claude-haiku-4-5".to_string()),
        ..cas_mux::WorkerSpec::builtin_default()
    };
    open_spawn_queue_store(&cas_dir)
        .unwrap()
        .enqueue_recycle(
            name,
            Some(&serde_json::to_string(&refused).unwrap()),
            Some(SESSION),
        )
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();
    daemon.process_pending_spawns().await;

    let tail = daemon
        .pane_buffers
        .get(name)
        .map(|buffer| buffer.as_plain_text())
        .unwrap_or_default();
    assert!(
        !tail.contains("OLD SESSION TRANSCRIPT"),
        "the stopped session's output must not be quoted for its replacement: {tail}"
    );
}

/// Exercise a real same-name relaunch with more old output than the replay
/// buffer can hold. The fake Codex emits one short startup line and stays
/// alive without registering, so the real timeout path has to diagnose it.
#[tokio::test(flavor = "current_thread")]
async fn long_transcript_recycle_timeout_quotes_only_replacement_cas_c1a2() {
    long_transcript_recycle(false).await;
}

/// The old output must also be gone when the replacement registers normally.
/// This is the successful control for the deliberately unregistered child.
#[tokio::test(flavor = "current_thread")]
async fn long_transcript_recycle_registers_fresh_session_cas_c1a2() {
    long_transcript_recycle(true).await;
}

async fn long_transcript_recycle(register_replacement: bool) {
    use std::os::unix::fs::PermissionsExt;

    const NAME: &str = "long-history-codex";
    const OLD: &str = "OLD SESSION TRANSCRIPT: prior delivery\r\n";
    const NEW: &str = "REPLACEMENT CODEX: startup reached";
    let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    env.set_current_dir(dir.path());
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex");
    std::fs::write(
        &codex,
        format!("#!/bin/sh\nprintf '\\033[32m{NEW}\\033[0m\\n'\nexec sleep 300\n"),
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    env.set("PATH", std::env::join_paths(paths).unwrap());

    let mut daemon = daemon(&cas_dir);
    register_idle_worker(&cas_dir, NAME);
    let old = idle_worker_pane(NAME, cas_mux::SupervisorCli::Codex);
    let old_pid = old.process_group_id().unwrap();
    daemon.app.adopt_worker_pane_for_test(old);
    daemon.buffer_pane_output(NAME, OLD.repeat(4_000).as_bytes());
    daemon.buffer_pane_output("unrelated-worker", b"unrelated live output");
    assert!(
        daemon.pane_buffers[NAME]
            .as_plain_text()
            .contains(OLD.trim())
    );

    let spec = cas_mux::WorkerSpec {
        cli: cas_mux::SupervisorCli::Codex,
        model: Some("gpt-6.1-sol".into()),
        effort: Some(cas_mux::Effort::High),
        ..cas_mux::WorkerSpec::builtin_default()
    };
    let request_id = open_spawn_queue_store(&cas_dir)
        .unwrap()
        .enqueue_recycle(
            NAME,
            Some(&serde_json::to_string(&spec).unwrap()),
            Some(SESSION),
        )
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();
    daemon.process_pending_spawns().await;
    assert!(
        !alive(old_pid),
        "the old process stopped before the relaunch"
    );
    let verification = daemon
        .spawn_verifications
        .get(NAME)
        .expect("relaunch awaits registration");
    assert_eq!(verification.request_id, Some(request_id));
    assert_eq!(daemon.app.harness_for(NAME), cas_mux::SupervisorCli::Codex);

    // Drive the real mux output handler; the child, not the test, emits NEW.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, events) = daemon.app.mux.poll_batch();
        for event in events {
            daemon.handle_mux_event(event).await;
        }
        if daemon
            .pane_buffers
            .get(NAME)
            .is_some_and(|buffer| buffer.as_plain_text().contains(NEW))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "replacement startup line never arrived"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    if register_replacement {
        let mut replacement = cas_types::Agent::new("replacement-session".into(), NAME.into());
        replacement.role = cas_types::AgentRole::Worker;
        replacement.factory_session = Some(SESSION.into());
        open_agent_store(&cas_dir)
            .unwrap()
            .register(&replacement)
            .unwrap();
    } else {
        // Expire verification without sleeping through the registration timeout.
        daemon
            .spawn_verifications
            .get_mut(NAME)
            .unwrap()
            .launched_at = Instant::now() - Duration::from_secs(600);
    }
    daemon.reconcile_spawn_verifications().await;
    let (state, detail) = shutdown_row_state(&cas_dir, request_id);
    if register_replacement {
        assert_eq!(
            state.as_deref(),
            Some(SpawnLifecycleState::Registered.as_str()),
            "{detail:?}"
        );
        let output = daemon.pane_buffers[NAME].as_plain_text();
        assert_eq!(
            output.trim(),
            NEW,
            "a registered replacement replays only its own output"
        );
        daemon.app.shutdown_worker_for_recycle(NAME).await.unwrap();
    } else {
        assert_eq!(
            state.as_deref(),
            Some(SpawnLifecycleState::Failed.as_str()),
            "{detail:?}"
        );
        let detail = detail.expect("timeout has a persisted reason");
        assert!(detail.contains("did not register"), "{detail}");
        assert!(
            detail.contains(NEW),
            "timeout names the replacement's startup line: {detail}"
        );
        assert!(
            !detail.contains(OLD.trim()),
            "timeout must exclude the old transcript: {detail}"
        );
        let notices = receipts(&cas_dir, daemon.app.supervisor_name());
        assert!(
            notices
                .iter()
                .any(|notice| notice.contains("stage=register") && notice.contains(NEW)),
            "{notices:?}"
        );
        assert!(
            notices.iter().all(|notice| !notice.contains(OLD.trim())),
            "{notices:?}"
        );
    }
    assert_eq!(
        daemon.pane_buffers["unrelated-worker"].as_plain_text(),
        "unrelated live output"
    );
}
