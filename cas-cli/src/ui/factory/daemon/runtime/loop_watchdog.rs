//! Daemon loop progress and its watchdog thread (cas-73b5, GH #970).
//!
//! The daemon loop records which step it is in and when it last completed a
//! pass. That costs a few atomic stores per pass. A plain OS thread publishes
//! the record through [`crate::factory_daemon_health`] every few seconds, so
//! `worker_status` still sees the loop when a pass blocks. The thread is not a
//! task on the loop's runtime, which a blocking call inside the loop would
//! also stall.
//!
//! While a pass is wedged and the supervisor has asked for a spawn-queue
//! restart, the watchdog also kills hung git/gh/ssh helper processes the
//! daemon started. Those are the blocking calls a pass can sit in. Harness
//! panes (the supervisor and workers) are never candidates.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::factory_daemon_health::{
    DaemonLoopStatus, LOOP_WEDGED_SECS, WATCHDOG_INTERVAL_SECS, pending_reset, status_path,
    write_status,
};

/// A helper must have run at least this long before a restart may kill it.
const HELPER_MIN_AGE_SECS: u64 = 30;
/// Helper commands the daemon runs in-line that can block a pass.
const HELPER_COMMANDS: &[&str] = &["git", "gh", "ssh", "git-remote-https", "git-remote-http"];
/// How many killed helpers the status file keeps.
const KILLED_HELPERS_KEPT: usize = 10;

/// The loop step running now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum LoopPhase {
    Start = 0,
    ClientInput,
    PtyOutput,
    Relay,
    PromptQueue,
    SpawnQueue,
    PendingSpawns,
    MergeSweep,
    Refresh,
    Render,
    Idle,
}

impl LoopPhase {
    const ALL: [LoopPhase; 11] = [
        LoopPhase::Start,
        LoopPhase::ClientInput,
        LoopPhase::PtyOutput,
        LoopPhase::Relay,
        LoopPhase::PromptQueue,
        LoopPhase::SpawnQueue,
        LoopPhase::PendingSpawns,
        LoopPhase::MergeSweep,
        LoopPhase::Refresh,
        LoopPhase::Render,
        LoopPhase::Idle,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            LoopPhase::Start => "start",
            LoopPhase::ClientInput => "client input",
            LoopPhase::PtyOutput => "pty output",
            LoopPhase::Relay => "cloud relay",
            LoopPhase::PromptQueue => "prompt queue",
            LoopPhase::SpawnQueue => "spawn queue poll",
            LoopPhase::PendingSpawns => "pending spawns",
            LoopPhase::MergeSweep => "merge sweep",
            LoopPhase::Refresh => "refresh",
            LoopPhase::Render => "render",
            LoopPhase::Idle => "idle",
        }
    }

    fn from_u8(value: u8) -> LoopPhase {
        LoopPhase::ALL
            .get(value as usize)
            .copied()
            .unwrap_or(LoopPhase::Start)
    }
}

#[derive(Debug, Default)]
struct SpawnSnapshot {
    pending_spawns: usize,
    in_flight: Option<(String, DateTime<Utc>)>,
    last_reset: Option<String>,
}

/// Progress the loop records and the watchdog publishes.
#[derive(Debug)]
pub(crate) struct LoopProgress {
    last_pass_ms: AtomicI64,
    passes: AtomicU64,
    phase: AtomicU8,
    loop_tid: AtomicI64,
    spawn: Mutex<SpawnSnapshot>,
}

impl LoopProgress {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            last_pass_ms: AtomicI64::new(Utc::now().timestamp_millis()),
            passes: AtomicU64::new(0),
            phase: AtomicU8::new(LoopPhase::Start as u8),
            loop_tid: AtomicI64::new(current_tid()),
            spawn: Mutex::new(SpawnSnapshot::default()),
        })
    }

    pub(crate) fn enter(&self, phase: LoopPhase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    /// Mark a pass complete. The loop's future may move between runtime
    /// threads, so the thread id is refreshed here too.
    pub(crate) fn complete_pass(&self) {
        self.last_pass_ms
            .store(Utc::now().timestamp_millis(), Ordering::Relaxed);
        self.passes.fetch_add(1, Ordering::Relaxed);
        self.loop_tid.store(current_tid(), Ordering::Relaxed);
    }

    pub(crate) fn set_spawn_snapshot(
        &self,
        pending_spawns: usize,
        in_flight: Option<(String, DateTime<Utc>)>,
    ) {
        if let Ok(mut snapshot) = self.spawn.lock() {
            snapshot.pending_spawns = pending_spawns;
            snapshot.in_flight = in_flight;
        }
    }

    pub(crate) fn record_reset(&self, outcome: String) {
        if let Ok(mut snapshot) = self.spawn.lock() {
            snapshot.last_reset = Some(outcome);
        }
    }

    pub(crate) fn status(&self, pid: u32, session: &str, now: DateTime<Utc>) -> DaemonLoopStatus {
        let last_pass_at =
            DateTime::from_timestamp_millis(self.last_pass_ms.load(Ordering::Relaxed))
                .unwrap_or(now);
        let (pending_spawns, in_flight, last_reset) = match self.spawn.lock() {
            Ok(snapshot) => (
                snapshot.pending_spawns,
                snapshot.in_flight.clone(),
                snapshot.last_reset.clone(),
            ),
            Err(_) => (0, None, None),
        };
        DaemonLoopStatus {
            pid,
            session: session.to_string(),
            written_at: now,
            last_pass_at,
            phase: LoopPhase::from_u8(self.phase.load(Ordering::Relaxed))
                .name()
                .to_string(),
            passes: self.passes.load(Ordering::Relaxed),
            pending_spawns,
            in_flight_spawn: in_flight.as_ref().map(|(name, _)| name.clone()),
            in_flight_started_at: in_flight.map(|(_, started)| started),
            loop_thread_wait: None,
            helpers_killed: Vec::new(),
            last_reset,
        }
    }
}

#[cfg(target_os = "linux")]
fn current_tid() -> i64 {
    // SAFETY: gettid has no arguments and cannot fail.
    unsafe { libc::syscall(libc::SYS_gettid) }
}

#[cfg(not(target_os = "linux"))]
fn current_tid() -> i64 {
    0
}

/// The kernel wait channel of one of this process's threads, e.g.
/// `pipe_read` or `futex_wait_queue` (Linux only).
fn thread_wait_channel(tid: i64) -> Option<String> {
    if tid <= 0 {
        return None;
    }
    let wchan = std::fs::read_to_string(format!("/proc/self/task/{tid}/wchan")).ok()?;
    let wchan = wchan.trim();
    (!wchan.is_empty() && wchan != "0").then(|| wchan.to_string())
}

/// `ps` elapsed time, `[[dd-]hh:]mm:ss`, in seconds.
fn parse_etime(value: &str) -> Option<u64> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, value),
    };
    let parts = clock
        .split(':')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * 60 + seconds,
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        _ => return None,
    };
    Some(days * 86_400 + seconds)
}

/// One `ps` row: (pid, ppid, age seconds, command basename).
fn parse_ps_row(line: &str) -> Option<(i32, u32, u64, String)> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let ppid = fields.next()?.parse().ok()?;
    let age = parse_etime(fields.next()?)?;
    let command = fields.collect::<Vec<_>>().join(" ");
    let basename = command.rsplit('/').next()?.to_string();
    Some((pid, ppid, age, basename))
}

/// Children of `parent` that are helper commands older than `min_age_secs`.
fn hung_helpers(ps_output: &str, parent: u32, min_age_secs: u64) -> Vec<(i32, u64, String)> {
    ps_output
        .lines()
        .filter_map(parse_ps_row)
        .filter(|(_, ppid, age, command)| {
            *ppid == parent && *age >= min_age_secs && HELPER_COMMANDS.contains(&command.as_str())
        })
        .map(|(pid, _, age, command)| (pid, age, command))
        .collect()
}

#[cfg(unix)]
fn kill_hung_helpers(parent: u32, min_age_secs: u64) -> Vec<String> {
    let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,etime=,comm="])
        .output()
    else {
        return Vec::new();
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    let mut killed = Vec::new();
    for (pid, age, command) in hung_helpers(&listing, parent, min_age_secs) {
        // SAFETY: plain kill(2) on a pid we just observed as our own child.
        if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
            tracing::warn!(
                pid,
                command = %command,
                age_secs = age,
                "cas-73b5: killed a hung helper to unblock the wedged daemon loop"
            );
            killed.push(format!("{command} (pid {pid}, {age}s)"));
        }
    }
    killed
}

#[cfg(not(unix))]
fn kill_hung_helpers(_parent: u32, _min_age_secs: u64) -> Vec<String> {
    Vec::new()
}

/// Start the watchdog thread. It stops when `shutdown` is set, and removes
/// its status file on the way out so a closed session does not read as a
/// silent daemon.
pub(crate) fn spawn_watchdog(
    progress: Arc<LoopProgress>,
    cas_dir: PathBuf,
    session: String,
    shutdown: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("cas-loop-watchdog".to_string())
        .spawn(move || {
            let pid = std::process::id();
            let mut helpers_killed: Vec<String> = Vec::new();
            while !shutdown.load(Ordering::Relaxed) {
                let now = Utc::now();
                let mut status = progress.status(pid, &session, now);
                let wedged =
                    now.signed_duration_since(status.last_pass_at).num_seconds() > LOOP_WEDGED_SECS;
                if wedged {
                    status.loop_thread_wait =
                        thread_wait_channel(progress.loop_tid.load(Ordering::Relaxed));
                    if pending_reset(&cas_dir, &session).is_some() {
                        helpers_killed.extend(kill_hung_helpers(pid, HELPER_MIN_AGE_SECS));
                        let excess = helpers_killed.len().saturating_sub(KILLED_HELPERS_KEPT);
                        helpers_killed.drain(..excess);
                    }
                }
                status.helpers_killed = helpers_killed.clone();
                if let Err(error) = write_status(&cas_dir, &status) {
                    tracing::debug!(%error, "cas-73b5: could not write the daemon loop status");
                }
                for _ in 0..(WATCHDOG_INTERVAL_SECS * 4) {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            let _ = std::fs::remove_file(status_path(&cas_dir, &session));
        })
        .map_err(|error| {
            tracing::warn!(%error, "cas-73b5: could not start the daemon loop watchdog");
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etime_parses_every_ps_shape() {
        assert_eq!(parse_etime("05"), None);
        assert_eq!(parse_etime("01:05"), Some(65));
        assert_eq!(parse_etime("02:01:05"), Some(7265));
        assert_eq!(parse_etime("3-02:01:05"), Some(3 * 86_400 + 7265));
        assert_eq!(parse_etime("x:05"), None);
    }

    #[test]
    fn only_old_helper_children_of_the_daemon_are_candidates() {
        let listing = "\
            100   1 10:00 /usr/bin/cas\n\
            200 100 05:00 git\n\
            201 100 00:10 git\n\
            202 100 05:00 /usr/bin/gh\n\
            203 100 05:00 claude\n\
            204 999 05:00 git\n\
            205 100 1-00:00:00 ssh\n";
        let candidates = hung_helpers(listing, 100, 30);
        assert_eq!(
            candidates,
            vec![
                (200, 300, "git".to_string()),
                (202, 300, "gh".to_string()),
                (205, 86_400, "ssh".to_string()),
            ],
            "a young helper, a harness pane and another process's child are never killed"
        );
    }

    /// A real hung helper: `git cat-file --batch` blocks on its stdin, the way
    /// a git waiting on a lock or a prompt blocks a daemon pass.
    #[cfg(unix)]
    #[test]
    fn a_hung_git_child_is_killed_and_named() {
        use std::os::unix::process::ExitStatusExt;
        let mut child = Command::new("git")
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("git is installed");
        std::thread::sleep(Duration::from_millis(1_200));
        let killed = kill_hung_helpers(std::process::id(), 1);
        assert!(
            killed
                .iter()
                .any(|entry| entry.starts_with(&format!("git (pid {}", child.id()))),
            "{killed:?}"
        );
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn progress_reports_phase_passes_and_spawn_state() {
        let progress = LoopProgress::new();
        let now = Utc::now();
        progress.enter(LoopPhase::Refresh);
        progress.set_spawn_snapshot(2, Some(("vivid-finch-91".to_string(), now)));
        progress.record_reset("dropped 2".to_string());
        let status = progress.status(7, "cas-src-test", now);
        assert_eq!(status.phase, "refresh");
        assert_eq!(status.passes, 0);
        assert_eq!(status.pending_spawns, 2);
        assert_eq!(status.in_flight_spawn.as_deref(), Some("vivid-finch-91"));
        assert_eq!(status.last_reset.as_deref(), Some("dropped 2"));
        progress.complete_pass();
        assert_eq!(progress.status(7, "cas-src-test", Utc::now()).passes, 1);
    }

    #[test]
    fn the_watchdog_publishes_a_wedged_loop_and_removes_its_file_on_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let progress = LoopProgress::new();
        progress.enter(LoopPhase::Refresh);
        progress.last_pass_ms.store(
            (Utc::now() - chrono::Duration::seconds(600)).timestamp_millis(),
            Ordering::Relaxed,
        );
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_watchdog(
            Arc::clone(&progress),
            temp.path().to_path_buf(),
            "cas-src-test".to_string(),
            Arc::clone(&shutdown),
        )
        .expect("watchdog starts");
        let path = status_path(temp.path(), "cas-src-test");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = crate::factory_daemon_health::read_status(temp.path(), "cas-src-test")
            .expect("status written while the loop is wedged");
        assert_eq!(status.phase, "refresh");
        assert!(
            Utc::now()
                .signed_duration_since(status.last_pass_at)
                .num_seconds()
                >= 600
        );
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert!(!path.exists());
    }
}
