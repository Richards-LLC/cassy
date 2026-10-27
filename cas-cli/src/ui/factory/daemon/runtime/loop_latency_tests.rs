//! GH #1165: the factory daemon loop keeps forwarding input and drawing while
//! another holder keeps `task-sync-intents.lock` and the SQLite write lock.
//! The flock holder takes the lock exclusively, as a pre-#1165 `cas serve`
//! still does; that blocks the shared lease every task mutation now takes.
//!
//! The holders here are other threads with their own file descriptions and
//! SQLite connections. `flock` locks belong to the open file description and
//! SQLite locks to the connection, so the daemon code waits on them exactly
//! as it waits on another process.
use super::super::imports::*;
use crate::ui::factory::app::task_dialog_load::TaskDialogLoad;
use cas_store::wait_budget;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// The pass latency #1165 asks for.
const PASS_LATENCY_LIMIT: Duration = Duration::from_millis(100);
/// How long the foreign holders would keep their locks if never released.
const FOREIGN_HOLD: Duration = Duration::from_secs(30);

/// A logged-in project, so task writes go through the cloud-sync wrapper and
/// its intents flock.
fn logged_in_project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    std::fs::write(cas_dir.join("cloud.json"), r#"{"token":"test-token"}"#).unwrap();
    (dir, cas_dir)
}

/// A thread holding a lock until released or [`FOREIGN_HOLD`] passes.
struct ForeignHolder {
    release: mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl ForeignHolder {
    fn release(self) {
        let _ = self.release.send(());
        self.thread.join().unwrap();
    }
}

fn hold_intents_flock(cas_dir: &Path) -> ForeignHolder {
    use fs2::FileExt;
    let path = cas_dir.join("task-sync-intents.lock");
    let (acquired_tx, acquired) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.lock_exclusive().unwrap();
        acquired_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(FOREIGN_HOLD);
        FileExt::unlock(&file).unwrap();
    });
    acquired.recv().unwrap();
    ForeignHolder { release, thread }
}

fn hold_sqlite_write_lock(cas_dir: &Path) -> ForeignHolder {
    let path = cas_dir.join("cas.db");
    let (acquired_tx, acquired) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        acquired_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(FOREIGN_HOLD);
        conn.execute_batch("ROLLBACK").unwrap();
    });
    acquired.recv().unwrap();
    ForeignHolder { release, thread }
}

fn seed(cas_dir: &Path) -> cas_types::Task {
    let store = crate::store::open_task_store_cached(cas_dir).unwrap();
    let mut task = cas_types::Task::new("cas-1165".into(), "frozen loop".into());
    task.assignee = Some("worker-a".into());
    store.add(&task).unwrap();
    let queue = open_prompt_queue_store(cas_dir).unwrap();
    queue
        .enqueue("supervisor", "worker-a", "hello while the store is locked")
        .unwrap();
    task
}

/// The store-touching work of one loop pass that #1165 traced: the prompt
/// queue, the spawn queue, spawn verification, the two-second refresh,
/// reminders, a worker task mutation (spawn/shutdown/assignment paths write
/// tasks on the loop thread) and a draw with the task dialog open.
async fn store_phases(
    daemon: &mut FactoryDaemon,
    terminal: &mut Terminal<BufferBackend>,
    task: &cas_types::Task,
) -> Vec<(&'static str, Duration)> {
    let mut phases = Vec::new();
    let mut mark = Instant::now();
    let mut lap = |name: &'static str, phases: &mut Vec<(&'static str, Duration)>| {
        phases.push((name, mark.elapsed()));
        mark = Instant::now();
    };
    let _ = daemon.process_prompt_queue().await;
    lap("prompt queue", &mut phases);
    let _ = daemon.enqueue_spawn_requests();
    lap("spawn queue", &mut phases);
    daemon.reconcile_spawn_verifications().await;
    lap("spawn verification", &mut phases);
    let _ = daemon.app.refresh_data();
    lap("refresh", &mut phases);
    daemon.process_reminders(&[]);
    lap("reminders", &mut phases);
    if let Ok(store) = crate::store::open_task_store_cached(daemon.app.cas_dir()) {
        let _ = store.update(task);
    }
    lap("task mutation", &mut phases);
    {
        let _forbid = wait_budget::forbid_store_access();
        terminal.draw(|frame| daemon.app.render(frame)).unwrap();
    }
    lap("draw", &mut phases);
    phases
}

#[tokio::test]
async fn loop_pass_stays_fast_while_the_intents_flock_and_sqlite_write_lock_are_held() {
    let (_dir, cas_dir) = logged_in_project();
    let task = seed(&cas_dir);
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.app.show_task_dialog = true;
    daemon.app.task_dialog_id = Some(task.id.clone());
    daemon.app.task_dialog_load = TaskDialogLoad::start(cas_dir.clone(), task.id.clone());
    let mut terminal = Terminal::new(BufferBackend::with_hyperlinks(
        120,
        40,
        daemon.app.full_pane_hyperlink_map(),
    ))
    .unwrap();

    // Uncontended warm-up: the daemon has opened its stores before any pass.
    store_phases(&mut daemon, &mut terminal, &task).await;
    let mut uncontended = Duration::ZERO;
    for _ in 0..3 {
        let started = Instant::now();
        store_phases(&mut daemon, &mut terminal, &task).await;
        uncontended = uncontended.max(started.elapsed());
    }

    let flock = hold_intents_flock(&cas_dir);
    let write_lock = hold_sqlite_write_lock(&cas_dir);
    let mut slowest = Duration::ZERO;
    let mut slowest_phases = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let phases = {
            let _budget = wait_budget::bound_waits_for(super::store_worker::PASS_STORE_WAIT_BUDGET);
            store_phases(&mut daemon, &mut terminal, &task).await
        };
        if started.elapsed() > slowest {
            slowest = started.elapsed();
            slowest_phases = phases;
        }
    }
    write_lock.release();
    flock.release();

    assert!(
        slowest < PASS_LATENCY_LIMIT,
        "slowest pass took {slowest:?} with the intents flock and SQLite write lock held \
         (uncontended {uncontended:?}); phases {slowest_phases:?}"
    );
}

#[test]
fn without_a_budget_the_same_task_write_queues_behind_the_flock() {
    // Control: the bound above is what keeps the pass fast. With no budget
    // the daemon's task write still waits for the foreign flock holder.
    let (_dir, cas_dir) = logged_in_project();
    let task = seed(&cas_dir);
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    let flock = hold_intents_flock(&cas_dir);
    let (done_tx, done) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let _ = store.update(&task);
        done_tx.send(()).unwrap();
    });
    assert!(
        done.recv_timeout(Duration::from_millis(300)).is_err(),
        "an unbudgeted task write must still serialise on the intents flock"
    );
    flock.release();
    done.recv_timeout(Duration::from_secs(10)).unwrap();
    writer.join().unwrap();
}

#[tokio::test]
async fn input_forwarding_exchange_and_draw_touch_no_store() {
    let (_dir, cas_dir) = logged_in_project();
    let task = seed(&cas_dir);
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.listener.set_nonblocking(true).unwrap();
    let mut client = UnixStream::connect(cas_dir.join("test.sock")).unwrap();
    let loop_progress = super::loop_watchdog::LoopProgress::new();
    let mut terminal = Terminal::new(BufferBackend::with_hyperlinks(
        120,
        40,
        daemon.app.full_pane_hyperlink_map(),
    ))
    .unwrap();

    let before = wait_budget::store_access_violations();
    // Typing, arrows, tab and enter, then a task dialog open and redraws.
    let keystrokes: [&[u8]; 6] = [b"hello", b"\x1b[A", b"\x1b[B", b"\t", b"\r", b"\x1b[C"];
    for keys in keystrokes {
        client.write_all(keys).unwrap();
        let _forbid = wait_budget::forbid_store_access();
        daemon.accept_clients().unwrap();
        daemon.process_client_input().await.unwrap();
        daemon.exchange_terminal(&loop_progress).await;
        terminal.draw(|frame| daemon.app.render(frame)).unwrap();
    }
    daemon.app.show_task_dialog = true;
    daemon.app.task_dialog_id = Some(task.id.clone());
    daemon.app.task_dialog_load = TaskDialogLoad::start(cas_dir.clone(), task.id.clone());
    let started = Instant::now();
    while daemon.app.task_dialog_load.is_loading() && started.elapsed() < Duration::from_secs(5) {
        let _forbid = wait_budget::forbid_store_access();
        terminal.draw(|frame| daemon.app.render(frame)).unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    {
        let _forbid = wait_budget::forbid_store_access();
        terminal.draw(|frame| daemon.app.render(frame)).unwrap();
    }
    assert!(daemon.app.task_dialog_load.task_for(&task.id).is_some());

    assert_eq!(
        wait_budget::store_access_violations(),
        before,
        "store access on the input/draw path: {:?}",
        wait_budget::last_store_access_violation()
    );
}

/// C6 review M2 (cas-9db0b): the daemon enforces on its client-input and
/// PTY-exchange steps (`lane_guard::enforce_here` in `FactoryDaemon::run`).
/// Keys typed on the client socket reach a real PTY and its echo is drained,
/// with the lane held exactly as `run` holds it and enforced violations set
/// to panic: no blocking call, no store access.
#[tokio::test]
async fn flv2_input_and_pty_exchange_run_enforced() {
    let (_dir, cas_dir) = logged_in_project();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.listener.set_nonblocking(true).unwrap();
    super::inject_command_tests::ready_claude_worker(&mut daemon).await;
    assert!(daemon.app.mux.focus("worker-a"));
    let mut client = UnixStream::connect(cas_dir.join("test.sock")).unwrap();
    let progress = super::loop_watchdog::LoopProgress::new();
    fn daemon_bytes(daemon: &FactoryDaemon) -> u64 {
        daemon.app.mux.pane_bytes_received("worker-a").unwrap_or(0)
    }

    let before_lane = cas_store::lane_guard::thread_violations();
    let before_store = wait_budget::store_access_violations();
    let echo_before = daemon_bytes(&daemon);
    let mut exchanged = 0usize;
    for keys in [&b"c6-echo"[..], b"\x1b[D", b"\x1b[C"] {
        client.write_all(keys).unwrap();
        let started = Instant::now();
        loop {
            let (attachment, enforced) = super::loop_watchdog::enforced_daemon_step(
                &progress,
                super::loop_watchdog::LoopPhase::ClientInput,
            );
            assert_eq!(
                cas_store::lane_guard::current_mode(),
                Some(cas_store::lane_guard::GuardMode::Enforce)
            );
            let _forbid = wait_budget::forbid_store_access();
            daemon.accept_clients().unwrap();
            daemon.process_client_input().await.unwrap();
            exchanged += daemon.exchange_terminal(&progress).await;
            drop(enforced);
            drop(attachment);
            if daemon_bytes(&daemon) > echo_before || started.elapsed() > Duration::from_secs(5) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert!(
        daemon_bytes(&daemon) > echo_before && exchanged > 0,
        "the typed keys echoed through the PTY exchange"
    );
    assert_eq!(
        cas_store::lane_guard::thread_violations(),
        before_lane,
        "{:?}",
        cas_store::lane_guard::last_thread_violation()
    );
    assert_eq!(wait_budget::store_access_violations(), before_store);
}

#[test]
fn a_task_sync_mutation_under_a_foreign_sqlite_write_lock_stays_in_budget() {
    // Only SQLite is held: the mutation takes its task-sync lease and stripe,
    // then reaches the SyncQueue's BEGIN IMMEDIATE, which must stop at the
    // thread's deadline rather than wait out the 5 s handler and retries.
    let (_dir, cas_dir) = logged_in_project();
    let mut task = seed(&cas_dir);
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    let write_lock = hold_sqlite_write_lock(&cas_dir);
    task.title = "renamed while locked".into();
    let started = Instant::now();
    let result = {
        let _budget = wait_budget::bound_waits_for(super::store_worker::PASS_STORE_WAIT_BUDGET);
        store.update(&task)
    };
    let waited = started.elapsed();
    write_lock.release();
    assert!(result.is_err(), "the write lock outlived the budget");
    assert!(
        waited < PASS_LATENCY_LIMIT,
        "the budgeted mutation waited {waited:?}"
    );
    // Nothing is wedged: the same mutation lands once the lock is gone.
    store.update(&task).unwrap();
    assert_eq!(store.get(&task.id).unwrap().title, "renamed while locked");
}

/// Guard (GH #1165): production code must not restore SQLite's built-in busy
/// timeout with `busy_timeout(SQLITE_BUSY_TIMEOUT)`. That replaces the
/// budget-aware handler and lets a budgeted thread wait the full 5 s again.
/// Use `cas_store::shared_db::install_busy_handler` instead.
#[test]
fn no_production_code_installs_sqlites_builtin_busy_timeout() {
    // Source scan: the archived test binary on a merge-queue shard has no
    // checkout, so this guard runs only where the sources exist.
    let root = crate::test_paths::workspace_root();
    if !root.join("cas-cli/src").is_dir() || !root.join("crates").is_dir() {
        return; // not a source checkout; nothing to assert against
    }
    let mut roots = vec![root.join("cas-cli/src")];
    for entry in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        roots.push(entry.path().join("src"));
    }
    let mut offenders = Vec::new();
    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name.contains("test") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Production code ends at the first test module.
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            for (index, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains("busy_timeout(") && code.contains("SQLITE_BUSY_TIMEOUT") {
                    offenders.push(format!("{}:{}", path.display(), index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "restore the budget-aware handler with install_busy_handler instead: {offenders:?}"
    );
}

/// cas-ee9ab: the agent and event stores locked the process-wide connection
/// with a plain mutex, so a loop pass that listed agents (session mappings,
/// phantom panes, worker relays) waited for whatever another daemon thread
/// was doing on that connection, budget or not. Measured under the cas-98b24
/// load: 5 s waits in "session mappings" and the relays. Both stores now
/// honour the pass budget like the others.
#[test]
fn agent_and_event_reads_stay_in_budget_while_the_connection_is_held_cas_ee9ab() {
    let (_dir, cas_dir) = logged_in_project();
    let agents = crate::store::open_agent_store(&cas_dir).unwrap();
    let events = crate::store::open_event_store(&cas_dir).unwrap();
    let shared = cas_store::shared_db::shared_connection(&cas_dir.join("cas.db")).unwrap();
    let (held_tx, held) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _guard = shared.lock().unwrap();
        held_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(FOREIGN_HOLD);
    });
    held.recv().unwrap();

    let started = Instant::now();
    let (agent_list, recent) = {
        let _budget = wait_budget::bound_waits_for(super::store_worker::PASS_STORE_WAIT_BUDGET);
        (agents.list(None), events.list_recent(10))
    };
    let waited = started.elapsed();
    // The holder may already have given up if the reads waited it out.
    let _ = release.send(());
    holder.join().unwrap();

    assert!(agent_list.is_err(), "the agent list did not wait for the held connection");
    assert!(recent.is_err(), "the event list did not wait for the held connection");
    assert!(
        waited < PASS_LATENCY_LIMIT,
        "agent and event reads waited {waited:?} for another thread's connection"
    );
    // Nothing is wedged: both read once the connection is free.
    assert!(agents.list(None).is_ok());
    assert!(events.list_recent(10).is_ok());
}

/// GH #1208: with pending external reminders whose origin is slow, the
/// reminder pass on the loop runs no fetch. Before the fix it ran one
/// `git fetch` per reminder inline, here at least 5 × 400 ms. The fetches
/// still happen, on the scanner thread, and their failures back off.
///
/// cas-4491f (C2b review L1): the pass runs on the control lane, where every
/// git command, store access, `/proc` scan or transcript read is counted
/// (`lane_guard`); the test asserts that count stays zero instead of timing
/// the pass. Pass latency receipts are C6's.
///
/// spec-4c5b §7 gate test (C2, cas-223f): the pass touches no store at all.
/// Reminder expiry, the due read and firing run as a `ReminderScan` job on
/// the write lane, and a due time reminder still fires.
#[tokio::test]
async fn flv2_external_reminders_never_on_lane() {
    const EXTERNAL_REMINDERS: usize = 5;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let project = dir.path();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(project)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    // Every fetch is a slow round trip to an origin that never answers.
    git(&["remote", "add", "origin", "ssh://cas-test.invalid/repo.git"]);
    git(&["config", "core.sshCommand", "sleep 0.4; false"]);

    let agents = crate::store::open_agent_store(&cas_dir).unwrap();
    agents
        .register(&cas_types::Agent::new("sup".into(), "supervisor".into()))
        .unwrap();
    let reminders = crate::store::open_reminder_store(&cas_dir).unwrap();
    for n in 0..EXTERNAL_REMINDERS {
        let filter = serde_json::json!({"commit": "HEAD", "target_branch": format!("epic/{n}")});
        reminders
            .create_with_scope(
                "sup",
                None,
                "landed",
                cas_store::ReminderTriggerType::Event,
                None,
                Some(super::ci_watch::EXTERNAL_BRANCH_CONTAINED_EVENT),
                Some(&filter),
                0,
                Some("another-factory"),
                None,
                true,
                None,
            )
            .unwrap();
    }

    // Only a retired supervisor can hand its reminder to a successor;
    // worker targets stay pending (CONTRIBUTING, reminder routing policy).
    // This gives the scan a recipient without reading the director.
    let mut gone = cas_types::Agent::new_with_role(
        "gone".into(),
        "previous-supervisor".into(),
        cas_types::AgentRole::Supervisor,
    );
    agents.register(&gone).unwrap();
    gone.status = cas_types::AgentStatus::Shutdown;
    agents.update(&gone).unwrap();
    let due = reminders
        .create_with_scope(
            "gone",
            None,
            "due now",
            cas_store::ReminderTriggerType::Time,
            Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
            None,
            None,
            0,
            None,
            None,
            true,
            None,
        )
        .unwrap();

    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    // Real job lanes: the fixture runs queue jobs inline, on this thread.
    daemon.queue_jobs =
        super::prompt_queue_jobs::QueueJobs::start(cas_dir.clone(), daemon.app.project_scope());
    assert_eq!(daemon.app.project_path(), project);
    let violations = wait_budget::store_access_violations();
    let lane_violations = cas_store::lane_guard::thread_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        let _control = cas_store::lane_guard::enter_lane(cas_store::lane_guard::Lane::Control);
        daemon.process_reminders(&[]);
    }
    assert_eq!(
        cas_store::lane_guard::thread_violations(),
        lane_violations,
        "the reminder pass ran blocking work on the loop with {EXTERNAL_REMINDERS} pending external reminders: {:?}",
        cas_store::lane_guard::last_thread_violation()
    );
    assert_eq!(
        wait_budget::store_access_violations(),
        violations,
        "the reminder pass read the store on the loop: {:?}",
        wait_budget::last_store_access_violation()
    );
    // The due time reminder fires from the ReminderScan job.
    let deadline = Instant::now() + Duration::from_secs(10);
    while reminders
        .get_due_time_reminders()
        .unwrap()
        .iter()
        .any(|reminder| reminder.id == due)
    {
        assert!(Instant::now() < deadline, "the due reminder never fired");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        reminders.list_recently_fired(60).unwrap().iter().any(|r| r.id == due),
        "the due time reminder must fire, not merely expire"
    );

    // The scan ran off the loop: every branch was fetched, failed, and is
    // now backing off. No condition was true, so nothing fired.
    let ready = daemon
        .external_wake
        .wait_for_scan(Duration::from_secs(30))
        .expect("the background scan finishes");
    assert!(ready.is_empty());
    let backoff = daemon.external_wake.backoff().expect("backoff returned with the scan");
    for n in 0..EXTERNAL_REMINDERS {
        let key = super::ci_watch::ExternalProbeKey::Branch(format!("epic/{n}"));
        assert_eq!(backoff.failures(&key), 1, "{key}");
    }
    assert_eq!(reminders.get_event_reminders("branch_contained_in").unwrap().len(), EXTERNAL_REMINDERS);

    // A second pass inside the interval neither waits nor starts a scan.
    {
        let _forbid = wait_budget::forbid_store_access();
        let _control = cas_store::lane_guard::enter_lane(cas_store::lane_guard::Lane::Control);
        daemon.process_reminders(&[]);
    }
    assert_eq!(cas_store::lane_guard::thread_violations(), lane_violations);
    assert_eq!(wait_budget::store_access_violations(), violations);
    assert!(daemon.external_wake.wait_for_scan(Duration::ZERO).is_none());
}

/// Factory loop v2 §6.6 (cas-6931; enforce mode C6, cas-9db0b): a store
/// open, a `git` command, an OS process probe and a transcript read on an
/// enforcing io or control lane each trip the assertion, naming the call,
/// lane, step and site. Off a lane nothing is reported. The daemon loop's
/// own lane (`LoopProgress::attach`) stays in report mode until C2c
/// (cas-4491f): there the same calls are reported per site with their step
/// and run as before.
#[test]
fn flv2_no_blocking_call_on_lanes() {
    use crate::cli::factory::wedged::{self, ProcessTable as _};
    use cas_store::lane_guard::{self, BlockingKind, GuardMode, Lane};

    let (dir, cas_dir) = logged_in_project();
    let transcript = dir.path().join("transcript.jsonl");
    std::fs::write(&transcript, "{\"type\":\"assistant\"}\n").unwrap();
    let calls: [(BlockingKind, &dyn Fn()); 4] = [
        (BlockingKind::Store, &|| {
            crate::store::open_agent_store(&cas_dir)
                .unwrap()
                .list(None)
                .unwrap();
        }),
        (BlockingKind::Command, &|| {
            let _ = crate::git_evidence::git_text(dir.path(), &["rev-parse", "--git-dir"]);
        }),
        (BlockingKind::Proc, &|| {
            if cfg!(target_os = "macos") {
                // RealProcessTable is a no-op off Linux. Exercise libproc,
                // the native process read used for macOS PID fingerprints.
                assert!(crate::mcp::daemon::read_pid_starttime(std::process::id()).is_some());
            } else {
                let _ = wedged::RealProcessTable.pids();
            }
        }),
        (BlockingKind::Transcript, &|| {
            let _ = wedged::transcript_has_in_flight_tool_call(
                &transcript,
                cas_mux::SupervisorCli::Claude,
            );
        }),
    ];
    let blocking_calls = || {
        let mut kinds = Vec::new();
        for (kind, call) in &calls {
            let before = lane_guard::thread_violations();
            call();
            if lane_guard::thread_violations() > before {
                kinds.push(*kind);
            }
        }
        kinds
    };

    assert!(blocking_calls().is_empty(), "off a lane nothing is reported");
    for lane in [Lane::Io, Lane::Control] {
        for (kind, call) in &calls {
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _lane = super::loop_watchdog::enforce_test_lane(lane);
                lane_guard::set_step("flv2 enforce probe");
                call();
            }));
            let payload = caught.expect_err("an enforced lane trips on a blocking call");
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default();
            // The call that tripped is this kind's own, on this lane.
            let (tripped_lane, tripped_kind, what) =
                lane_guard::last_thread_violation().expect("the tripping call is recorded");
            assert_eq!((tripped_lane, tripped_kind), (lane, *kind), "{message}");
            assert!(
                message.starts_with(&format!(
                    "blocking {} call `{what}` on the {} lane in step `flv2 enforce probe` at ",
                    kind.name(),
                    lane.name()
                )),
                "{kind:?} on {lane:?}: {message}"
            );
            assert_eq!(lane_guard::current_lane(), None, "the lane unwinds");
        }
    }

    // The daemon loop's lane reports: every kind is counted and runs.
    let every_kind = vec![
        BlockingKind::Store,
        BlockingKind::Command,
        BlockingKind::Proc,
        BlockingKind::Transcript,
    ];
    let progress = super::loop_watchdog::LoopProgress::new();
    {
        let _lane = progress.attach(super::loop_watchdog::Lane::Control);
        assert_eq!(lane_guard::current_mode(), Some(GuardMode::Report));
        progress.enter(super::loop_watchdog::LoopPhase::ClientInput);
        assert_eq!(blocking_calls(), every_kind, "on the io lane");
        progress.enter(super::loop_watchdog::LoopPhase::PromptQueue);
        assert_eq!(blocking_calls(), every_kind, "on the control lane");
        assert_eq!(
            lane_guard::last_thread_violation().map(|last| last.0),
            Some(Lane::Control)
        );
    }
    assert_eq!(lane_guard::current_lane(), None);

    // The daemon loop attaches its thread as the control lane; io steps
    // switch it to io and back.
    {
        let _lane = progress.attach(super::loop_watchdog::Lane::Control);
        assert_eq!(lane_guard::current_lane(), Some(Lane::Control));
        progress.enter(super::loop_watchdog::LoopPhase::ClientInput);
        assert_eq!(lane_guard::current_lane(), Some(Lane::Io));
        progress.enter(super::loop_watchdog::LoopPhase::PromptQueue);
        assert_eq!(lane_guard::current_lane(), Some(Lane::Control));
    }
    assert_eq!(lane_guard::current_lane(), None);

    // `loop.json` carries the report.
    let status = progress.status(1, "flv2", chrono::Utc::now());
    assert!(status.lane_violations >= 8, "{}", status.lane_violations);
    for kind in ["io/store", "control/command", "io/proc", "control/transcript"] {
        assert!(
            status.lane_violation_sites.iter().any(|site| site.starts_with(kind)),
            "{kind} missing from {:?}",
            status.lane_violation_sites
        );
    }
    // Each site names the loop step it ran in (C6), e.g. the prompt queue.
    assert!(
        status
            .lane_violation_sites
            .iter()
            .any(|site| site.starts_with("control/transcript") && site.contains(" in prompt queue (x")),
        "{:?}",
        status.lane_violation_sites
    );
}

/// cas-cd73: an operator's Commander send arrives on the loop thread, often
/// after earlier work in the pass has spent [`super::store_worker::PASS_STORE_WAIT_BUDGET`].
/// Under brief write contention it must still enqueue and answer
/// `MessageQueued`, not "semantic message enqueue failed" ("Not sent").
#[tokio::test(flavor = "current_thread")]
async fn commander_send_queues_under_a_spent_pass_budget_and_brief_write_contention_cas_cd73() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    let write_lock = hold_sqlite_write_lock(&cas_dir);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        write_lock.release();
    });

    let reply = {
        let _spent = wait_budget::bound_waits_until(Instant::now());
        daemon
            .dispatch_commander_control(super::delivery::CommanderControl::SendMessage {
                target: "supervisor".to_string(),
                text: "sent from the phone while the factory is busy".to_string(),
                summary: None,
                urgent: false,
                client_ref: Some("phone-1".to_string()),
                in_reply_to: None,
                attribution: crate::ui::factory::protocol::MessageAttribution {
                    device_id: Some("device-phone".to_string()),
                    credential_id: Some("credential-phone".to_string()),
                    device_label: Some("Phone".to_string()),
                    operator_label: Some("Operator".to_string()),
                    controller_origin: None,
                    request_id: None,
                    scopes: vec!["message:send".to_string()],
                    operator_verified: true,
                },
            })
            .await
    };
    release.join().unwrap();

    let Ok(Some(crate::ui::factory::DaemonMessage::MessageQueued {
        notification_id,
        client_ref,
        ..
    })) = reply
    else {
        panic!("expected MessageQueued, got {reply:?}");
    };
    assert_eq!(client_ref.as_deref(), Some("phone-1"));
    let row = open_prompt_queue_store(&cas_dir)
        .unwrap()
        .queued_prompt(notification_id)
        .unwrap();
    assert!(row.is_some(), "the send wrote its prompt_queue row");
}
