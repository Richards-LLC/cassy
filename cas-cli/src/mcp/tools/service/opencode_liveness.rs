//! OpenCode liveness and session attribution for factory surfaces.
//!
//! OpenCode does not expose a Claude-shaped JSONL transcript. The plugin's
//! CAS-side projection is therefore the only durable session signal here;
//! when its signal expires, the process table is an intentionally weaker
//! fallback. In particular, this module never consults OpenCode's shared
//! SQLite/WAL mtimes and never synthesizes a Claude transcript path.

use cas_mux::{
    OpenCodeLiveness, OpenCodeLivenessVerdict, OpenCodeSessionState, load_opencode_session_state,
    opencode_session_state_path,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Plugin signals are authoritative only for this bounded interval. After
/// expiry, a live process is reported as process evidence rather than as a
/// stale session claim.
pub(crate) const OPENCODE_SIGNAL_TTL_MS: u64 = 30_000;
const MAX_EXPORT_BYTES: usize = 64 * 1024;
const EXPORT_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCodeObservation {
    pub state: OpenCodeSessionState,
    pub verdict: OpenCodeLivenessVerdict,
}

/// Read one worker's CAS↔OpenCode projection. A missing or invalid mapping is
/// deliberately distinguishable from a Claude/Codex transcript miss.
pub(crate) fn observe(
    cas_root: &Path,
    cas_session_id: &str,
    now_ms: u64,
    process_alive: bool,
) -> Option<OpenCodeObservation> {
    if !valid_cas_session_key(cas_session_id) {
        return None;
    }
    let path = opencode_session_state_path(cas_root, cas_session_id);
    let state = load_opencode_session_state(&path).ok()?;
    if state.schema_version != cas_mux::OPENCODE_STATE_SCHEMA_VERSION
        || state.cas_session_id != cas_session_id
    {
        return None;
    }
    let verdict = state.liveness_verdict(now_ms, OPENCODE_SIGNAL_TTL_MS, process_alive);
    Some(OpenCodeObservation { state, verdict })
}

/// Session identity used for blame/export attribution. `None` means the
/// plugin has not delivered its root-session mapping yet.
pub(crate) fn mapped_session_id(observation: &OpenCodeObservation) -> Option<&str> {
    observation.state.opencode_session_id.as_deref()
}

pub(crate) fn active_tool(observation: &OpenCodeObservation) -> bool {
    observation.state.active_tool.is_some()
}

pub(crate) fn has_active_work(observation: &OpenCodeObservation) -> bool {
    active_tool(observation)
        || matches!(
            observation.verdict,
            OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Busy)
                | OpenCodeLivenessVerdict::ProcessAliveFallback
        )
}

/// Last activity is derived from plugin event timestamps, never state-file
/// mtime. The phase is intentionally harness-neutral for shared worker views.
pub(crate) fn last_activity_secs(
    observation: &OpenCodeObservation,
    now_ms: u64,
) -> Option<(i64, &'static str)> {
    observation.state.last_activity_at.map(|at| {
        (
            now_ms.saturating_sub(at).saturating_div(1_000) as i64,
            if active_tool(observation) {
                "OpenCode tool activity"
            } else {
                "OpenCode session signal"
            },
        )
    })
}

pub(crate) fn verdict_label(verdict: OpenCodeLivenessVerdict) -> &'static str {
    match verdict {
        OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Unknown) => "signal: unknown",
        OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Busy) => "signal: busy",
        OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Idle) => "signal: idle",
        OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Error) => "signal: error",
        OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Deleted) => "signal: deleted",
        OpenCodeLivenessVerdict::ProcessAliveFallback => "process-alive fallback",
        OpenCodeLivenessVerdict::NotObserved => "not observed",
    }
}

pub(crate) fn render_status(observation: Option<&OpenCodeObservation>) -> String {
    let Some(observation) = observation else {
        return "\n    OpenCode session mapping: unavailable/delayed (process evidence only)"
            .to_string();
    };
    let session = mapped_session_id(observation).unwrap_or("<pending ses_* mapping>");
    format!(
        "\n    OpenCode session: {session}\n    OpenCode liveness: {}",
        verdict_label(observation.verdict)
    )
}

/// Export the mapped OpenCode session with a bounded read-only subprocess.
/// This is the debug/attribution equivalent of a transcript tail and is never
/// used as a fallback to a Claude path.
pub(crate) fn export_session(
    observation: &OpenCodeObservation,
    account_dir: Option<&str>,
) -> Result<String, String> {
    export_session_with(Path::new("opencode"), observation, account_dir)
}

fn export_session_with(
    executable: &Path,
    observation: &OpenCodeObservation,
    account_dir: Option<&str>,
) -> Result<String, String> {
    let session_id = mapped_session_id(observation)
        .ok_or_else(|| "OpenCode session mapping is unavailable/delayed".to_string())?;
    let directory = PathBuf::from(&observation.state.directory);
    if observation.state.directory.trim().is_empty() || !directory.is_dir() {
        return Err(format!(
            "mapped OpenCode directory is unavailable: {}",
            directory.display()
        ));
    }

    let mut command = Command::new(executable);
    command.args(["export", session_id]).current_dir(&directory);
    if let Some(account_dir) = account_dir {
        let env = cas_pty::opencode::account_root_env(account_dir, None, None)
            .map_err(|error| format!("OpenCode account root rejected: {error}"))?;
        for (key, value) in env {
            command.env(key, value);
        }
    }
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(EXPORT_DEADLINE),
        EXPORT_DEADLINE,
    )
    .map_err(|error| format!("bounded OpenCode export failed: {error:?}"))?;
    if !output.status.success() {
        return Err(format!(
            "opencode export exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = output.stdout.len().min(MAX_EXPORT_BYTES);
    let mut rendered = String::from_utf8_lossy(&output.stdout[..bytes]).into_owned();
    if output.stdout.len() > MAX_EXPORT_BYTES {
        rendered.push_str("\n[OpenCode export truncated at 64 KiB]");
    }
    Ok(rendered)
}

fn valid_cas_session_key(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_mux::{
        OpenCodeSessionEvent, OpenCodeSessionEventKind, OpenCodeStatus,
        persist_opencode_session_state,
    };
    use tempfile::TempDir;

    fn state(root: &TempDir, cas_id: &str, directory: &Path) -> OpenCodeSessionState {
        let mut state = OpenCodeSessionState::new(cas_id, directory.display().to_string());
        state.apply(OpenCodeSessionEvent {
            at: 1_000,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_test-root".to_string(),
                directory: directory.display().to_string(),
            },
        });
        let path = opencode_session_state_path(root.path(), cas_id);
        persist_opencode_session_state(&path, &state).unwrap();
        state
    }

    #[test]
    fn busy_idle_cancel_and_crash_use_reduced_signals() {
        let root = TempDir::new().unwrap();
        let directory = TempDir::new().unwrap();
        let cas_id = "opencode-worker";
        let mut snapshot = state(&root, cas_id, directory.path());
        snapshot.apply(OpenCodeSessionEvent {
            at: 2_000,
            kind: OpenCodeSessionEventKind::ToolBefore {
                session_id: "ses_test-root".to_string(),
                name: "bash".to_string(),
                call_id: Some("call-1".to_string()),
            },
        });
        snapshot.apply(OpenCodeSessionEvent {
            at: 3_000,
            kind: OpenCodeSessionEventKind::ToolAfter {
                session_id: "ses_test-root".to_string(),
                call_id: Some("call-1".to_string()),
                success: false,
            },
        });
        snapshot.apply(OpenCodeSessionEvent {
            at: 4_000,
            kind: OpenCodeSessionEventKind::Status {
                session_id: "ses_test-root".to_string(),
                status: OpenCodeStatus::Idle,
            },
        });
        persist_opencode_session_state(
            &opencode_session_state_path(root.path(), cas_id),
            &snapshot,
        )
        .unwrap();
        let idle = observe(root.path(), cas_id, 4_100, true).unwrap();
        assert_eq!(
            idle.verdict,
            OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Idle)
        );
        assert!(!has_active_work(&idle));
        assert_eq!(
            idle.state.last_tool.as_ref().and_then(|tool| tool.success),
            Some(false)
        );

        snapshot.apply(OpenCodeSessionEvent {
            at: 5_000,
            kind: OpenCodeSessionEventKind::Status {
                session_id: "ses_test-root".to_string(),
                status: OpenCodeStatus::Error,
            },
        });
        persist_opencode_session_state(
            &opencode_session_state_path(root.path(), cas_id),
            &snapshot,
        )
        .unwrap();
        let crashed = observe(root.path(), cas_id, 5_100, false).unwrap();
        assert_eq!(
            crashed.verdict,
            OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Error)
        );
    }

    #[test]
    fn missing_or_delayed_mapping_never_becomes_transcript_evidence() {
        let root = TempDir::new().unwrap();
        assert!(observe(root.path(), "opencode-missing", 1_000, true).is_none());
        let directory = TempDir::new().unwrap();
        let cas_id = "opencode-delayed";
        let delayed = OpenCodeSessionState::new(cas_id, directory.path().display().to_string());
        persist_opencode_session_state(&opencode_session_state_path(root.path(), cas_id), &delayed)
            .unwrap();
        let observation = observe(root.path(), cas_id, 1_000, true).unwrap();
        assert_eq!(
            observation.verdict,
            OpenCodeLivenessVerdict::ProcessAliveFallback
        );
        assert!(mapped_session_id(&observation).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn export_uses_mapped_directory_and_account_root() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempDir::new().unwrap();
        let directory = TempDir::new().unwrap();
        let account = TempDir::new().unwrap();
        let cas_id = "opencode-isolated";
        let state = state(&root, cas_id, directory.path());
        let script = root.path().join("fake-opencode");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$CAS_OPENCODE_ACCOUNT_DIR\" \"$2\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();

        let rendered = export_session_with(
            &script,
            &OpenCodeObservation {
                state,
                verdict: OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Idle),
            },
            Some(account.path().to_str().unwrap()),
        )
        .unwrap();
        assert!(rendered.starts_with(&format!(
            "{}|{}|ses_test-root",
            directory.path().display(),
            account.path().display()
        )));
    }
}
