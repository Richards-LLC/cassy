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
const REPLACEMENT_STARTUP: &str = "REPLACEMENT CODEX: startup reached";

/// Exercise the real daemon loop out of process: no LLM or operator launch
/// gate, and no signals installed in the test runner itself.
#[cfg(unix)]
#[test]
fn factory_daemon_lifetime_cleans_owned_groups_and_preserves_detach_cas_8fac1() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process::{Child, Stdio};

    const NAME: &str = "ui::factory::daemon::runtime::shutdown_tests::factory_daemon_lifetime_cleans_owned_groups_and_preserves_detach_cas_8fac1";
    if let Some(root) = std::env::var_os("CAS_8FAC1_CHILD") {
        let root = std::path::PathBuf::from(root);
        let cas_dir = crate::store::init_cas_dir(&root.join("project")).unwrap();
        let mut daemon = daemon(&cas_dir);
        daemon.listener.set_nonblocking(true).unwrap();
        daemon.gui_listener.set_nonblocking(true).unwrap();
        let pty = cas_mux::Pty::spawn("owned-worker", cas_mux::PtyConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "trap '' HUP TERM; sleep 300 & echo $! > \"$1\"; wait".into(),
                "fixture".into(), root.join("descendant").to_string_lossy().into_owned()],
            cwd: Some(root.clone()), env: vec![], env_remove: vec![], rows: 24, cols: 80,
        }).unwrap();
        let pgid = pty.process_group_id().unwrap();
        daemon.app.adopt_worker_pane_for_test(cas_mux::Pane::with_pty(
            "owned-worker", cas_mux::PaneKind::Worker, pty, 24, 80, cas_mux::SupervisorCli::Claude,
        ).unwrap());
        std::fs::write(root.join("group"), pgid.to_string()).unwrap();
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
            .block_on(daemon.run()).unwrap();
        return;
    }

    struct Owned {
        child: Child,
        root: std::path::PathBuf,
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            // Only the PTY group recorded by this fixture, never a name scan.
            if let Ok(group) = std::fs::read_to_string(self.root.join("group"))
                && let Ok(group) = group.parse::<i32>()
            {
                unsafe { libc::killpg(group, libc::SIGKILL); }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    fn running(pid: u32) -> bool {
        std::process::Command::new("/bin/ps").args(["-o", "stat=", "-p", &pid.to_string()])
            .output().ok().is_some_and(|output| {
                let state = String::from_utf8_lossy(&output.stdout);
                !state.trim().is_empty() && !state.trim().starts_with('Z')
            })
    }

    struct Shared(Child);
    impl Drop for Shared {
        fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
    }
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let mut shared = Shared(std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap());
    let mut failures = Vec::new();
    for route in ["quit", "term", "int", "hup", "client-death", "detach"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("project")).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args([NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env_clear().env("HOME", root).env("PATH", "/usr/bin:/bin")
            .env("XDG_CONFIG_HOME", root.join(".config")).env("CAS_8FAC1_CHILD", root);
        let child = super::super::process::spawn_daemon_process(command,
            std::fs::File::create(root.join("daemon.log")).unwrap()).unwrap();
        let mut owned = Owned { child, root: root.into() };
        let socket = root.join("project/.cas/test.sock");
        assert!(wait_until(Duration::from_secs(20), || socket.exists() && root.join("descendant").exists()),
            "{route}: daemon startup: {}", std::fs::read_to_string(root.join("daemon.log")).unwrap());
        let mut client = UnixStream::connect(&socket).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        client.read_exact(&mut [0u8; 1]).unwrap(); // run() has accepted us and installed its lifecycle handlers.
        let group: u32 = std::fs::read_to_string(root.join("group")).unwrap().parse().unwrap();
        let descendant: u32 = std::fs::read_to_string(root.join("descendant")).unwrap().trim().parse().unwrap();
        let daemon_pid = owned.child.id();
        let listing = std::process::Command::new("/bin/ps")
            .args(["-o", "pid,ppid,pgid,stat,command", "-p", &format!("{daemon_pid},{group},{descendant},{}", shared.0.id())])
            .stdout(Stdio::piped()).output().unwrap();
        eprintln!("{route} BEFORE\n{}", String::from_utf8_lossy(&listing.stdout));
        if unsafe { libc::getpgid(daemon_pid as i32) } != daemon_pid as i32 {
            failures.push(format!("{route}: daemon still shares its launcher's process group"));
        }
        match route {
            "quit" => client.write_all(&[0x11]).unwrap(),
            "client-death" => {},
            "detach" => {
                let mut detach = CONTROL_PREFIX.to_vec();
                detach.extend_from_slice(b"detach"); detach.push(CONTROL_SUFFIX);
                client.write_all(&detach).unwrap();
            },
            signal => unsafe { libc::kill(daemon_pid as i32, match signal {
                "term" => libc::SIGTERM, "int" => libc::SIGINT, _ => libc::SIGHUP,
            }); },
        }
        drop(client);
        let mut reattached = None;
        if route == "detach" {
            std::thread::sleep(Duration::from_millis(700));
            if owned.child.try_wait().unwrap().is_some() || !running(group) || !running(descendant) {
                failures.push("intentional detach must retain the daemon and its children".into());
            } else {
                let mut stream = UnixStream::connect(&socket).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                stream.read_exact(&mut [0u8; 1]).unwrap();
                stream.write_all(&[0x11]).unwrap();
                reattached = Some(stream);
            }
        }
        let exited = wait_until(Duration::from_secs(12), || owned.child.try_wait().unwrap().is_some());
        drop(reattached);
        let gone = wait_until(Duration::from_secs(2), || !running(group) && !running(descendant));
        if !exited || !gone {
            failures.push(format!("{route}: exited={exited}, children_gone={gone}"));
        }
        let after = std::process::Command::new("/bin/ps")
            .args(["-o", "pid,ppid,pgid,stat,command", "-p", &format!("{daemon_pid},{group},{descendant},{}", shared.0.id())])
            .output().unwrap();
        eprintln!("{route} AFTER\n{}", String::from_utf8_lossy(&after.stdout));
        assert!(shared.0.try_wait().unwrap().is_none(), "a shared service must be unaffected");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A watchdog can leave its 80x24 launch client connected while the operator
/// attaches from a larger terminal. The new owner must drive both the TUI and
/// the kernel PTY sizes; a later resize must also win over that old client.
#[cfg(unix)]
#[test]
fn detached_factory_attach_and_later_resize_use_the_owner_terminal_cas_9631() {
    use crate::ui::factory::protocol::{DaemonMessage, FRAME_HEADER_SIZE, decode_length};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::process::Child;

    const NAME: &str = "ui::factory::daemon::runtime::shutdown_tests::detached_factory_attach_and_later_resize_use_the_owner_terminal_cas_9631";
    if let Some(root) = std::env::var_os("CAS_9631_CHILD") {
        let root = std::path::PathBuf::from(root);
        let cas_dir = crate::store::init_cas_dir(&root.join("project")).unwrap();
        let mut daemon = daemon(&cas_dir);
        daemon.listener.set_nonblocking(true).unwrap();
        daemon.gui_listener.set_nonblocking(true).unwrap();
        let mut groups = Vec::new();
        for (name, kind) in [
            ("test-supervisor", cas_mux::PaneKind::Supervisor),
            ("owned-worker", cas_mux::PaneKind::Worker),
        ] {
            let size_file = root.join(format!("{name}.size"));
            let pty = cas_mux::Pty::spawn(
                name,
                cas_mux::PtyConfig {
                    command: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        "while :; do stty size > \"$1\"; sleep 0.1; done".into(),
                        "fixture".into(),
                        size_file.to_string_lossy().into_owned(),
                    ],
                    cwd: Some(root.clone()),
                    env: vec![],
                    env_remove: vec![],
                    rows: 24,
                    cols: 80,
                },
            )
            .unwrap();
            groups.push(pty.process_group_id().unwrap());
            let pane =
                cas_mux::Pane::with_pty(name, kind.clone(), pty, 24, 80, cas_mux::SupervisorCli::Claude)
                    .unwrap();
            if kind == cas_mux::PaneKind::Worker {
                daemon.app.adopt_worker_pane_for_test(pane);
            } else {
                daemon.app.mux.add_pane(pane);
            }
        }
        std::fs::write(
            root.join("groups"),
            groups
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(daemon.run())
            .unwrap();
        return;
    }

    struct Owned {
        child: Child,
        root: std::path::PathBuf,
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            if let Ok(groups) = std::fs::read_to_string(self.root.join("groups")) {
                for group in groups.lines().filter_map(|line| line.parse::<i32>().ok()) {
                    unsafe {
                        libc::killpg(group, libc::SIGKILL);
                    }
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    fn connect_tui(path: &std::path::Path) -> UnixStream {
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.read_exact(&mut [0u8; 1]).unwrap();
        stream
    }
    fn resize(stream: &mut UnixStream, cols: u16, rows: u16) {
        let mut frame = CONTROL_PREFIX.to_vec();
        frame.extend_from_slice(format!("resize;{cols};{rows}").as_bytes());
        frame.push(CONTROL_SUFFIX);
        stream.write_all(&frame).unwrap();
    }
    fn gui_size(path: &std::path::Path) -> (u16, u16) {
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = [0u8; FRAME_HEADER_SIZE];
        stream.read_exact(&mut header).unwrap();
        let len = decode_length(&header);
        assert!(len < 1_000_000, "bounded GUI welcome");
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).unwrap();
        match serde_json::from_slice::<DaemonMessage>(&body).unwrap() {
            DaemonMessage::Welcome { state, .. } => (state.cols, state.rows),
            other => panic!("expected GUI Welcome, got {other:?}"),
        }
    }
    fn pty_size(path: &std::path::Path) -> Option<(u16, u16)> {
        let value = std::fs::read_to_string(path).ok()?;
        let mut parts = value.split_whitespace();
        let rows = parts.next()?.parse().ok()?;
        let cols = parts.next()?.parse().ok()?;
        Some((cols, rows))
    }

    let _env = crate::test_support::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("project")).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env_clear()
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("CAS_9631_CHILD", root);
    let child = super::super::process::spawn_daemon_process(
        command,
        std::fs::File::create(root.join("daemon.log")).unwrap(),
    )
    .unwrap();
    let mut owned = Owned {
        child,
        root: root.into(),
    };
    let tui_socket = root.join("project/.cas/test.sock");
    let gui_socket = root.join("project/.cas/gui.sock");
    assert!(
        wait_until(Duration::from_secs(20), || root.join("groups").exists()),
        "daemon startup: {}",
        std::fs::read_to_string(root.join("daemon.log")).unwrap()
    );

    // The watchdog's detached 80x24 client stays connected but stops sending input.
    let mut launch_client = connect_tui(&tui_socket);
    resize(&mut launch_client, 80, 24);
    std::thread::sleep(Duration::from_secs(INPUT_OWNER_IDLE_SECS + 1));
    let mut operator = connect_tui(&tui_socket);
    let mut layout = FactoryApp::for_test_at(root.join("project/.cas"));
    layout.add_worker_name_for_test("owned-worker");
    for (cols, rows) in [(200, 60), (120, 40)] {
        resize(&mut operator, cols, rows);
        layout.handle_resize(cols, rows).unwrap();
        let expected = ["test-supervisor", "owned-worker"].map(|name| {
            let (pane_cols, pane_rows) = layout.dashboard_pane_size(name).unwrap();
            (name, (pane_cols, pane_rows))
        });
        let panes_match = wait_until(Duration::from_secs(5), || {
            expected
                .iter()
                .all(|(name, size)| pty_size(&root.join(format!("{name}.size"))) == Some(*size))
        });
        let tui = gui_size(&gui_socket);
        assert_eq!(tui, (cols, rows), "the attaching owner's TUI size must win");
        assert!(
            panes_match,
            "PTY sizes at {cols}x{rows}: expected {expected:?}, got {:?}; daemon: {}",
            expected.map(|(name, _)| (name, pty_size(&root.join(format!("{name}.size"))))),
            std::fs::read_to_string(root.join("daemon.log")).unwrap()
        );
    }

    // A viewer's resize records its own geometry without replacing the active owner.
    resize(&mut operator, 121, 41);
    assert!(wait_until(Duration::from_secs(5), || gui_size(&gui_socket) == (121, 41)));
    resize(&mut launch_client, 105, 33);
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(gui_size(&gui_socket), (121, 41));
    drop(operator);
    assert!(
        wait_until(Duration::from_secs(5), || gui_size(&gui_socket) == (105, 33)),
        "owner loss must use the remaining viewer's latest size"
    );

    let mut operator = connect_tui(&tui_socket);
    resize(&mut operator, 180, 50);
    assert!(wait_until(Duration::from_secs(5), || gui_size(&gui_socket) == (180, 50)));
    std::thread::sleep(Duration::from_secs(INPUT_OWNER_IDLE_SECS + 1));
    launch_client.write_all(b"x").unwrap();
    assert!(
        wait_until(Duration::from_secs(5), || gui_size(&gui_socket) == (105, 33)),
        "typing takeover must apply the new owner's recorded terminal size"
    );
    launch_client.write_all(&[0x11]).unwrap();
    drop(operator);
    drop(launch_client);
    assert!(
        wait_until(Duration::from_secs(12), || owned
            .child
            .try_wait()
            .unwrap()
            .is_some()),
        "daemon did not stop after quit"
    );
}

/// macOS has no /proc discovery. Keep that OS boundary deterministic on CI
/// while retaining real PID checks, registry reads, PTYs and queue consumers.
struct EmptyProcessTable;

impl crate::cli::factory::wedged::ProcessTable for EmptyProcessTable {
    fn pids(&self) -> Vec<u32> {
        Vec::new()
    }

    fn cmdline(&self, _pid: u32) -> Option<Vec<u8>> {
        None
    }

    fn environ(&self, _pid: u32) -> Option<Vec<u8>> {
        None
    }
}

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
    if pid == 0 || pid > i32::MAX as u32 || !crate::mcp::daemon::pid_alive(pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
            })
            .is_some_and(|state| state != "Z" && state != "X")
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS has no /proc. Signal 0 above probes existence; ps keeps the
        // same zombie exclusion as the Linux check and the daemon fixture.
        std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()
            .is_some_and(|output| {
                let state = String::from_utf8_lossy(&output.stdout);
                let state = state.trim();
                output.status.success() && !state.is_empty() && !state.starts_with(['Z', 'X'])
            })
    }
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
/// queue row and the supervisor. The recipe here passes routing policy, so
/// the stop runs (cas-033a refuses a policy-rejected recipe before it), but
/// its Claude account holds a malformed `.claude.json`: the relaunch refuses
/// to record workspace trust after the old worker stops, which exercises the
/// failure report without launching a real harness.
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

    let account = tempfile::tempdir().unwrap();
    std::fs::write(account.path().join(".claude.json"), "{ not json").unwrap();
    let unlaunchable = cas_mux::WorkerSpec {
        cli: cas_mux::SupervisorCli::Claude,
        model: Some("claude-opus-5-5".to_string()),
        effort: Some(cas_mux::Effort::High),
        config_dir: Some(account.path().display().to_string()),
        ..cas_mux::WorkerSpec::builtin_default()
    };
    cas_factory::validate_explicit(&unlaunchable, &cas_factory::CapabilitySnapshot::default())
        .expect("fixture: the recipe passes routing policy, so the stop runs");
    let queue = open_spawn_queue_store(&cas_dir).unwrap();
    let request_id = queue
        .enqueue_recycle(
            name,
            Some(&serde_json::to_string(&unlaunchable).unwrap()),
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

/// cas-033a: a recycle whose recipe routing policy refuses never stops the
/// worker. The row still ends `failed` and the supervisor still hears why,
/// even under a spent pass budget.
#[tokio::test(flavor = "current_thread")]
async fn recycle_with_a_refused_recipe_keeps_the_worker_running_cas_033a() {
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
        cli: cas_mux::SupervisorCli::Codex,
        model: Some("claude-opus-5-5".to_string()),
        ..cas_mux::WorkerSpec::builtin_default()
    };
    assert!(
        cas_factory::validate_explicit(&refused, &cas_factory::CapabilitySnapshot::default())
            .is_err(),
        "fixture: the incident recipe (codex + a Claude model) is refused"
    );
    let queue = open_spawn_queue_store(&cas_dir).unwrap();
    let request_id = queue
        .enqueue_recycle(
            name,
            Some(&serde_json::to_string(&refused).unwrap()),
            Some(SESSION),
        )
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();
    {
        let _spent = cas_store::wait_budget::bound_waits_until(Instant::now());
        daemon.process_pending_spawns().await;
    }

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
    assert!(alive(pid), "a refused recipe must not stop the worker");
    assert!(
        daemon
            .app
            .worker_names()
            .iter()
            .any(|worker| worker == name),
        "the worker keeps its pane"
    );
    let prompts = open_prompt_queue_store(&cas_dir).unwrap();
    assert!(
        wait_until(Duration::from_secs(15), || prompts
            .peek_all(50)
            .unwrap()
            .iter()
            .any(|row| row.target == daemon.app.supervisor_name()
                && row.prompt.contains(&format!("request {request_id}"))
                && row.prompt.contains("the worker keeps running"))),
        "the supervisor is told the recycle was refused before the stop"
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
    if !run_hermetic_recycle_child(
        "long_transcript_recycle_timeout_quotes_only_replacement_cas_c1a2",
    ) {
        long_transcript_recycle(false).await;
    }
}

/// The old output must also be gone when the replacement registers normally.
/// This is the successful control for the deliberately unregistered child.
#[tokio::test(flavor = "current_thread")]
async fn long_transcript_recycle_registers_fresh_session_cas_c1a2() {
    if !run_hermetic_recycle_child("long_transcript_recycle_registers_fresh_session_cas_c1a2") {
        long_transcript_recycle(true).await;
    }
}

/// Keep harness discovery local to a child process, including the CAS binary
/// required by the Codex PTY preflight. Neither stand-in uses the host install.
fn run_hermetic_recycle_child(test_name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    if std::env::var_os("CAS_C1A2_RECYCLE_CHILD").is_some() {
        return false;
    }
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for (name, script) in [
        (
            "codex",
            format!(
                "#!/bin/sh\nprintf '\\033[32m{REPLACEMENT_STARTUP}\\033[0m\\n'\nexec /bin/sleep 300\n"
            ),
        ),
        ("cas", "#!/bin/sh\nexit 0\n".into()),
    ] {
        let executable = bin.join(name);
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths([bin, "/usr/bin".into(), "/bin".into()]).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .env("PATH", path)
        .env("CAS_C1A2_RECYCLE_CHILD", "1")
        .arg(format!(
            "ui::factory::daemon::runtime::shutdown_tests::{test_name}"
        ))
        .arg("--exact")
        .arg("--test-threads=1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "hermetic {test_name}:\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains("test result: ok. 1 passed;"), "{stdout}");
    true
}

/// Reproduce the lane environment inside the capped test runner: no login
/// profile, local bin directory, inherited CAS identity, or installed harness.
#[test]
fn shutdown_module_and_env_policy_pass_without_local_harnesses_cas_c1a2() {
    if std::env::var_os("CAS_C1A2_CLEAN_SUITE").is_some() {
        return; // The child runs this same module; do not recursively spawn it.
    }
    let home = tempfile::tempdir().unwrap();
    for filter in [
        "ui::factory::daemon::runtime::shutdown_tests::",
        "test_support::lib_test_process_env_mutation_is_isolated",
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("PATH", "/usr/bin:/bin")
            .env("CAS_C1A2_CLEAN_SUITE", "1")
            .arg(filter)
            .arg("--test-threads=1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "scrubbed {filter}:\n{stdout}\n{stderr}"
        );
        let summary = stdout
            .lines()
            .find(|line| line.starts_with("test result:"))
            .unwrap();
        assert!(
            !summary.contains(" 0 passed;"),
            "no tests executed: {summary}"
        );
        eprintln!("scrubbed {filter}: {summary}");
    }
}

async fn consume_queued_shutdowns(daemon: &mut FactoryDaemon) -> Vec<i64> {
    let cas_dir = daemon.app.cas_dir().to_path_buf();
    let controls: Vec<_> = open_spawn_queue_store(&cas_dir)
        .unwrap()
        .peek(100)
        .unwrap()
        .into_iter()
        .filter(|request| request.action == cas_store::SpawnAction::Shutdown)
        .map(|request| request.id)
        .collect();
    // The real job lanes poll ingress asynchronously: do not mistake a
    // submitted-but-unconsumed shutdown for a surviving replacement.
    let deadline = Instant::now() + Duration::from_secs(10);
    while controls.iter().any(|id| {
        !matches!(
            shutdown_row_state(&cas_dir, *id).0.as_deref(),
            Some("stopped" | "failed")
        )
    }) {
        daemon.enqueue_spawn_requests().unwrap();
        daemon.process_pending_spawns().await;
        assert!(
            Instant::now() < deadline,
            "shutdown controls never consumed: {controls:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    controls
}

async fn long_transcript_recycle(register_replacement: bool) {
    const NAME: &str = "long-history-codex";
    const OLD: &str = "OLD SESSION TRANSCRIPT: prior delivery\r\n";
    const NEW: &str = REPLACEMENT_STARTUP;
    let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    env.set_current_dir(dir.path());

    let mut daemon = daemon(&cas_dir);
    register_idle_worker(&cas_dir, NAME);
    let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
    if register_replacement {
        // The daemon fixture otherwise has no factory session and refuses
        // phantom teardown before it can ever enqueue the shutdown.
        daemon.app.set_factory_session_name_for_test(SESSION);
        let agents = open_agent_store(&cas_dir).unwrap();
        let mut old_registration = agents.get(&format!("agent-{NAME}")).unwrap();
        old_registration.factory_session = Some(SESSION.into());
        agents.update(&old_registration).unwrap();
        let mut held =
            cas_types::Task::new("cas-recycled-held".into(), "Resume after recycle".into());
        held.assignee = Some(NAME.into());
        held.status = cas_types::TaskStatus::InProgress;
        tasks.add(&held).unwrap();
    }
    let old = idle_worker_pane(NAME, cas_mux::SupervisorCli::Codex);
    let old_pid = old.process_group_id().unwrap();
    daemon.app.adopt_worker_pane_for_test(old);
    let old_generation = daemon.app.phantom_worker_target(NAME);
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
    let verification = daemon.spawn_verifications.get(NAME).unwrap_or_else(|| {
        panic!(
            "relaunch awaits registration: {:?}",
            shutdown_row_state(&cas_dir, request_id)
        )
    });
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
        let replacement_pid = daemon.app.mux.pane_process_group_id(NAME).unwrap();
        assert_ne!(replacement_pid, old_pid, "recycle started a new process");
        // The old registration is retired, but the replacement has not
        // registered. A write makes this a changed-DB director snapshot.
        let old_registration = open_agent_store(&cas_dir)
            .unwrap()
            .get(&format!("agent-{NAME}"))
            .unwrap();
        assert_eq!(old_registration.status, cas_types::AgentStatus::Shutdown);
        let mut held = tasks.get("cas-recycled-held").unwrap();
        held.notes = "Replacement is waiting to register".into();
        tasks.update(&held).unwrap();

        // Drive the actual async read -> apply -> store-worker dispatch in
        // lifecycle.rs, rather than the synchronous refresh shortcut.
        daemon.queue_jobs =
            super::prompt_queue_jobs::QueueJobs::start(cas_dir.clone(), daemon.app.project_scope());
        let mut refresh = super::director_refresh::DirectorRefresh::new()
            .with_process_table(|| Box::new(EmptyProcessTable));
        let store_worker = super::store_worker::StoreWorker::start();
        let progress = super::loop_watchdog::LoopProgress::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut due = true;
        loop {
            if daemon
                .director_refresh_pass(&mut refresh, due, &store_worker, &progress)
                .await
            {
                break;
            }
            due = false;
            assert!(
                Instant::now() < deadline,
                "async director snapshot never applied"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            store_worker.wait_idle(Duration::from_secs(5)),
            "phantom dispatch never completed"
        );
        store_worker.drain(Duration::from_secs(5));
        let session = daemon
            .app
            .mux
            .get(NAME)
            .unwrap()
            .harness_session_id()
            .unwrap();
        let mut replacement = cas_types::Agent::new(session, NAME.into());
        replacement.role = cas_types::AgentRole::Worker;
        replacement.factory_session = Some(SESSION.into());
        open_agent_store(&cas_dir)
            .unwrap()
            .register(&replacement)
            .unwrap();
        // A delayed store-worker callback for the retired generation must
        // also remain harmless after same-name registration.
        crate::ui::factory::app::queue_phantom_worker_teardowns(
            &cas_dir,
            SESSION,
            &[old_generation],
        );
        let controls = consume_queued_shutdowns(&mut daemon).await;
        let held = tasks.get("cas-recycled-held").unwrap();
        assert_eq!(
            daemon.app.mux.pane_process_group_id(NAME),
            Some(replacement_pid),
            "retired-session shutdown must preserve replacement; controls={controls:?}, task={:?}/{:?}",
            held.status,
            held.assignee,
        );
        assert!(crate::mcp::daemon::pid_alive(replacement_pid));
        let held = tasks.get("cas-recycled-held").unwrap();
        assert_eq!(held.status, cas_types::TaskStatus::InProgress);
        assert_eq!(held.assignee.as_deref(), Some(NAME));
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
        // A PTY can also echo its startup cursor-position reply. Assert the
        // session boundary independently of that terminal handshake.
        assert!(output.contains(NEW), "replacement output is retained");
        assert!(
            !output.contains(OLD.trim()),
            "a registered replacement must not replay the old session"
        );
        assert!(
            output.len() < 2_000,
            "the short new output is not padded with old history"
        );
        // A teardown of this actual generation must still reap it and
        // release its binding; generation checks are not a blanket bypass.
        let target = daemon.app.phantom_worker_target(NAME);
        let replacement_pid = target.process_group_id.unwrap();
        let agents = open_agent_store(&cas_dir).unwrap();
        let mut retired = agents
            .get(target.harness_session_id.as_deref().unwrap())
            .unwrap();
        retired.status = cas_types::AgentStatus::Shutdown;
        agents.update(&retired).unwrap();
        crate::ui::factory::app::queue_phantom_worker_teardowns(&cas_dir, SESSION, &[target]);
        consume_queued_shutdowns(&mut daemon).await;
        assert!(daemon.app.mux.pane_process_group_id(NAME).is_none());
        assert!(!crate::mcp::daemon::pid_alive(replacement_pid));
        assert_eq!(
            tasks.get("cas-recycled-held").unwrap().status,
            cas_types::TaskStatus::Open,
        );
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
        let notices: Vec<_> = open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(100)
            .unwrap()
            .into_iter()
            .filter(|row| row.target == daemon.app.supervisor_name())
            .map(|row| row.prompt)
            .collect();
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

/// The same stop operation backs shutdown and same-name replacement. Both must
/// drop regenerable scratch output while preserving the checkout and its proof.
#[tokio::test]
async fn shutdown_and_replacement_reclaim_scratch_target_cas_2e6e() {
    let env = crate::test_env_guard::TestEnvGuard::temp_home();
    use cas_types::TaskStatus::{AwaitingMerge, Closed, InProgress};
    for (index, (replacement, status, unpushed, reclaimed)) in [
        (false, Closed, false, true),
        (true, Closed, false, true),
        (true, AwaitingMerge, false, true),
        (true, InProgress, false, false),
        (true, AwaitingMerge, true, false),
        (false, Closed, true, false),
    ]
    .into_iter()
    .enumerate()
    {
        let repo = env.home().join(format!("case-{index}"));
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.join("source"), "keep source").unwrap();
        git(&["add", "source"]);
        git(&["commit", "-qm", "fixture"]);
        let remote = env.home().join(format!("origin-{index}.git"));
        git(&["init", "--bare", "-q", remote.to_str().unwrap()]);
        git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&["push", "-q", "origin", "HEAD"]);
        let cas = crate::store::init_cas_dir(&repo).unwrap();
        let scratch = env.home().join("scratch");
        std::fs::write(
            cas.join("config.toml"),
            format!(
                "[factory]\nscratch_root = {:?}\nartifacts_root = {:?}\n",
                scratch.display().to_string(),
                env.home().join("artifacts").display().to_string()
            ),
        )
        .unwrap();
        let name = "retiring-worker";
        let worker = cas.join("worktrees").join(name);
        std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
        git(&[
            "worktree",
            "add",
            "-qb",
            "factory/retiring-worker",
            worker.to_str().unwrap(),
        ]);
        drop(
            crate::factory_target_cache::owner::acquire(&cas, &worker)
                .unwrap()
                .unwrap(),
        );
        let target = crate::factory_scratch::worker_target_dir(&worker);
        std::fs::create_dir_all(target.join("debug")).unwrap();
        std::fs::write(target.join("debug/output"), "regenerable").unwrap();
        let proof = b"proof survives";
        std::fs::write(target.join("worker-check.log"), proof).unwrap();
        register_idle_worker(&cas, name);
        let agents = open_agent_store(&cas).unwrap();
        let mut agent = agents.get(&format!("agent-{name}")).unwrap();
        agent
            .metadata
            .insert("clone_path".into(), worker.display().to_string());
        agents.update(&agent).unwrap();
        let mut task = cas_types::Task::new("cas-retiring".into(), "Delivered task".into());
        task.assignee = Some(name.into());
        task.status = status;
        crate::store::open_task_store(&cas).unwrap().add(&task).unwrap();
        if unpushed {
            git(&[
                "-C",
                worker.to_str().unwrap(),
                "commit",
                "--allow-empty",
                "-qm",
                "unpublished",
            ]);
        }
        let mut runtime = daemon(&cas);
        runtime
            .app
            .adopt_worker_pane_for_test(idle_worker_pane(name, cas_mux::SupervisorCli::Codex));
        if replacement {
            runtime.app.shutdown_worker_for_recycle(name).await.unwrap();
        } else {
            runtime.app.shutdown_worker(name, false).await.unwrap();
        }
        assert_eq!(
            !target.exists(),
            reclaimed,
            "replacement={replacement}, status={status}, unpushed={unpushed}: {}",
            target.display()
        );
        assert_eq!(
            std::fs::read_to_string(worker.join("source")).unwrap(),
            "keep source"
        );
        if !reclaimed {
            assert_eq!(
                std::fs::read(target.join("worker-check.log")).unwrap(),
                proof
            );
            continue;
        }
        // pin: retirement preserves capped runner proof-log bytes under its durable artifact basename.
        assert!(
            walkdir::WalkDir::new(env.home().join("artifacts"))
                .into_iter()
                .flatten()
                .any(|entry| {
                    // pin: retirement must preserve the capped runner proof log by its artifact basename.
                    entry.file_name() == "worker-check.log"
                        && std::fs::read(entry.path()).unwrap() == proof
                }),
            "proof must be durable before scratch is removed"
        );
    }
}
