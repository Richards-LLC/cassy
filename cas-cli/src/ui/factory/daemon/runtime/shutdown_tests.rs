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
