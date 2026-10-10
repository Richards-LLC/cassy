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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
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
