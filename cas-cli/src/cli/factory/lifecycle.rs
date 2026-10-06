use std::io::{self, Write};

use anyhow::{Result, bail};
use crossterm::style::Color;

use crate::cli::interactive::{PickerItem, PickerTag, pick};
use crate::ui::components::{Formatter, Renderable, StatusLine};
use crate::ui::factory::{SessionInfo, SessionManager};
use crate::ui::theme::ActiveTheme;

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
    let (status, color) = if s.is_running {
        ("running", Color::Green)
    } else {
        ("stale", Color::DarkGrey)
    };
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
                color,
            },
            PickerTag {
                text: format!("PID {}", s.metadata.daemon_pid),
                color: Color::DarkGrey,
            },
            PickerTag {
                text: workers,
                color: Color::DarkGrey,
            },
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
    // SIGTERM delivery is not death. Wait before reclaiming socket names; if
    // shutdown stalls, retain the receipt and leave the live daemon reachable.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while crate::ui::factory::daemon_identity_is_live(&session.metadata) {
        if std::time::Instant::now() >= deadline {
            bail!(
                "Session '{name}' is still shutting down; sockets retained. Check: cas list --name {name}"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
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
