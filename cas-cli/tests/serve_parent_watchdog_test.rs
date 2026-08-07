//! `cas serve` must not outlive the harness that spawned it (cas-82d6c).
//!
//! The incident: four `cas serve` processes survived 36+ hours after their
//! harness died, holding write-side fds on the shared project `cas.db` (plus
//! `-wal`/`-shm`). Stdin EOF did not save them, because EOF only fires when the
//! *last* writer to stdin closes — and in the real topologies (a pty held by
//! tmux, or a sibling process that inherited the pipe write end) that writer
//! outlives the harness.
//!
//! These tests reproduce exactly that shape: the server's stdin is a FIFO whose
//! write end this test process holds open for the whole run, so EOF can never
//! fire. The only thing that can end the server is the parent-death watchdog.
//!
//! The reproduction is Linux-only because it depends on `/proc` re-parenting
//! semantics under `systemd --user` — the environment the incident happened in.
//! The "a live parent is never killed" guarantee is asserted here too, since
//! that is the property that makes self-reaping safe to ship.

#![cfg(target_os = "linux")]

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod support;
use support::CasSandbox;

/// Poll fast so the tests do not pay the production 5s interval.
const TEST_POLL_MS: &str = "150";

/// Generous relative to `TEST_POLL_MS` * confirmations; short enough that a
/// regression fails the suite instead of hanging it.
const EXIT_DEADLINE: Duration = Duration::from_secs(45);

/// A `cas serve` running under a killable intermediate parent, with a stdin
/// that never reaches EOF.
struct OrphanRig {
    /// The intermediate process that spawned `cas serve` — the stand-in for a
    /// harness we are going to kill.
    parent: Child,
    /// pid of the `cas serve` process itself.
    server_pid: u32,
    /// The FIFO write end. Holding this for the lifetime of the rig is the
    /// whole point: it models the pty/leaked-pipe topology in which stdin EOF
    /// never fires, so the test cannot pass by accident.
    _stdin_writer: File,
    log_path: std::path::PathBuf,
}

impl OrphanRig {
    fn launch(sandbox: &CasSandbox) -> Self {
        let fifo = sandbox.path().join("serve-stdin.fifo");
        let log_path = sandbox.path().join("serve.log");
        let pid_path = sandbox.path().join("serve.pid");

        let status = Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed for {}", fifo.display());

        // Open read+write so this never blocks waiting for a peer, and so a
        // writer exists before the server opens the read side.
        let stdin_writer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .expect("hold the FIFO write end open");

        let script = format!(
            "{bin} serve < {fifo} > /dev/null 2> {log} & echo $! > {pid}; sleep 600",
            bin = shell_quote(env!("CARGO_BIN_EXE_cas")),
            fifo = shell_quote(fifo.to_str().expect("utf-8 fifo path")),
            log = shell_quote(log_path.to_str().expect("utf-8 log path")),
            pid = shell_quote(pid_path.to_str().expect("utf-8 pid path")),
        );

        let mut cmd = Command::new("sh");
        sandbox.configure_command(&mut cmd);
        // Set the watchdog knobs *after* `configure_command`, which scrubs
        // every inherited `CAS_*` key.
        let parent = cmd
            .arg("-c")
            .arg(script)
            .env("CAS_SERVE_PARENT_WATCHDOG_INTERVAL_MS", TEST_POLL_MS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn intermediate parent");

        let server_pid = wait_for_pid_file(&pid_path);
        wait_for_serve_ready(&log_path, server_pid);

        Self {
            parent,
            server_pid,
            _stdin_writer: stdin_writer,
            log_path,
        }
    }

    fn server_is_alive(&self) -> bool {
        pid_is_alive(self.server_pid)
    }

    fn kill_parent(&mut self) {
        let _ = self.parent.kill();
        let _ = self.parent.wait();
    }

    fn serve_log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for OrphanRig {
    fn drop(&mut self) {
        let _ = self.parent.kill();
        let _ = self.parent.wait();
        if pid_is_alive(self.server_pid) {
            let _ = Command::new("kill")
                .args(["-9", &self.server_pid.to_string()])
                .status();
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn pid_is_alive(pid: u32) -> bool {
    // A zombie is not "alive" for our purposes — it holds no file descriptors.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rfind(')')
            .and_then(|close| stat.get(close + 1..))
            .and_then(|rest| rest.split_whitespace().next())
            .is_none_or(|state| state != "Z"),
        Err(_) => false,
    }
}

fn wait_for_pid_file(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(pid) = raw.trim().parse::<u32>() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("cas serve never recorded its pid at {}", path.display());
}

/// Wait until the server has finished startup, so a later exit is attributable
/// to the watchdog rather than to a startup failure.
fn wait_for_serve_ready(log_path: &Path, server_pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let mut log = String::new();
        if let Ok(mut file) = File::open(log_path) {
            let _ = file.read_to_string(&mut log);
        }
        if log.contains("Starting MCP server") {
            return;
        }
        assert!(
            pid_is_alive(server_pid),
            "cas serve died during startup; log:\n{log}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("cas serve never reported readiness");
}

/// AC1/AC3: a `cas serve` whose parent harness dies terminates on its own
/// within a bounded interval, in the topology where stdin EOF cannot save it.
#[test]
fn serve_exits_after_its_parent_harness_is_killed() {
    let sandbox = CasSandbox::new();
    let mut rig = OrphanRig::launch(&sandbox);

    // Control: with the parent alive, the server keeps running across many
    // watchdog polls. This is the "an idle client is not a dead one" property —
    // if the watchdog were trigger-happy, it would already have fired.
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        rig.server_is_alive(),
        "watchdog killed a server whose parent was still alive; log:\n{}",
        rig.serve_log()
    );

    rig.kill_parent();

    let deadline = Instant::now() + EXIT_DEADLINE;
    while Instant::now() < deadline {
        if !rig.server_is_alive() {
            let log = rig.serve_log();
            assert!(
                log.contains("Parent harness is gone"),
                "server exited but not via the watchdog; log:\n{log}"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    panic!(
        "cas serve survived its parent by {EXIT_DEADLINE:?} — the orphan leak is back; log:\n{}",
        rig.serve_log()
    );
}

/// AC2/AC5: the watchdog must never end a session that still has an owner, and
/// reaping is self-scoped — a second, unrelated `cas serve` in its own project
/// is untouched while the first one orphans and exits.
#[test]
fn a_second_projects_live_server_is_untouched_when_another_orphans() {
    let orphan_sandbox = CasSandbox::new();
    let bystander_sandbox = CasSandbox::new();

    let mut orphan = OrphanRig::launch(&orphan_sandbox);
    let bystander = OrphanRig::launch(&bystander_sandbox);

    orphan.kill_parent();

    let deadline = Instant::now() + EXIT_DEADLINE;
    let mut orphan_exited = false;
    while Instant::now() < deadline {
        if !orphan.server_is_alive() {
            orphan_exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        orphan_exited,
        "orphaned server did not exit; log:\n{}",
        orphan.serve_log()
    );

    assert!(
        bystander.server_is_alive(),
        "reaping one project's orphan took down another project's live server; log:\n{}",
        bystander.serve_log()
    );
}
