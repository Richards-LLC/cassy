//! Parent-death watchdog for `cas serve` (cas-82d6c).
//!
//! # The leak
//!
//! `cas serve` is an MCP **stdio** server: it is always spawned as a child of a
//! harness (Claude Code, Codex, an editor) and has no reason to exist once that
//! harness is gone. The transport already handles the polite case — when the
//! last writer to our stdin closes, rmcp's read loop sees EOF and
//! `RunningService::waiting()` returns. Measured on the development host
//! (cas 2.49.0): a `cas serve` spawned with `stdin=PIPE` whose parent was
//! `SIGKILL`ed exited on its own in 6 seconds.
//!
//! That is exactly why the observed leak is *not* an EOF-handling bug. Four
//! `cas serve` processes survived 36+ hours with a dead parent while holding
//! write-side fds on the shared project `cas.db` (`cas.db`, `-wal`, `-shm`).
//! Their stdin never reached EOF, which means the write end outlived the
//! harness. Two real topologies do that, and neither is under our control:
//!
//! - **stdin is a pty.** A worker running inside tmux hands its child a pty
//!   slave; the pty master is held by tmux, not the harness, so killing the
//!   harness closes nothing.
//! - **A sibling inherited the pipe.** Any process the harness spawned *after*
//!   creating our stdin pipe, without `CLOEXEC` on the write end, keeps that
//!   end open for its own lifetime.
//!
//! So the fix cannot be built on stdin at all. Parenthood is the one signal
//! that is true in every topology: a stdio MCP server whose parent process is
//! gone has no possible client, whatever its file descriptors say.
//!
//! # Why the server reaps itself
//!
//! `ui/factory/orphan_gc.rs` already reaps orphans, and it did not fire for the
//! observed pids. It has exactly two candidate classes: processes whose **cwd
//! is under `<cas_root>/worktrees`**, and processes recorded in the
//! `server_start` registry. An MCP `cas serve` for a main checkout has cwd =
//! the project root, so it fails the first; it is never registered via
//! `server_start`, so it fails the second. And even where a worker's `cas
//! serve` does sit in a worktree, `orphan_gc` only runs when a supervisor
//! manually invokes `gc_cleanup` behind a `force=true` + `dry_run=false` double
//! gate — it is an operator tool, not a periodic reaper.
//!
//! Adding a *second* external reaper would inherit the hard part of the first:
//! deciding from the outside whether some other project's `cas serve` is still
//! wanted. Self-reaping has no such problem. Each process only ever evaluates
//! its own parent and only ever exits itself, so a `cas serve` in project A can
//! never affect one in project B on the same host — cross-project safety is
//! structural, not a rule someone has to keep enforcing.
//!
//! # What counts as proof the parent is gone
//!
//! Two independent facts, both required, then confirmed twice:
//!
//! 1. **Our ppid changed** from the value captured at startup. A process cannot
//!    be re-parented while its parent lives, so this alone proves the original
//!    parent exited. It is also immune to pid reuse: a recycled pid cannot
//!    become our parent.
//! 2. **The current parent is init-like** — [`ParentState::Reaped`] or
//!    [`ParentState::Gone`] per `orphan_gc`'s classifier, which deliberately
//!    treats "adopted by `systemd --user`" as reaped because `ppid == 1` is
//!    wrong on any host with a `child_subreaper` (this one included).
//!
//! Requiring both keeps a legitimately re-parented but still-owned process
//! alive, and [`CONFIRMATIONS`] consecutive observations keep a single failed
//! `/proc` read from ending a healthy session. An **idle** client is untouched
//! by construction: idleness never changes a ppid.
//!
//! If the parent is already init-like at startup we disable the watchdog and
//! say so on stderr. That shape means someone deliberately daemonized us
//! (`nohup`, a service manager, a double-forking launcher), and we have no
//! evidence to distinguish it from an orphan.
//!
//! # Bounded interval
//!
//! Detection to exit is at most `POLL_INTERVAL * (CONFIRMATIONS + 1)` — 15s at
//! the defaults — plus [`GRACEFUL_SHUTDOWN_GRACE`] for the cooperative
//! shutdown to release agent tasks and stop the daemon. After that grace the
//! process exits hard rather than trusting a shutdown path that may itself be
//! wedged on the very database lock this bug creates.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

use crate::ui::factory::orphan_gc::{ParentState, parent_state};

/// How often the parent is checked. Small enough that an orphan never spans a
/// meaningful fraction of a session, large enough that the cost (two `/proc`
/// stat reads) is irrelevant.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Consecutive orphan observations required before acting. One transient
/// `/proc` read failure must never end a live session.
const CONFIRMATIONS: u32 = 2;

/// How long the cooperative shutdown gets before the process exits hard.
/// Generous relative to "release tasks + stop daemon", short relative to the
/// hours an orphan otherwise squats the database.
const GRACEFUL_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Set to `0`/`false`/`off` to disable the watchdog entirely.
const ENV_ENABLED: &str = "CAS_SERVE_PARENT_WATCHDOG";

/// Poll interval override in milliseconds. Exists so tests do not have to wait
/// out the production interval.
const ENV_INTERVAL_MS: &str = "CAS_SERVE_PARENT_WATCHDOG_INTERVAL_MS";

/// What a single poll observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    /// Someone still owns this process.
    Owned,
    /// The original parent is provably gone.
    Orphaned,
}

/// Decide from one poll whether this process still has an owner.
///
/// Pure so the whole matrix is testable without killing real processes.
/// `original_ppid` is the ppid captured at startup, `current_ppid` the ppid
/// read now, `current_parent` the classification of `current_ppid`.
pub(crate) fn observe(
    original_ppid: u32,
    current_ppid: u32,
    current_parent: ParentState,
) -> Observation {
    if current_ppid == original_ppid {
        // Not re-parented: the process that spawned us is still alive. This is
        // the branch that protects a healthy long-lived session, however idle.
        return Observation::Owned;
    }
    match current_parent {
        // Re-parented onto a real, live process — a wrapper we do not model,
        // not an orphan. Refuse to act without both facts.
        ParentState::Alive => Observation::Owned,
        ParentState::Reaped | ParentState::Gone => Observation::Orphaned,
    }
}

/// Whether the watchdog may arm at all, given the parent state at startup.
///
/// Returns `false` when we were born already adopted: deliberate daemonization
/// is indistinguishable from orphanhood at that point, and killing a service
/// someone started on purpose is worse than the leak.
pub(crate) fn may_arm(startup_parent: ParentState) -> bool {
    startup_parent == ParentState::Alive
}

/// Handle the serve loop uses to learn that the parent died.
#[derive(Clone)]
pub(crate) struct ParentWatchdog {
    tripped: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ParentWatchdog {
    /// A watchdog that never trips (disabled, or unsupported platform state).
    fn inert() -> Self {
        Self {
            tripped: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Resolve once the parent is confirmed gone. Never resolves otherwise.
    pub(crate) async fn tripped(&self) {
        loop {
            if self.tripped.load(Ordering::SeqCst) {
                return;
            }
            self.notify.notified().await;
        }
    }
}

fn enabled_by_env() -> bool {
    match std::env::var(ENV_ENABLED) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

fn poll_interval() -> Duration {
    std::env::var(ENV_INTERVAL_MS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(POLL_INTERVAL)
}

/// Start the watchdog. Returns a handle whose [`ParentWatchdog::tripped`]
/// future resolves when this process is confirmed orphaned.
///
/// Called as early as possible in `cas serve` so a startup that wedges before
/// the transport exists is covered too.
pub(crate) fn spawn() -> ParentWatchdog {
    if !enabled_by_env() {
        eprintln!("[CAS] Parent watchdog disabled via {ENV_ENABLED}");
        return ParentWatchdog::inert();
    }

    let original_ppid = std::os::unix::process::parent_id();
    let startup_parent = parent_state(original_ppid);
    if !may_arm(startup_parent) {
        eprintln!(
            "[CAS] Parent watchdog not armed: already adopted at startup (ppid {original_ppid}) — \
             treating this as a deliberately detached server"
        );
        return ParentWatchdog::inert();
    }

    let watchdog = ParentWatchdog::inert();
    let tripped = Arc::clone(&watchdog.tripped);
    let notify = Arc::clone(&watchdog.notify);
    let interval = poll_interval();

    tokio::spawn(async move {
        let mut consecutive = 0u32;
        loop {
            tokio::time::sleep(interval).await;
            let current_ppid = std::os::unix::process::parent_id();
            let observation = observe(original_ppid, current_ppid, parent_state(current_ppid));
            match observation {
                Observation::Owned => consecutive = 0,
                Observation::Orphaned => consecutive += 1,
            }
            if consecutive < CONFIRMATIONS {
                continue;
            }

            eprintln!(
                "[CAS] Parent harness is gone (ppid {original_ppid} -> {current_ppid}); \
                 shutting down so this server stops holding the project database."
            );
            tripped.store(true, Ordering::SeqCst);
            notify.notify_waiters();

            // The cooperative path releases agent tasks and stops the daemon.
            // If it cannot finish — the likeliest reason being the database
            // contention this bug causes — exiting hard is still strictly
            // better than squatting write-side fds for another 36 hours.
            tokio::time::sleep(GRACEFUL_SHUTDOWN_GRACE).await;
            eprintln!("[CAS] Graceful shutdown did not complete in time; exiting.");
            std::process::exit(0);
        }
    });

    watchdog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_ppid_is_always_owned() {
        // The healthy-session guarantee: an idle client changes nothing, so no
        // amount of idleness can be read as orphanhood.
        for parent in [ParentState::Alive, ParentState::Reaped, ParentState::Gone] {
            assert_eq!(observe(4242, 4242, parent), Observation::Owned);
        }
    }

    #[test]
    fn reparenting_onto_a_live_process_is_not_enough() {
        // Both facts are required; a wrapper topology we do not model must not
        // be mistaken for a dead harness.
        assert_eq!(observe(4242, 99, ParentState::Alive), Observation::Owned);
    }

    #[test]
    fn reparenting_onto_an_adopter_is_an_orphan() {
        // `Reaped` is the systemd --user case that a `ppid == 1` rule misses.
        assert_eq!(
            observe(4242, 3204, ParentState::Reaped),
            Observation::Orphaned
        );
        assert_eq!(observe(4242, 1, ParentState::Gone), Observation::Orphaned);
    }

    #[test]
    fn a_server_born_adopted_never_arms() {
        assert!(may_arm(ParentState::Alive));
        assert!(!may_arm(ParentState::Reaped));
        assert!(!may_arm(ParentState::Gone));
    }

    #[tokio::test]
    async fn an_inert_watchdog_never_trips() {
        let watchdog = ParentWatchdog::inert();
        let outcome = tokio::time::timeout(Duration::from_millis(50), watchdog.tripped()).await;
        assert!(outcome.is_err(), "inert watchdog must never resolve");
    }
}
