//! Durable ownership records for factory worker process groups.
//!
//! Worker panes are spawned through `portable_pty`, which calls `setsid(2)` in
//! the child. The pane child is therefore both the session leader and process
//! group leader. Recording that PGID lets exact factory teardown paths kill
//! descendants which have outlived the interactive harness, and lets GC report
//! groups left behind by an abruptly-dead factory daemon.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROCESS_GROUP_DIR: &str = "factory-process-groups";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TrackedProcessGroup {
    pub worker_name: String,
    pub factory_session: String,
    pub pgid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_starttime: Option<u64>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReapOutcome {
    Reaped,
    AlreadyGone,
    FingerprintMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupIdentity {
    Original,
    Gone,
    FingerprintMismatch,
    Unverifiable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessGroupStatus {
    Live,
    Gone,
    FingerprintMismatch,
    Unverifiable,
}

fn registry_dir(cas_root: &Path) -> PathBuf {
    cas_root.join(PROCESS_GROUP_DIR)
}

fn record_path(cas_root: &Path, pgid: u32) -> PathBuf {
    registry_dir(cas_root).join(format!("{pgid}.json"))
}

/// Persist ownership immediately after a worker pane is spawned.
pub(crate) fn track(
    cas_root: &Path,
    worker_name: &str,
    factory_session: &str,
    pgid: u32,
) -> io::Result<TrackedProcessGroup> {
    let record = TrackedProcessGroup {
        worker_name: worker_name.to_string(),
        factory_session: factory_session.to_string(),
        pgid,
        pid_starttime: crate::mcp::daemon::read_pid_starttime(pgid),
        recorded_at: Utc::now(),
    };
    let dir = registry_dir(cas_root);
    fs::create_dir_all(&dir)?;
    let path = record_path(cas_root, pgid);
    let tmp = dir.join(format!(".{pgid}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(record)
}

pub(crate) fn untrack(cas_root: &Path, pgid: u32) -> io::Result<()> {
    let path = record_path(cas_root, pgid);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn list(cas_root: &Path) -> io::Result<Vec<TrackedProcessGroup>> {
    let dir = registry_dir(cas_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<TrackedProcessGroup>(&bytes) else {
            continue;
        };
        records.push(record);
    }
    records.sort_by_key(|record| record.pgid);
    Ok(records)
}

/// True only while the original process group still owns this PGID.
///
/// The `/proc` starttime check prevents a stale record from targeting an
/// unrelated process after PID reuse. If the original leader has already been
/// reaped but descendants remain, Linux keeps the PGID allocated to that
/// leaderless group; a `/proc` pgrp scan recognizes that safe-to-reap state.
pub(crate) fn is_live(record: &TrackedProcessGroup) -> bool {
    group_identity(record) == GroupIdentity::Original
}

pub(crate) fn status(record: &TrackedProcessGroup) -> ProcessGroupStatus {
    match group_identity(record) {
        GroupIdentity::Original => ProcessGroupStatus::Live,
        GroupIdentity::Gone => ProcessGroupStatus::Gone,
        GroupIdentity::FingerprintMismatch => ProcessGroupStatus::FingerprintMismatch,
        GroupIdentity::Unverifiable => ProcessGroupStatus::Unverifiable,
    }
}

fn group_identity(record: &TrackedProcessGroup) -> GroupIdentity {
    #[cfg(target_os = "linux")]
    {
        let Some(expected) = record.pid_starttime else {
            return GroupIdentity::Unverifiable;
        };
        return match crate::mcp::daemon::read_pid_starttime(record.pgid) {
            Some(actual) if actual == expected => {
                let leader_is_zombie = process_state_and_group(record.pgid)
                    .is_some_and(|(state, _)| state == 'Z');
                if leader_is_zombie && !process_group_has_live_members(record.pgid) {
                    GroupIdentity::Gone
                } else {
                    GroupIdentity::Original
                }
            }
            Some(_) => GroupIdentity::FingerprintMismatch,
            None if process_group_has_live_members(record.pgid) => GroupIdentity::Original,
            None => GroupIdentity::Gone,
        };
    }

    #[cfg(target_os = "macos")]
    {
        let Some(expected) = record.pid_starttime else {
            return GroupIdentity::Unverifiable;
        };
        let Some(actual_starttime) = crate::mcp::daemon::read_pid_starttime(record.pgid) else {
            return if crate::mcp::daemon::pid_alive(record.pgid) {
                GroupIdentity::Unverifiable
            } else if macos_process_group_has_members(record.pgid) {
                // As on Linux, a leaderless group cannot acquire a new leader
                // with the same PGID while those original members remain.
                GroupIdentity::Original
            } else {
                GroupIdentity::Gone
            };
        };
        if actual_starttime != expected {
            return GroupIdentity::FingerprintMismatch;
        }
        // SAFETY: getpgid is a read-only process-table query. Both the start
        // timestamp and process-group relationship must still match.
        let actual_pgid = unsafe { libc::getpgid(record.pgid as libc::pid_t) };
        if actual_pgid == record.pgid as libc::pid_t {
            GroupIdentity::Original
        } else {
            GroupIdentity::Gone
        }
    }

    #[cfg(all(
        unix,
        not(any(target_os = "linux", target_os = "macos"))
    ))]
    {
        // No stable, supported process-start fingerprint is available here.
        // Destructive cleanup must fail closed instead of trusting a recycled
        // numeric PGID. We also preserve the record when the original leader
        // appears gone because this platform has no supported group-member
        // probe to prove that descendants are gone too.
        GroupIdentity::Unverifiable
    }

    #[cfg(not(unix))]
    {
        if crate::mcp::daemon::pid_alive(record.pgid) {
            GroupIdentity::Original
        } else {
            GroupIdentity::Gone
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_process_group_has_members(pgid: u32) -> bool {
    let mut pids = [0 as libc::pid_t; 64];
    // SAFETY: proc_listpgrppids writes at most `size_of_val(pids)` bytes into
    // the supplied stack buffer and does not retain the pointer.
    let written = unsafe {
        libc::proc_listpgrppids(
            pgid as libc::pid_t,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(&pids) as libc::c_int,
        )
    };
    written > 0 && pids.iter().any(|pid| *pid > 0)
}

#[cfg(target_os = "linux")]
fn process_group_has_live_members(pgid: u32) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            return false;
        };
        process_state_and_group(pid).is_some_and(|(state, group)| state != 'Z' && group == pgid)
    })
}

#[cfg(target_os = "linux")]
fn process_state_and_group(pid: u32) -> Option<(char, u32)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    process_state_and_group_from_stat(&stat)
}

#[cfg(target_os = "linux")]
fn process_state_and_group_from_stat(stat: &str) -> Option<(char, u32)> {
    let after_comm = stat.rsplit_once(')')?.1.trim_start();
    // Fields after comm begin with: state (3), ppid (4), pgrp (5).
    let mut fields = after_comm.split_whitespace();
    let state = fields.next()?.chars().next()?;
    fields.next()?;
    let pgid = fields.next()?.parse().ok()?;
    Some((state, pgid))
}

pub(crate) fn age(record: &TrackedProcessGroup) -> Duration {
    (Utc::now() - record.recorded_at)
        .to_std()
        .unwrap_or_default()
}

/// Reclaim one fingerprint-matched orphan process group.
pub(crate) async fn reap(
    cas_root: &Path,
    record: &TrackedProcessGroup,
) -> io::Result<ReapOutcome> {
    match group_identity(record) {
        GroupIdentity::Original => {}
        GroupIdentity::Gone => {
            untrack(cas_root, record.pgid)?;
            return Ok(ReapOutcome::AlreadyGone);
        }
        GroupIdentity::FingerprintMismatch => {
            untrack(cas_root, record.pgid)?;
            return Ok(ReapOutcome::FingerprintMismatch);
        }
        GroupIdentity::Unverifiable => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to signal process group {}: durable process identity cannot be verified",
                    record.pgid
                ),
            ));
        }
    }

    #[cfg(unix)]
    {
        // SAFETY: the PGID was resolved from a durable, starttime-fingerprinted
        // factory record and revalidated immediately above.
        let rc = unsafe { libc::killpg(record.pgid as libc::pid_t, libc::SIGKILL) };
        if rc != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
    }

    #[cfg(not(unix))]
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process-group cleanup requires Unix",
        ));
    }

    for _ in 0..20 {
        if !is_live(record) {
            untrack(cas_root, record.pgid)?;
            return Ok(ReapOutcome::Reaped);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Err(io::Error::other(format!(
        "process group {} still exists after SIGKILL",
        record.pgid
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_round_trips_and_untrack_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let record = track(temp.path(), "worker-a", "factory-a", std::process::id()).unwrap();
        let listed = list(temp.path()).unwrap();
        assert_eq!(listed, vec![record.clone()]);

        untrack(temp.path(), record.pgid).unwrap();
        untrack(temp.path(), record.pgid).unwrap();
        assert!(list(temp.path()).unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_linux_process_group_from_stat_with_spaces_in_comm() {
        let stat = "123 (worker name) S 10 456 456 0 -1 0 0 0 0 0 0 0 0 0 0 0 0 0 99";
        assert_eq!(process_state_and_group_from_stat(stat), Some(('S', 456)));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reap_refuses_a_recycled_pgid_fingerprint_at_kill_time() {
        let temp = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let mut record = track(temp.path(), "stale-worker", "dead-factory", pid).unwrap();
        record.pid_starttime = record.pid_starttime.map(|start| start + 1);

        assert_eq!(
            reap(temp.path(), &record).await.unwrap(),
            ReapOutcome::FingerprintMismatch
        );
        assert!(
            crate::mcp::daemon::pid_alive(pid),
            "fingerprint mismatch must never signal the recycled process"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reap_preserves_an_unverifiable_live_record() {
        let temp = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let mut record = track(temp.path(), "legacy-worker", "legacy-factory", pid).unwrap();
        record.pid_starttime = None;
        fs::write(
            record_path(temp.path(), pid),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();

        assert_eq!(status(&record), ProcessGroupStatus::Unverifiable);
        let error = reap(temp.path(), &record).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            crate::mcp::daemon::pid_alive(pid),
            "unverifiable identity must never be signaled"
        );
        assert_eq!(list(temp.path()).unwrap(), vec![record]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reap_kills_a_synthetic_long_lived_process_group() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 300 & wait"]);
        // SAFETY: setsid is async-signal-safe and runs in the child between
        // fork and exec, isolating the test from cargo's process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let pgid = child.id();
        let record = track(temp.path(), "synthetic-worker", "synthetic-factory", pgid).unwrap();
        assert!(is_live(&record));

        assert_eq!(
            reap(temp.path(), &record).await.unwrap(),
            ReapOutcome::Reaped
        );
        let _ = child.wait();
        assert!(!is_live(&record));
        assert!(list(temp.path()).unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reap_kills_group_after_the_original_leader_has_exited() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let child_pid_path = temp.path().join("child.pid");
        let script = format!(
            "sleep 300 & echo $! > '{}' ; sleep 0.25",
            child_pid_path.display()
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        // SAFETY: isolate the synthetic lane from cargo's process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut leader = command.spawn().unwrap();
        let pgid = leader.id();
        let record = track(temp.path(), "leaderless-worker", "dead-factory", pgid).unwrap();
        assert!(is_live(&record));

        assert!(leader.wait().unwrap().success());
        let child_pid: u32 = fs::read_to_string(&child_pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            crate::mcp::daemon::pid_alive(child_pid),
            "synthetic long-lived child must survive its shell leader"
        );
        assert!(
            is_live(&record),
            "leaderless descendants must keep the tracked group live"
        );

        assert_eq!(
            reap(temp.path(), &record).await.unwrap(),
            ReapOutcome::Reaped
        );
        assert!(
            !crate::mcp::daemon::pid_alive(child_pid)
                || process_state_and_group(child_pid).is_some_and(|(state, _)| state == 'Z'),
            "leaderless synthetic child must be terminated"
        );
    }
}
