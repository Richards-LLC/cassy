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
pub fn execute_kill(name: Option<&str>, force: bool, force_stale: bool) -> Result<()> {
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

    if force_stale {
        let pid = session.metadata.daemon_pid;
        if !force {
            use std::io::IsTerminal;
            if !io::stdin().is_terminal() {
                bail!(
                    "--force-stale needs confirmation and no terminal is attached; session '{name}' was left unchanged. \
                     After checking that `ps -p {pid}` shows another program, rerun with --force."
                );
            }
            StatusLine::warning(format!(
                "This removes session '{name}' and reaps its recorded workers without signalling PID {pid}. \
                 Only continue if `ps -p {pid}` shows a program other than this factory daemon."
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
        match force_clear_stale_session(&name, pid, &crate::mcp::daemon::read_pid_starttime)? {
            EndSessionOutcome::NotFound => {
                StatusLine::info(format!(
                    "Session '{name}' no longer exists; nothing was cleared."
                ))
                .render(&mut fmt)?;
            }
            _ => {
                StatusLine::success(format!("Cleared stale session: {name}")).render(&mut fmt)?;
            }
        }
        return Ok(());
    }

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
    metadata: &crate::ui::factory::SessionMetadata,
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
    end_session_with(name, term_grace, &crate::mcp::daemon::read_pid_starttime)
}

/// [`end_session_with_grace`] reading process start times through
/// `starttime_of`, so tests can make one unreadable.
fn end_session_with(
    name: &str,
    term_grace: std::time::Duration,
    starttime_of: &dyn Fn(u32) -> Option<u64>,
) -> Result<EndSessionOutcome> {
    let manager = SessionManager::new();
    let Some(session) = manager.find_session(Some(name))? else {
        return Ok(EndSessionOutcome::NotFound);
    };
    if session.name != name {
        return Ok(EndSessionOutcome::NotFound);
    }
    let pid = session.metadata.daemon_pid;
    match recorded_daemon(&session, starttime_of) {
        RecordedDaemon::Gone => {
            // cas-fcb0: the daemon is already gone, for example after a
            // manual `kill -9`. Its workers lead their own sessions and
            // outlive it, so reap the groups it recorded (fingerprint-
            // checked) before the record that names its store is removed.
            #[cfg(unix)]
            reap_session_worker_groups(name, &session.metadata, "after the daemon exited");
            manager.remove_metadata(name)?;
            return Ok(EndSessionOutcome::CleanedStale);
        }
        RecordedDaemon::Unknown => {
            let reason = format!(
                "cas kill: worker reap skipped; daemon PID {pid} is running but its start time could not be read, so it may still be this session's daemon"
            );
            crate::ui::factory::record_daemon_lifecycle(name, "worker_reap_skipped", &reason);
            tracing::warn!(session = name, pid, "{reason}");
            bail!(
                "Session '{name}' was left untouched: PID {pid} is running but its start time could not be read, so it may still be this session's daemon. \
                 If `ps -p {pid}` shows this factory daemon, run `kill -9 {pid}`, then `cas kill {name}` to clean up the session and reap its workers; \
                 if it shows another program, run `cas kill {name} --force-stale` to remove the session record and reap its recorded workers."
            );
        }
        RecordedDaemon::Same => {}
    }
    terminate_process(pid)?;
    // SIGTERM delivery is not death. Wait before reclaiming socket names.
    // Set once the daemon is known dead after an escalation; it names how.
    let mut reap_cause: Option<&str> = None;
    let mut exited_mid_escalation = false;
    if !wait_for_daemon_exit(&session.metadata, term_grace) {
        // GH #1363: a daemon whose loop is wedged never acts on SIGTERM.
        // Escalate only on a positive identity (cas-3372): an unreadable or
        // unrecorded start time is not a match.
        let identity = match session.metadata.daemon_pid_starttime {
            None => Err("the session record has no daemon start time"),
            Some(expected) => match starttime_of(pid) {
                Some(actual) if actual == expected => Ok(true),
                // cas-fcb0: the daemon exited after the grace check and its
                // PID was reused. It did end; there is nothing to signal.
                Some(_) => Ok(false),
                None => Err("the daemon's start time could not be read"),
            },
        };
        if let Ok(false) = identity {
            exited_mid_escalation = true;
        }
        if let Err(why) = identity {
            // cas-4b42: say what actually happened, and leave a record.
            let reason = format!(
                "cas kill: daemon PID {pid} did not exit within {} s of SIGTERM; SIGKILL withheld because {why}",
                term_grace.as_secs()
            );
            crate::ui::factory::record_daemon_lifecycle(name, "kill_withheld", &reason);
            tracing::warn!(session = name, pid, "{reason}");
            bail!(
                "Session '{name}' (PID {pid}) did not exit after SIGTERM, and SIGKILL was withheld because {why}. \
                 If `ps -p {pid}` shows this factory daemon, run `kill -9 {pid}`, then `cas kill {name}` to clean up the session and reap its workers; \
                 if it shows another program, run `cas kill {name} --force-stale`."
            );
        }
        if !exited_mid_escalation {
            #[cfg(unix)]
            {
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
        reap_cause = Some(if exited_mid_escalation {
            "after the daemon exited"
        } else {
            "after SIGKILL"
        });
    }
    // A replacement daemon may have acquired the same friendly name while
    // shutdown ran. Its receipt, sockets and freshly recorded workers belong
    // to it, not this command, so this check precedes the reap (cas-4b42).
    if let Some(current) = manager.find_session(Some(name))? {
        if current.metadata.daemon_pid != pid
            || current.metadata.daemon_pid_starttime != session.metadata.daemon_pid_starttime
        {
            if reap_cause.is_some() {
                crate::ui::factory::record_daemon_lifecycle(
                    name,
                    "worker_reap_skipped",
                    &format!(
                        "cas kill: worker reap skipped; replacement daemon PID {} now owns this session",
                        current.metadata.daemon_pid
                    ),
                );
            }
            return Ok(EndSessionOutcome::Ended);
        }
    }
    // Workers lead their own sessions, so the daemon's death does not reach
    // them, and a SIGKILL can land before its teardown stopped them. Reap
    // the groups the daemon recorded for this session.
    #[cfg(unix)]
    if let Some(cause) = reap_cause {
        reap_session_worker_groups(name, &session.metadata, cause);
    }
    manager.remove_metadata(name)?;
    Ok(EndSessionOutcome::Ended)
}

/// cas-47037: a daemon is dead only when its PID is not running or a
/// readable start time differs. An unreadable start time proves nothing.
fn recorded_daemon(
    session: &crate::ui::factory::SessionInfo,
    starttime_of: &dyn Fn(u32) -> Option<u64>,
) -> RecordedDaemon {
    match session.metadata.daemon_pid_starttime {
        _ if !session.is_running => RecordedDaemon::Gone,
        None => RecordedDaemon::Same,
        Some(expected) => match starttime_of(session.metadata.daemon_pid) {
            Some(actual) if actual == expected => RecordedDaemon::Same,
            Some(_) => RecordedDaemon::Gone,
            None => RecordedDaemon::Unknown,
        },
    }
}

/// `cas kill <name> --force-stale` (cas-47037): the operator has confirmed
/// that the PID recorded for `name` is not its factory daemon. Remove the
/// record and reap the recorded workers, fingerprint-checked. The recorded
/// PID is never signalled, and a PID whose readable start time matches the
/// recorded one, which proves it is the daemon, is refused.
fn force_clear_stale_session(
    name: &str,
    confirmed_pid: u32,
    starttime_of: &dyn Fn(u32) -> Option<u64>,
) -> Result<EndSessionOutcome> {
    let manager = SessionManager::new();
    let Some(session) = manager.find_session(Some(name))? else {
        return Ok(EndSessionOutcome::NotFound);
    };
    if session.name != name {
        return Ok(EndSessionOutcome::NotFound);
    }
    let pid = session.metadata.daemon_pid;
    // The operator confirmed one PID; a record rewritten since then belongs
    // to a daemon nobody has looked at.
    if pid != confirmed_pid {
        bail!(
            "Session '{name}' now records PID {pid}, not the PID {confirmed_pid} you confirmed; nothing was changed. Run `cas kill {name}` again."
        );
    }
    // Refuse only on positive proof: a recorded start time that matches the
    // running PID's readable one. No record, or an unreadable start time,
    // proves nothing, and the operator has checked the PID.
    if session.is_running
        && let Some(expected) = session.metadata.daemon_pid_starttime
        && starttime_of(pid) == Some(expected)
    {
        bail!(
            "Session '{name}': PID {pid} has the start time recorded for this session's daemon, so it is the daemon and --force-stale does not apply. Run `cas kill {name}`."
        );
    }
    crate::ui::factory::record_daemon_lifecycle(
        name,
        "force_stale",
        &format!("cas kill --force-stale: the operator confirmed PID {pid} is not this session's daemon"),
    );
    #[cfg(unix)]
    reap_session_worker_groups(
        name,
        &session.metadata,
        "after the operator confirmed the daemon is gone (--force-stale)",
    );
    manager.remove_metadata(name)?;
    Ok(EndSessionOutcome::CleanedStale)
}

/// What `cas kill` knows about the process at a session's recorded PID.
enum RecordedDaemon {
    /// Still the session's daemon, or no start time was recorded to doubt it.
    Same,
    /// Not running, or a different process (its start time differs).
    Gone,
    /// Running, but its start time could not be read.
    Unknown,
}

/// Kill the worker process groups a dead daemon recorded for
/// `session` (cas-3372). Each reap is fingerprint-checked, so a recycled
/// PGID is never signalled. The groups live in the daemon's own store,
/// which a CAS_ROOT daemon records as `cas_root` (cas-4b42).
#[cfg(unix)]
fn reap_session_worker_groups(
    session: &str,
    metadata: &crate::ui::factory::SessionMetadata,
    cause: &str,
) {
    let skipped = |why: String| {
        let detail = format!("cas kill: worker reap skipped; {why}");
        tracing::warn!(session, "{detail}");
        crate::ui::factory::record_daemon_lifecycle(session, "worker_reap_skipped", &detail);
    };
    let cas_root = match session_store(metadata) {
        Ok(root) => root,
        Err(why) => return skipped(why),
    };
    let records = match crate::ui::factory::process_groups::list(&cas_root) {
        Ok(records) => records,
        Err(error) => {
            return skipped(format!(
                "could not list worker process groups in {}: {error}",
                cas_root.display()
            ));
        }
    };
    for record in records.iter().filter(|record| record.factory_session == session) {
        let outcome = crate::ui::factory::process_groups::reap_blocking(&cas_root, record);
        let detail = format!(
            "cas kill: reaped worker '{}' process group {} {cause}: {outcome:?}",
            record.worker_name, record.pgid
        );
        tracing::info!(session, "{detail}");
        crate::ui::factory::record_daemon_lifecycle(session, "worker_reaped", &detail);
    }
}

/// The store a session's daemon recorded its worker groups in.
#[cfg(unix)]
fn session_store(
    metadata: &crate::ui::factory::SessionMetadata,
) -> std::result::Result<std::path::PathBuf, String> {
    if let Some(root) = metadata.cas_root.as_deref() {
        let root = std::path::PathBuf::from(root);
        return if root.is_dir() {
            Ok(root)
        } else {
            Err(format!("the daemon's recorded store {} is gone", root.display()))
        };
    }
    let Some(project_dir) = metadata.project_dir.as_deref() else {
        return Err("the session record names no store or project directory".to_string());
    };
    let project_dir = std::path::Path::new(project_dir);
    let own_store = project_dir.join(".cas");
    if own_store.is_dir() {
        return Ok(own_store);
    }
    crate::store::find_cas_root_ignoring_env(project_dir).map_err(|error| {
        format!("no store found for project {}: {error}", project_dir.display())
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Command, ExitStatus, Stdio};
    use std::thread::JoinHandle;

    /// A child in its own process group with no inherited output handles, so
    /// nothing it leaves behind holds the test's pipes (cas-4b42). The join
    /// handle reaps it, so a killed child never lingers as a zombie.
    fn spawn_group(program: &str, args: &[&str]) -> (u32, JoinHandle<std::io::Result<ExitStatus>>) {
        let mut child = Command::new(program)
            .args(args)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        (pid, std::thread::spawn(move || child.wait()))
    }

    fn kill_group(pgid: u32) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pgid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    fn save_session(
        name: &str,
        pid: u32,
        project: &std::path::Path,
    ) -> crate::ui::factory::SessionMetadata {
        let metadata = crate::ui::factory::create_metadata(
            name,
            pid,
            "supervisor",
            &[],
            None,
            project.to_str(),
            None,
        );
        SessionManager::new().save_metadata(&metadata).unwrap();
        metadata
    }

    fn daemon_log(name: &str) -> String {
        std::fs::read_to_string(crate::ui::factory::daemon_log_path(name)).unwrap_or_default()
    }

    /// GH #1363: a daemon whose loop is wedged ignores SIGTERM. `cas kill`
    /// escalates to SIGKILL, records why in daemon.log, and removes the
    /// session. Workers lead their own sessions, so it then reaps the
    /// session's worker groups from the store the daemon recorded, which a
    /// CAS_ROOT daemon keeps outside the project (cas-3372, cas-4b42).
    #[test]
    fn kill_escalates_to_sigkill_when_the_daemon_ignores_sigterm() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (worker_pgid, worker) = spawn_group("sleep", &["30"]);
        let (pid, daemon) = spawn_group("sh", &["-c", "trap '' TERM; exec sleep 30"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        crate::ui::factory::process_groups::track(store.path(), "w1", "wedged", worker_pgid)
            .unwrap();
        let mut metadata = save_session("wedged", pid, project.path());
        metadata.cas_root = Some(store.path().to_string_lossy().to_string());
        SessionManager::new().save_metadata(&metadata).unwrap();

        let outcome = end_session_with_grace("wedged", std::time::Duration::from_millis(300));
        kill_group(pid);
        let status = daemon.join().unwrap().unwrap();
        // A worker that survived would hold this join for 30 s.
        let started = std::time::Instant::now();
        let worker_status = worker.join().unwrap().unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the SIGKILLed daemon's worker group was left running"
        );
        assert_eq!(worker_status.signal(), Some(libc::SIGKILL));
        assert_eq!(outcome.unwrap(), EndSessionOutcome::Ended);
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert!(SessionManager::new().find_session(Some("wedged")).unwrap().is_none());
        let log = daemon_log("wedged");
        assert!(log.contains("\"killed\"") && log.contains("sent SIGKILL"), "{log}");
    }

    /// cas-fcb0: after a manual `kill -9` the daemon is gone but its workers,
    /// which lead their own sessions, are not. The follow-up `cas kill`
    /// reaps the groups the dead daemon recorded.
    #[test]
    fn stale_cleanup_reaps_the_dead_daemons_workers() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (worker_pgid, worker) = spawn_group("sleep", &["30"]);
        let (dead_pid, daemon) = spawn_group("true", &[]);
        daemon.join().unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        crate::ui::factory::process_groups::track(store.path(), "w1", "killed9", worker_pgid)
            .unwrap();
        let mut metadata = save_session("killed9", dead_pid, project.path());
        metadata.cas_root = Some(store.path().to_string_lossy().to_string());
        SessionManager::new().save_metadata(&metadata).unwrap();

        let outcome = end_session_by_name("killed9");
        let started = std::time::Instant::now();
        let reaped = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            kill_group(worker_pgid);
        });
        let worker_status = worker.join().unwrap().unwrap();
        let elapsed = started.elapsed();
        reaped.join().unwrap();
        assert_eq!(outcome.unwrap(), EndSessionOutcome::CleanedStale);
        assert_eq!(worker_status.signal(), Some(libc::SIGKILL));
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "the dead daemon's worker group was left running"
        );
        assert!(daemon_log("killed9").contains("\"worker_reaped\""));
    }

    /// cas-47037: a running daemon whose start time cannot be read is not
    /// known to be dead. `cas kill` must not reap its workers or drop its
    /// record; it says why it stopped and how to proceed.
    #[test]
    fn alive_daemon_with_unreadable_start_time_is_not_reaped() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (worker_pgid, worker) = spawn_group("sleep", &["30"]);
        let (pid, daemon) = spawn_group("sleep", &["30"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        crate::ui::factory::process_groups::track(store.path(), "w1", "opaque", worker_pgid)
            .unwrap();
        let mut metadata = save_session("opaque", pid, project.path());
        assert!(metadata.daemon_pid_starttime.is_some());
        metadata.cas_root = Some(store.path().to_string_lossy().to_string());
        SessionManager::new().save_metadata(&metadata).unwrap();

        let outcome = end_session_with("opaque", std::time::Duration::from_millis(300), &|_| None);
        let daemon_alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok();
        let worker_alive =
            nix::sys::signal::killpg(nix::unistd::Pid::from_raw(worker_pgid as i32), None).is_ok();
        let still_recorded = SessionManager::new().find_session(Some("opaque")).unwrap().is_some();
        kill_group(pid);
        kill_group(worker_pgid);
        daemon.join().unwrap().unwrap();
        worker.join().unwrap().unwrap();
        let error = outcome.unwrap_err().to_string();
        assert!(daemon_alive && worker_alive, "an alive daemon's worker was reaped");
        assert!(still_recorded, "an alive daemon's session record was removed");
        assert!(
            error.contains("start time could not be read")
                && error.contains(&format!("ps -p {pid}"))
                && error.contains("cas kill opaque --force-stale")
                && !error.contains("gc cleanup"),
            "every remedy offered must clear the session: {error}"
        );
        assert!(daemon_log("opaque").contains("\"worker_reap_skipped\""));
    }

    /// cas-47037: when the recorded PID runs another program, the operator
    /// clears the session with `cas kill <name> --force-stale`. The record
    /// goes, the recorded workers are reaped, and that PID is never signalled.
    #[test]
    fn force_stale_clears_the_session_without_signalling_its_pid() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (worker_pgid, worker) = spawn_group("sleep", &["30"]);
        let (other_pid, other) = spawn_group("sleep", &["30"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        crate::ui::factory::process_groups::track(store.path(), "w1", "reused", worker_pgid)
            .unwrap();
        let mut metadata = save_session("reused", other_pid, project.path());
        metadata.cas_root = Some(store.path().to_string_lossy().to_string());
        SessionManager::new().save_metadata(&metadata).unwrap();

        let outcome = force_clear_stale_session("reused", other_pid, &|_| None);
        let other_alive =
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(other_pid as i32), None).is_ok();
        let started = std::time::Instant::now();
        let stop_worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            kill_group(worker_pgid);
        });
        let worker_status = worker.join().unwrap().unwrap();
        let elapsed = started.elapsed();
        kill_group(other_pid);
        other.join().unwrap().unwrap();
        stop_worker.join().unwrap();
        assert_eq!(outcome.unwrap(), EndSessionOutcome::CleanedStale);
        assert!(other_alive, "--force-stale signalled the recorded PID");
        assert!(elapsed < std::time::Duration::from_secs(4), "the recorded worker was left running");
        assert_eq!(worker_status.signal(), Some(libc::SIGKILL));
        assert!(SessionManager::new().find_session(Some("reused")).unwrap().is_none());
        assert!(daemon_log("reused").contains("\"force_stale\""));
    }

    /// cas-47037: a record with no start time proves nothing about the
    /// running PID. `cas kill` withholds SIGKILL and points at --force-stale,
    /// so --force-stale must clear such a session instead of refusing it.
    #[test]
    fn force_stale_clears_a_session_recorded_without_a_start_time() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let (worker_pgid, worker) = spawn_group("sleep", &["30"]);
        let (other_pid, other) = spawn_group("sleep", &["30"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        crate::ui::factory::process_groups::track(store.path(), "w1", "unstamped", worker_pgid)
            .unwrap();
        let mut metadata = save_session("unstamped", other_pid, project.path());
        metadata.daemon_pid_starttime = None;
        metadata.cas_root = Some(store.path().to_string_lossy().to_string());
        SessionManager::new().save_metadata(&metadata).unwrap();

        let outcome = force_clear_stale_session(
            "unstamped",
            other_pid,
            &crate::mcp::daemon::read_pid_starttime,
        );
        let other_alive =
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(other_pid as i32), None).is_ok();
        let started = std::time::Instant::now();
        let stop_worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            kill_group(worker_pgid);
        });
        let worker_status = worker.join().unwrap().unwrap();
        let elapsed = started.elapsed();
        kill_group(other_pid);
        other.join().unwrap().unwrap();
        stop_worker.join().unwrap();
        assert_eq!(outcome.unwrap(), EndSessionOutcome::CleanedStale);
        assert!(other_alive, "--force-stale signalled the recorded PID");
        assert!(elapsed < std::time::Duration::from_secs(4), "the recorded worker was left running");
        assert_eq!(worker_status.signal(), Some(libc::SIGKILL));
        assert!(SessionManager::new().find_session(Some("unstamped")).unwrap().is_none());
    }

    /// cas-4b42: without a positive start-time match SIGKILL is withheld, and
    /// `cas kill` says so, with the manual remedy, instead of claiming the
    /// daemon survived SIGKILL.
    #[test]
    fn withheld_sigkill_is_reported_and_logged() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let (pid, daemon) = spawn_group("sh", &["-c", "trap '' TERM; exec sleep 30"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut metadata = save_session("unverified", pid, project.path());
        metadata.daemon_pid_starttime = None;
        SessionManager::new().save_metadata(&metadata).unwrap();

        let outcome = end_session_with_grace("unverified", std::time::Duration::from_millis(300));
        let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok();
        kill_group(pid);
        daemon.join().unwrap().unwrap();
        let error = outcome.unwrap_err().to_string();
        assert!(alive, "SIGKILL must not be sent without a start-time match");
        assert!(
            error.contains("SIGKILL was withheld because the session record has no daemon start time")
                && error.contains(&format!("kill -9 {pid}")),
            "{error}"
        );
        assert!(daemon_log("unverified").contains("\"kill_withheld\""));
    }

    /// cas-4b42: a replacement daemon that took the session name while the
    /// old one was being killed owns the workers now recorded under it; the
    /// reap must not touch them.
    #[test]
    fn replacement_daemon_workers_survive_the_old_daemons_kill() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let cas_root = project.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let (worker_pgid, worker) = spawn_group("sleep", &["30"]);
        std::thread::sleep(std::time::Duration::from_millis(100));
        crate::ui::factory::process_groups::track(&cas_root, "w1", "renamed", worker_pgid)
            .unwrap();
        // The replacement (this test process) takes the name when the old
        // daemon receives SIGTERM.
        let replacement = crate::ui::factory::create_metadata(
            "renamed",
            std::process::id(),
            "supervisor",
            &[],
            None,
            project.path().to_str(),
            None,
        );
        let staged = project.path().join("replacement.json");
        std::fs::write(&staged, serde_json::to_string(&replacement).unwrap()).unwrap();
        let script = format!(
            "trap 'cp \"{}\" \"{}\"' TERM; while :; do sleep 0.05; done",
            staged.display(),
            crate::ui::factory::metadata_path("renamed").display()
        );
        let (pid, daemon) = spawn_group("sh", &["-c", &script]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        save_session("renamed", pid, project.path());

        let outcome = end_session_with_grace("renamed", std::time::Duration::from_millis(500));
        kill_group(pid);
        daemon.join().unwrap().unwrap();
        let worker_alive =
            nix::sys::signal::killpg(nix::unistd::Pid::from_raw(worker_pgid as i32), None).is_ok();
        kill_group(worker_pgid);
        worker.join().unwrap().unwrap();
        assert_eq!(outcome.unwrap(), EndSessionOutcome::Ended);
        assert!(worker_alive, "the replacement daemon's worker was reaped");
        assert!(daemon_log("renamed").contains("\"worker_reap_skipped\""));
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
