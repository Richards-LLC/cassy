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
    let Ok(body) = std::fs::read_to_string(marker_path(cas_dir, agent)) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    let Some(at_ms) = value.get("at_ms").and_then(serde_json::Value::as_u64) else {
        return false;
    };
    let since_ms = since
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    at_ms >= since_ms
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
