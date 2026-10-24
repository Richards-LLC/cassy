use std::io::{self, Write};

use anyhow::{Result, bail};

use crate::cli::interactive::{PickerItem, PickerTag, pick};
use crate::ui::components::{Formatter, Renderable, StatusLine};
use crate::ui::factory::{SessionInfo, SessionManager};
use crate::ui::theme::ActiveTheme;

/// How long a daemon gets to honour SIGTERM before `cas kill` sends SIGKILL
/// (GH #1363). A daemon whose loop is wedged never honours it.
const SIGTERM_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long SIGKILL gets before the kill is reported as failed.
const SIGKILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

fn terminate_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;

        kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
            .map_err(|e| anyhow::anyhow!("failed to send SIGTERM to {pid}: {e}"))?;
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        let output = Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("taskkill failed for PID {}: {}", pid, stderr.trim());
        }
    }

    Ok(())
}

/// Build a picker item for a session (shared by kill and attach).
pub(super) fn session_picker_item(s: &SessionInfo) -> PickerItem {
    let status = if s.is_running { "running" } else { "stale" };
    let workers = format!(
        "{} worker{}",
        s.metadata.workers.len(),
        if s.metadata.workers.len() == 1 {
            ""
        } else {
            "s"
        }
    );
    PickerItem {
        label: s.name.clone(),
        tags: vec![
            PickerTag {
                text: status.into(),
            },
            PickerTag {
                text: format!("PID {}", s.metadata.daemon_pid),
            },
            PickerTag { text: workers },
        ],
    }
}

/// Kill a factory session. If no name is given, show an interactive picker.
pub fn execute_kill(name: Option<&str>, force: bool) -> Result<()> {
    let manager = SessionManager::new();

    let (name, picked) = match name {
        Some(n) => (n.to_string(), false),
        None => {
            let sessions = manager.list_sessions()?;
            if sessions.is_empty() {
                bail!("No factory sessions found.");
            }
            let items: Vec<PickerItem> = sessions.iter().map(session_picker_item).collect();
            match pick("Kill session", &items)? {
                Some(idx) => (sessions[idx].name.clone(), true),
                None => return Ok(()),
            }
        }
    };

    let session = manager
        .find_session(Some(&name))?
        .ok_or_else(|| anyhow::anyhow!("Session '{name}' not found"))?;

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    if !session.is_running {
        end_session_by_name(&name)?;
        StatusLine::success(format!("Cleaned up stale session: {name}")).render(&mut fmt)?;
        return Ok(());
    }

    // Skip confirmation if user already picked from the interactive list
    if !force && !picked {
        StatusLine::warning(format!(
            "This will terminate session '{}' (PID: {})",
            name, session.metadata.daemon_pid
        ))
        .render(&mut fmt)?;
        write!(io::stdout(), "Continue? [y/N] ")?;
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            StatusLine::info("Cancelled.").render(&mut fmt)?;
            return Ok(());
        }
    }

    end_session_by_name(&name)?;
    StatusLine::success(format!("Terminated session: {name}")).render(&mut fmt)?;
    Ok(())
}

/// What ending a session from Commander did (cas-55a4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndSessionOutcome {
    /// The session's daemon was sent SIGTERM and its metadata removed.
    Ended,
    /// No daemon of this session was running: only its metadata was removed.
    CleanedStale,
    /// No session of that name exists.
    NotFound,
}

/// End one factory session by name, the way `cas kill <name> --force` does,
/// for an authenticated Commander device. A daemon PID whose recorded start
/// time no longer matches belongs to another process now: it is never
/// signalled, and the session is cleaned up as stale.
pub(crate) fn end_session_by_name(name: &str) -> Result<EndSessionOutcome> {
    end_session_with_grace(name, SIGTERM_GRACE)
}

fn wait_for_daemon_exit(
    metadata: &crate::ui::factory::protocol::SessionMetadata,
    grace: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + grace;
    while crate::ui::factory::daemon_identity_is_live(metadata) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    true
}

fn end_session_with_grace(name: &str, term_grace: std::time::Duration) -> Result<EndSessionOutcome> {
    let manager = SessionManager::new();
    let Some(session) = manager.find_session(Some(name))? else {
        return Ok(EndSessionOutcome::NotFound);
    };
    if session.name != name {
        return Ok(EndSessionOutcome::NotFound);
    }
    let pid = session.metadata.daemon_pid;
    let same_daemon = match session.metadata.daemon_pid_starttime {
        Some(expected) => crate::mcp::daemon::read_pid_starttime(pid) == Some(expected),
        None => true,
    };
    if !session.is_running || !same_daemon {
        manager.remove_metadata(name)?;
        return Ok(EndSessionOutcome::CleanedStale);
    }
    terminate_process(pid)?;
    // SIGTERM delivery is not death. Wait before reclaiming socket names.
    if !wait_for_daemon_exit(&session.metadata, term_grace) {
        // GH #1363: a daemon whose loop is wedged never acts on SIGTERM.
        // Escalate, but only against the same daemon identity.
        #[cfg(unix)]
        if crate::ui::factory::daemon_identity_is_live(&session.metadata) {
            let reason = format!(
                "cas kill: daemon PID {pid} did not exit within {} s of SIGTERM; sent SIGKILL",
                term_grace.as_secs()
            );
            crate::ui::factory::record_daemon_lifecycle(name, "killed", &reason);
            tracing::warn!(session = name, pid, "{reason}");
            use nix::sys::signal::{Signal, kill};
            let _ = kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
        if !wait_for_daemon_exit(&session.metadata, SIGKILL_GRACE) {
            bail!(
                "Session '{name}' (PID {pid}) survived SIGTERM and SIGKILL; sockets retained. Check: cas list --name {name}"
            );
        }
    }
    // A replacement daemon may have acquired the same friendly name while
    // shutdown ran. Its receipt and sockets belong to it, not this command.
    if let Some(current) = manager.find_session(Some(name))? {
        if current.metadata.daemon_pid != pid
            || current.metadata.daemon_pid_starttime != session.metadata.daemon_pid_starttime
        {
            return Ok(EndSessionOutcome::Ended);
        }
    }
    manager.remove_metadata(name)?;
    Ok(EndSessionOutcome::Ended)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// GH #1363: a daemon whose loop is wedged ignores SIGTERM. `cas kill`
    /// used to give up after 3 s and leave it running; it now escalates to
    /// SIGKILL, records why in daemon.log, and removes the session.
    #[test]
    fn kill_escalates_to_sigkill_when_the_daemon_ignores_sigterm() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let child = std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
            .spawn()
            .unwrap();
        let pid = child.id();
        // Reap the child so a killed daemon does not linger as a zombie.
        let reaper = std::thread::spawn(move || {
            let mut child = child;
            child.wait()
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        let manager = SessionManager::new();
        manager
            .save_metadata(&crate::ui::factory::create_metadata(
                "wedged", pid, "supervisor", &[], None, None, None,
            ))
            .unwrap();

        let outcome = end_session_with_grace("wedged", std::time::Duration::from_millis(300));
        if outcome.is_err() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let status = reaper.join().unwrap().unwrap();
        assert_eq!(outcome.unwrap(), EndSessionOutcome::Ended);
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert!(manager.find_session(Some("wedged")).unwrap().is_none());
        let log =
            std::fs::read_to_string(crate::ui::factory::daemon_log_path("wedged")).unwrap();
        assert!(log.contains("\"killed\"") && log.contains("sent SIGKILL"), "{log}");
    }
}

/// Kill all factory sessions
pub fn execute_kill_all(force: bool) -> Result<()> {
    let manager = SessionManager::new();
    let sessions = manager.list_sessions()?;

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    if sessions.is_empty() {
        StatusLine::info("No factory sessions found.").render(&mut fmt)?;
        return Ok(());
    }

    if !force {
        StatusLine::warning(format!(
            "This will terminate {} session(s).",
            sessions.len()
        ))
        .render(&mut fmt)?;
        write!(io::stdout(), "Continue? [y/N] ")?;
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            StatusLine::info("Cancelled.").render(&mut fmt)?;
            return Ok(());
        }
    }

    let mut killed = 0usize;
    for session in sessions {
        match kill_session_if_running(&session.name) {
            Ok(true) => {
                killed += 1;
                StatusLine::success(format!("Terminated session: {}", session.name))
                    .render(&mut fmt)?;
            }
            Ok(false) => {}
            Err(e) => {
                StatusLine::error(format!(
                    "Failed to terminate session '{}': {}",
                    session.name, e
                ))
                .render(&mut fmt)?;
            }
        }
    }

    if killed == 0 {
        StatusLine::info("No running sessions to terminate.").render(&mut fmt)?;
    } else {
        StatusLine::success(format!("Terminated {killed} session(s).")).render(&mut fmt)?;
    }

    Ok(())
}

/// Kill a session if it's running (internal use, no confirmation)
pub(super) fn kill_session_if_running(name: &str) -> Result<bool> {
    Ok(end_session_by_name(name)? == EndSessionOutcome::Ended)
}

/// Kill all orphaned daemon processes (running but socket gone)
pub(super) fn cleanup_orphaned_daemons() -> usize {
    let manager = SessionManager::new();
    let sessions = match manager.list_sessions() {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let mut killed = 0;

    for session in sessions {
        if session.is_running && !session.socket_exists {
            match end_session_by_name(&session.name) {
                Ok(EndSessionOutcome::Ended) => {
                    killed += 1;
                    tracing::info!(
                        "Killed orphaned daemon: {} (PID {})",
                        session.name,
                        session.metadata.daemon_pid
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        "Failed to terminate orphaned daemon {} (PID {}): {}",
                        session.name,
                        session.metadata.daemon_pid,
                        e
                    );
                }
            }
        }
    }

    killed
}
