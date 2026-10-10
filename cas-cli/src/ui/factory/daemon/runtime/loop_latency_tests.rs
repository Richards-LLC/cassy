//! GH #1165: the factory daemon loop keeps forwarding input and drawing while
//! another holder keeps `task-sync-intents.lock` and the SQLite write lock.
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
/// reminders and a draw with the task dialog open. Loop-thread task
/// mutations still take the intents flock until cas-78ac4 bounds it by the
/// thread's wait budget; the control test below pins that behaviour.
async fn store_phases(
    daemon: &mut FactoryDaemon,
    terminal: &mut Terminal<BufferBackend>,
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
    store_phases(&mut daemon, &mut terminal).await;
    let mut uncontended = Duration::ZERO;
    for _ in 0..3 {
        let started = Instant::now();
        store_phases(&mut daemon, &mut terminal).await;
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
            store_phases(&mut daemon, &mut terminal).await
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
