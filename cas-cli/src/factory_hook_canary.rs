//! cas-0f5b: the launch canary that proves a factory agent's hooks run.
//!
//! The SessionStart hook writes a per-agent marker under
//! `<cas_dir>/factory/hook-canary/`; the factory's spawn verification refuses
//! (kills, and reports to the supervisor) a Claude worker whose marker has not
//! appeared since its launch. Workspace trust (cas-pty claude_trust) is what
//! should make hooks run; this marker is the proof that they do. A worker with
//! no hooks has no guard at all, so there is no warn-and-continue path.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a Claude worker has, from launch, to show its SessionStart marker.
/// SessionStart fires before the worker's first turn; registration itself is
/// allowed 60 s, and the marker normally lands well before it.
pub const HOOK_CANARY_TIMEOUT: Duration = Duration::from_secs(60);

pub fn marker_dir(cas_dir: &Path) -> PathBuf {
    cas_dir.join("factory").join("hook-canary")
}

fn safe_name(agent: &str) -> String {
    agent
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

pub fn marker_path(cas_dir: &Path, agent: &str) -> PathBuf {
    marker_dir(cas_dir).join(safe_name(agent))
}

/// Record that this agent's SessionStart hook ran (written by the hook).
pub fn record_session_start(cas_dir: &Path, agent: &str, session_id: &str) -> io::Result<PathBuf> {
    let dir = marker_dir(cas_dir);
    std::fs::create_dir_all(&dir)?;
    let path = marker_path(cas_dir, agent);
    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let body = serde_json::json!({ "agent": agent, "session_id": session_id, "at_ms": at_ms });
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, body.to_string())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Whether the agent's SessionStart marker was written at or after `since`.
/// A marker left by an earlier worker of the same name never counts.
pub fn fired_since(cas_dir: &Path, agent: &str, since: SystemTime) -> bool {
    marker_written_at_ms(cas_dir, agent).is_some_and(|at_ms| at_ms >= unix_ms(since))
}

/// The instant a spawn's canary must postdate. Take it BEFORE the harness
/// process is started.
///
/// cas-2a49: the daemon used to stamp this after `finish_worker_spawn`
/// returned. That call starts the PTY and only then does git, store and pane
/// bookkeeping, which took 0.8–19 s on a loaded host (2026-10-09, load 14–24).
/// A Claude SessionStart hook that ran during that bookkeeping wrote a marker
/// older than the stamp. The verifier then rejected it as a previous worker's
/// marker and killed six healthy workers (spawn requests 2448, 2450–2453 and
/// 2459). Every refusal had its marker 0.8–19 s before the stamp; every
/// confirmed spawn was 0.1–0.6 s after it.
pub fn launch_floor() -> SystemTime {
    SystemTime::now()
}

/// The `at_ms` recorded in the agent's marker, if a readable one exists.
pub fn marker_written_at_ms(cas_dir: &Path, agent: &str) -> Option<u64> {
    let body = std::fs::read_to_string(marker_path(cas_dir, agent)).ok()?;
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()?
        .get("at_ms")?
        .as_u64()
}

fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// What the spawn verification does with a worker's hook canary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryVerdict {
    /// Not a hook-guarded harness, or the marker has appeared.
    Passed,
    /// No marker yet, still inside the timeout.
    Pending,
    /// No marker by the timeout: refuse the worker.
    Failed,
}

pub fn canary_verdict(
    hooks_required: bool,
    fired: bool,
    since_launch: Duration,
    timeout: Duration,
) -> CanaryVerdict {
    if !hooks_required || fired {
        CanaryVerdict::Passed
    } else if since_launch < timeout {
        CanaryVerdict::Pending
    } else {
        CanaryVerdict::Failed
    }
}

/// The refusal reported to the supervisor, naming a marker that exists but
/// predates `since` (cas-2a49) so "no canary" is never claimed while the
/// file is present.
pub fn failure_detail_since(
    cas_dir: &Path,
    agent: &str,
    timeout: Duration,
    since: SystemTime,
) -> String {
    let marker = marker_path(cas_dir, agent);
    let mut detail = failure_detail(agent, timeout, &marker);
    if let Some(at_ms) = marker_written_at_ms(cas_dir, agent) {
        let since_ms = unix_ms(since);
        detail.push_str(&format!(
            " A marker exists but was written at {at_ms} ms, {} ms before this launch's floor \
             ({since_ms} ms), so it was treated as a previous worker's.",
            since_ms.saturating_sub(at_ms)
        ));
    }
    detail
}

/// The refusal reported to the supervisor.
pub fn failure_detail(agent: &str, timeout: Duration, marker: &Path) -> String {
    format!(
        "Worker '{agent}' was refused: its Claude hooks did not run. No SessionStart canary at {} \
         within {}s of launch. A worker without hooks has no capped cargo runner, no worker-memory \
         admission and no Slack/publication/browser guards (cas-0f5b). Check that the worktree is \
         trusted in the worker's CLAUDE_CONFIG_DIR/.claude.json (projects[<cwd>].hasTrustDialogAccepted) \
         and that `cas hook SessionStart` is configured, then respawn.",
        marker.display(),
        timeout.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_0f5b_canary_refuses_a_claude_worker_whose_hooks_never_ran() {
        let t = HOOK_CANARY_TIMEOUT;
        assert_eq!(canary_verdict(true, false, Duration::from_secs(5), t), CanaryVerdict::Pending);
        assert_eq!(canary_verdict(true, false, t, t), CanaryVerdict::Failed);
        assert_eq!(canary_verdict(true, true, t * 2, t), CanaryVerdict::Passed);
        // Harnesses without CAS hooks are not held to the canary.
        assert_eq!(canary_verdict(false, false, t * 2, t), CanaryVerdict::Passed);
    }

    /// cas-2a49 regression: the SessionStart hook runs while the daemon is
    /// still doing post-PTY bookkeeping. A floor taken before the launch
    /// accepts that marker. A stamp taken after bookkeeping, the old
    /// behaviour, would reject it, and the refusal must then name the
    /// existing marker instead of claiming none exists.
    #[test]
    fn cas_2a49_marker_written_during_spawn_bookkeeping_counts() {
        let dir = tempfile::tempdir().unwrap();
        let floor = launch_floor();
        std::thread::sleep(Duration::from_millis(5));
        // PTY started; the hook fires while the daemon is still busy.
        record_session_start(dir.path(), "steady-leopard-44", "s-2459").unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let after_bookkeeping = SystemTime::now();

        assert!(fired_since(dir.path(), "steady-leopard-44", floor));
        assert!(
            !fired_since(dir.path(), "steady-leopard-44", after_bookkeeping),
            "a post-bookkeeping stamp is exactly what rejected the live marker"
        );
        let detail = failure_detail_since(
            dir.path(),
            "steady-leopard-44",
            HOOK_CANARY_TIMEOUT,
            after_bookkeeping,
        );
        assert!(detail.contains("A marker exists but was written at"), "{detail}");
        let none = failure_detail_since(dir.path(), "never-ran", HOOK_CANARY_TIMEOUT, floor);
        assert!(!none.contains("A marker exists"), "{none}");
    }

    #[test]
    fn cas_0f5b_only_a_marker_written_since_launch_counts() {
        let dir = tempfile::tempdir().unwrap();
        let launched = SystemTime::now();
        assert!(!fired_since(dir.path(), "quick-jaguar-39", launched), "no marker yet");
        record_session_start(dir.path(), "quick-jaguar-39", "s-1").unwrap();
        assert!(fired_since(dir.path(), "quick-jaguar-39", launched));
        // A respawn under the same name must not be confirmed by the old marker.
        let relaunched = SystemTime::now() + Duration::from_secs(1);
        assert!(!fired_since(dir.path(), "quick-jaguar-39", relaunched));
        // Names cannot escape the marker directory.
        assert_eq!(
            marker_path(dir.path(), "../x/y"),
            marker_dir(dir.path()).join("___x_y")
        );
    }
}
