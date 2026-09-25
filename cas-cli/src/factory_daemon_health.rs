//! Factory daemon loop health, visible outside the daemon (cas-73b5, GH #970).
//!
//! # Why a file written by a separate thread
//!
//! The factory daemon runs every duty in one sequential loop: client input,
//! PTY output, the prompt queue, the spawn queue, merge sweeps, refreshes. Its
//! own stall scan runs inside that loop. When a pass blocked, spawn and
//! shutdown requests sat unprocessed for 30+ minutes and nothing reported it,
//! because the reporter was stuck too.
//!
//! So the loop only records progress in memory, and a plain OS thread (not a
//! task on the loop's runtime) writes that progress to
//! `.cas/factory-daemon/<session>.loop.json` every few seconds. `worker_status`
//! runs in `cas serve`, a different process, and reads the file next to the
//! oldest pending `spawn_queue` row. A wedged loop is then visible even while
//! the daemon can do nothing about it.
//!
//! Recovery goes through the same directory. `restart_spawn_queue` drops a
//! reset request. The loop applies it on its next pass. If the pass itself is
//! blocked, the watchdog thread kills hung git/gh/ssh helper processes the
//! daemon started, so the pass can finish. The supervisor's pane is never
//! touched.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A loop pass older than this is reported as wedged.
pub const LOOP_WEDGED_SECS: i64 = 60;
/// A spawn/shutdown request pending longer than this is reported as stalled.
pub const SPAWN_REQUEST_STALL_SECS: i64 = 60;
/// An in-flight spawn provisioning longer than this is called out.
pub const IN_FLIGHT_SPAWN_SLOW_SECS: i64 = 120;
/// How often the watchdog rewrites the status file.
pub const WATCHDOG_INTERVAL_SECS: u64 = 5;
/// A status file not rewritten for this long means the daemon itself is gone.
pub const STATUS_STALE_SECS: i64 = 30;

/// One snapshot of the daemon loop, written by its watchdog thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonLoopStatus {
    pub pid: u32,
    pub session: String,
    /// When the watchdog wrote this snapshot.
    pub written_at: DateTime<Utc>,
    /// When the loop last completed a full pass.
    pub last_pass_at: DateTime<Utc>,
    /// The loop step running now (or last entered).
    pub phase: String,
    pub passes: u64,
    /// Spawn/shutdown actions dequeued from `spawn_queue` but not yet run.
    pub pending_spawns: usize,
    /// Worker whose worktree provisioning is in flight, and since when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight_spawn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight_started_at: Option<DateTime<Utc>>,
    /// Kernel wait channel of the loop's thread (Linux), e.g. `pipe_read`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_thread_wait: Option<String>,
    /// Helper processes the watchdog killed to unblock a wedged pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helpers_killed: Vec<String>,
    /// Outcome of the most recent spawn-queue reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reset: Option<String>,
}

/// A supervisor's request to restart the spawn queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnQueueResetRequest {
    pub requested_at: DateTime<Utc>,
    pub requester: String,
}

/// The oldest spawn/shutdown request still waiting to be dequeued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSpawnRequest {
    pub id: i64,
    pub action: String,
    pub created_at: DateTime<Utc>,
}

fn health_dir(cas_dir: &Path) -> PathBuf {
    cas_dir.join("factory-daemon")
}

/// Session names become file names; keep them to a safe alphabet.
fn file_stem(session: &str) -> String {
    session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn status_path(cas_dir: &Path, session: &str) -> PathBuf {
    health_dir(cas_dir).join(format!("{}.loop.json", file_stem(session)))
}

pub fn reset_request_path(cas_dir: &Path, session: &str) -> PathBuf {
    health_dir(cas_dir).join(format!("{}.reset.json", file_stem(session)))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

pub fn write_status(cas_dir: &Path, status: &DaemonLoopStatus) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(status).map_err(std::io::Error::other)?;
    write_atomic(&status_path(cas_dir, &status.session), &bytes)
}

pub fn read_status(cas_dir: &Path, session: &str) -> Option<DaemonLoopStatus> {
    let bytes = std::fs::read(status_path(cas_dir, session)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Record a reset request for the daemon of `session`. A newer request
/// replaces an unapplied older one.
pub fn request_reset(cas_dir: &Path, session: &str, requester: &str) -> std::io::Result<()> {
    let request = SpawnQueueResetRequest {
        requested_at: Utc::now(),
        requester: requester.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&request).map_err(std::io::Error::other)?;
    write_atomic(&reset_request_path(cas_dir, session), &bytes)
}

/// The pending reset request, without consuming it.
pub fn pending_reset(cas_dir: &Path, session: &str) -> Option<SpawnQueueResetRequest> {
    let bytes = std::fs::read(reset_request_path(cas_dir, session)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Consume the pending reset request. An unreadable file is removed too, so a
/// corrupt request cannot re-trigger on every pass.
pub fn take_reset(cas_dir: &Path, session: &str) -> Option<SpawnQueueResetRequest> {
    let path = reset_request_path(cas_dir, session);
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes)
        .ok()
        .or(Some(SpawnQueueResetRequest {
            requested_at: Utc::now(),
            requester: "unreadable request".to_string(),
        }))
}

/// The recovery instruction every warning ends with.
pub const RESTART_HINT: &str = "Recover without restarting the session: \
     `factory action=restart_spawn_queue`.";

/// Warnings for `worker_status`, or `None` when the spawn queue is healthy.
///
/// `pending` is this session's `spawn_queue` rows still waiting to be
/// dequeued, oldest first. `status` is the daemon's latest loop snapshot.
pub fn spawn_queue_health_warnings(
    now: DateTime<Utc>,
    pending: &[PendingSpawnRequest],
    status: Option<&DaemonLoopStatus>,
) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(status) = status {
        let written_age = now.signed_duration_since(status.written_at).num_seconds();
        let pass_age = now.signed_duration_since(status.last_pass_at).num_seconds();
        if written_age > STATUS_STALE_SECS {
            lines.push(format!(
                "⚠ FACTORY DAEMON SILENT: its loop watchdog (pid {}) last reported {}s ago; \
                 the daemon may have exited.",
                status.pid, written_age
            ));
        } else if pass_age > LOOP_WEDGED_SECS {
            let wait = status
                .loop_thread_wait
                .as_deref()
                .map(|wait| format!(", thread waiting in {wait}"))
                .unwrap_or_default();
            lines.push(format!(
                "⚠ FACTORY DAEMON LOOP WEDGED: no completed pass for {}s; stuck in phase \
                 '{}' (pid {}{}). Spawn, shutdown and message delivery are not being processed.",
                pass_age, status.phase, status.pid, wait
            ));
        }
        if status.pending_spawns > 0 && pass_age > LOOP_WEDGED_SECS {
            lines.push(format!(
                "  {} dequeued spawn/shutdown action(s) are waiting inside the daemon.",
                status.pending_spawns
            ));
        }
        if let (Some(worker), Some(started)) = (
            status.in_flight_spawn.as_deref(),
            status.in_flight_started_at,
        ) {
            let age = now.signed_duration_since(started).num_seconds();
            if age > IN_FLIGHT_SPAWN_SLOW_SECS {
                lines.push(format!(
                    "⚠ SPAWN IN FLIGHT FOR {age}s: worktree provisioning for '{worker}' has not \
                     finished; queued spawns wait behind it."
                ));
            }
        }
        if !status.helpers_killed.is_empty() {
            lines.push(format!(
                "  Watchdog killed hung helper process(es) to unblock the loop: {}.",
                status.helpers_killed.join(", ")
            ));
        }
    }
    if let Some(oldest) = pending.first() {
        let age = now.signed_duration_since(oldest.created_at).num_seconds();
        if age > SPAWN_REQUEST_STALL_SECS {
            lines.push(format!(
                "⚠ SPAWN QUEUE STALLED: {} request(s) not dequeued; oldest #{} ({}) queued {}s ago.",
                pending.len(),
                oldest.id,
                oldest.action,
                age
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.push(format!("  {RESTART_HINT}"));
    if let Some(reset) = status.and_then(|status| status.last_reset.as_deref()) {
        lines.push(format!("  Last reset: {reset}"));
    }
    Some(lines.join("\n"))
}

/// This session's pending spawn-queue rows, oldest first. Rows scoped to
/// another session belong to another daemon and are left out; unscoped legacy
/// rows can be drained by any daemon, so they count.
pub fn pending_requests_for_session(cas_dir: &Path, session: &str) -> Vec<PendingSpawnRequest> {
    let Ok(queue) = crate::store::open_spawn_queue_store(cas_dir) else {
        return Vec::new();
    };
    queue
        .peek(50)
        .unwrap_or_default()
        .into_iter()
        .filter(|request| {
            request
                .factory_session
                .as_deref()
                .is_none_or(|owner| owner == session)
        })
        .map(|request| PendingSpawnRequest {
            id: request.id,
            action: request.action.as_str().to_string(),
            created_at: request.created_at,
        })
        .collect()
}

/// The `worker_status` section for `session`, or empty when healthy.
pub fn worker_status_section(cas_dir: &Path, session: &str) -> String {
    let pending = pending_requests_for_session(cas_dir, session);
    let status = read_status(cas_dir, session);
    spawn_queue_health_warnings(Utc::now(), &pending, status.as_ref())
        .map(|warnings| format!("{warnings}\n\n"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(now: DateTime<Utc>, pass_age: i64) -> DaemonLoopStatus {
        DaemonLoopStatus {
            pid: 4242,
            session: "cas-src-test".to_string(),
            written_at: now,
            last_pass_at: now - chrono::Duration::seconds(pass_age),
            phase: "refresh".to_string(),
            passes: 10,
            pending_spawns: 0,
            in_flight_spawn: None,
            in_flight_started_at: None,
            loop_thread_wait: None,
            helpers_killed: Vec::new(),
            last_reset: None,
        }
    }

    #[test]
    fn a_healthy_loop_and_a_fresh_queue_add_nothing() {
        let now = Utc::now();
        let pending = [PendingSpawnRequest {
            id: 1,
            action: "spawn".into(),
            created_at: now - chrono::Duration::seconds(2),
        }];
        assert_eq!(
            spawn_queue_health_warnings(now, &pending, Some(&status(now, 1))),
            None
        );
        assert_eq!(spawn_queue_health_warnings(now, &[], None), None);
    }

    #[test]
    fn a_wedged_loop_names_its_phase_the_oldest_request_and_the_recovery() {
        let now = Utc::now();
        let mut wedged = status(now, 1_900);
        wedged.pending_spawns = 2;
        wedged.loop_thread_wait = Some("pipe_read".into());
        let pending = [
            PendingSpawnRequest {
                id: 2204,
                action: "shutdown".into(),
                created_at: now - chrono::Duration::seconds(1_850),
            },
            PendingSpawnRequest {
                id: 2205,
                action: "spawn".into(),
                created_at: now - chrono::Duration::seconds(30),
            },
        ];
        let text = spawn_queue_health_warnings(now, &pending, Some(&wedged)).unwrap();
        assert!(
            text.contains("LOOP WEDGED: no completed pass for 1900s"),
            "{text}"
        );
        assert!(
            text.contains("phase 'refresh' (pid 4242, thread waiting in pipe_read)"),
            "{text}"
        );
        assert!(
            text.contains("2 dequeued spawn/shutdown action(s)"),
            "{text}"
        );
        assert!(
            text.contains("2 request(s) not dequeued; oldest #2204 (shutdown) queued 1850s ago"),
            "{text}"
        );
        assert!(text.contains("restart_spawn_queue"), "{text}");
    }

    #[test]
    fn a_silent_daemon_and_a_slow_spawn_are_reported() {
        let now = Utc::now();
        let mut silent = status(now, 5);
        silent.written_at = now - chrono::Duration::seconds(300);
        let text = spawn_queue_health_warnings(now, &[], Some(&silent)).unwrap();
        assert!(text.contains("FACTORY DAEMON SILENT"), "{text}");

        let mut slow = status(now, 1);
        slow.in_flight_spawn = Some("vivid-finch-91".into());
        slow.in_flight_started_at = Some(now - chrono::Duration::seconds(400));
        let text = spawn_queue_health_warnings(now, &[], Some(&slow)).unwrap();
        assert!(text.contains("SPAWN IN FLIGHT FOR 400s"), "{text}");
    }

    #[test]
    fn status_and_reset_requests_round_trip_through_the_session_directory() {
        let temp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let snapshot = status(now, 1);
        write_status(temp.path(), &snapshot).unwrap();
        assert_eq!(read_status(temp.path(), "cas-src-test"), Some(snapshot));

        assert!(take_reset(temp.path(), "cas-src-test").is_none());
        request_reset(temp.path(), "cas-src-test", "supervisor-1").unwrap();
        assert_eq!(
            pending_reset(temp.path(), "cas-src-test")
                .unwrap()
                .requester,
            "supervisor-1"
        );
        assert_eq!(
            take_reset(temp.path(), "cas-src-test").unwrap().requester,
            "supervisor-1"
        );
        assert!(pending_reset(temp.path(), "cas-src-test").is_none());
    }
}
