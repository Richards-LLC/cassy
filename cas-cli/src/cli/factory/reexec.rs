//! Local factory daemon controls used by both the CLI and post-install update.
use crate::cli::Cli;
use crate::ui::factory::daemon::reexec::ReexecReceipt;
use crate::ui::factory::{
    ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE, decode_length,
    encode_message,
};
use crate::ui::factory::{SessionInfo, SessionManager, gui_socket_path};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
#[error(
    "relaunch required: factory daemon did not answer the in-place update before the request deadline; it may be too old to support reexec"
)]
struct RelaunchRequired;

fn time_left(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| RelaunchRequired.into())
}

fn read_exact_before(
    stream: &mut UnixStream,
    mut data: &mut [u8],
    deadline: Instant,
) -> Result<()> {
    while !data.is_empty() {
        stream.set_read_timeout(Some(time_left(deadline)?))?;
        match stream.read(data) {
            Ok(0) => bail!("Daemon disconnected before completing its response"),
            Ok(n) => data = &mut data[n..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(RelaunchRequired.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn read_frame(stream: &mut UnixStream, deadline: Instant) -> Result<DaemonMessage> {
    let mut header = [0; FRAME_HEADER_SIZE];
    read_exact_before(stream, &mut header, deadline)?;
    let length = decode_length(&header);
    ensure!(
        length <= MAX_MESSAGE_SIZE,
        "Daemon response exceeds the protocol limit"
    );
    let mut data = vec![0; length];
    read_exact_before(stream, &mut data, deadline)?;
    Ok(serde_json::from_slice(&data)?)
}

pub(crate) fn request(session: &str, binary: &Path) -> Result<ReexecReceipt> {
    request_with_timeout(session, binary, Duration::from_secs(60))
}

fn request_with_timeout(session: &str, binary: &Path, timeout: Duration) -> Result<ReexecReceipt> {
    let deadline = Instant::now() + timeout;
    let session_info = SessionManager::new()
        .list_sessions_read_only()?
        .into_iter()
        .find(|item| item.name == session && item.is_running)
        .with_context(|| format!("No live factory session named {session}"))?;
    let pid = session_info.metadata.daemon_pid;
    let mut stream = UnixStream::connect(gui_socket_path(session)).with_context(|| {
        format!("Could not connect to factory {session}; an older daemon may require relaunch")
    })?;
    ensure!(
        matches!(
            read_frame(&mut stream, deadline)?,
            DaemonMessage::Welcome { .. }
        ),
        "Daemon did not send its welcome"
    );
    let nonce = uuid::Uuid::new_v4().to_string();
    let frame = encode_message(&ClientMessage::Reexec {
        binary: binary.to_path_buf(),
        version: env!("CARGO_PKG_VERSION").into(),
        nonce: nonce.clone(),
    })?;
    let mut pending = frame.as_slice();
    while !pending.is_empty() {
        stream.set_write_timeout(Some(time_left(deadline)?))?;
        match stream.write(pending) {
            Ok(0) => bail!("Daemon disconnected before receiving the request"),
            Ok(n) => pending = &pending[n..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(RelaunchRequired.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    loop {
        match read_frame(&mut stream, deadline)? {
            DaemonMessage::ReexecResult { receipt } if receipt.nonce == nonce => {
                ensure!(
                    receipt.pid == pid,
                    "Daemon changed PID during reexec: {pid} -> {}",
                    receipt.pid
                );
                return Ok(receipt);
            }
            DaemonMessage::Error {
                message,
                client_ref,
            } if client_ref.as_deref() == Some(&nonce) => bail!("{message}"),
            _ => {}
        }
    }
}

pub(super) fn execute(session: Option<&str>, cli: &Cli) -> Result<()> {
    let manager = SessionManager::new();
    let session = manager
        .find_session(session)?
        .context("No running factory session")?;
    let binary = std::env::current_exe()?;
    let receipt = request(&session.name, &binary)?;
    if cli.json {
        println!("{}", serde_json::to_string(&receipt)?);
    } else {
        println!(
            "Factory {}: {} -> {}; PID {}; {} panes adopted{}",
            receipt.session,
            receipt.old_version,
            receipt.version,
            receipt.pid,
            receipt.panes_adopted,
            if receipt.fallback {
                "; previous build restored"
            } else {
                ""
            }
        );
    }
    ensure!(
        !receipt.fallback,
        "Candidate failed before adoption; previous daemon restored"
    );
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdateOutcome {
    pub session: String,
    pub status: &'static str,
    pub receipt: Option<ReexecReceipt>,
    pub error: Option<String>,
}

/// cas-e1e9: `cas update` re-execs each running factory session right after
/// its own project's store is refreshed, so one slow project never delays
/// another session. Sessions not handled per project re-exec in [`finish`].
///
/// [`finish`]: LiveDaemonUpdate::finish
pub(crate) struct LiveDaemonUpdate {
    timeout: Duration,
    handled: std::collections::BTreeSet<String>,
    outcomes: Vec<UpdateOutcome>,
}

impl LiveDaemonUpdate {
    pub(crate) fn new() -> Self {
        Self::with_timeout(Duration::from_secs(60))
    }

    fn with_timeout(timeout: Duration) -> Self {
        Self { timeout, handled: Default::default(), outcomes: Vec::new() }
    }

    /// Re-exec the running sessions whose store is `cas_root`. Call only
    /// after that store refreshed; a listing failure is reported by `finish`.
    pub(crate) fn after_project(&mut self, cas_root: &Path) {
        let root = cas_root.canonicalize().unwrap_or_else(|_| cas_root.to_path_buf());
        let _ = self.run(|session| {
            let store = session.metadata.cas_root.as_ref().map(PathBuf::from).or_else(|| {
                session.metadata.project_dir.as_ref().map(|dir| Path::new(dir).join(".cas"))
            });
            store.is_some_and(|store| store.canonicalize().unwrap_or(store) == root)
        });
    }

    /// Re-exec every remaining running session, including those whose project
    /// failed to refresh, and return one outcome per session.
    pub(crate) fn finish(mut self) -> Vec<UpdateOutcome> {
        if let Err(outcome) = self.run(|_| true) {
            self.outcomes.push(outcome);
        }
        self.outcomes
    }

    fn run(&mut self, select: impl Fn(&SessionInfo) -> bool) -> Result<(), UpdateOutcome> {
        let failed = |session: &str, error: String| UpdateOutcome {
            session: session.into(),
            status: "failed",
            receipt: None,
            error: Some(error),
        };
        let sessions = SessionManager::new()
            .list_sessions_read_only()
            .map_err(|error| failed("discovery", error.to_string()))?;
        let binary = std::env::current_exe().map_err(|error| failed("binary", error.to_string()))?;
        for session in sessions {
            if session.is_running && !self.handled.contains(&session.name) && select(&session) {
                self.handled.insert(session.name.clone());
                self.outcomes.push(update_one(session, &binary, self.timeout));
            }
        }
        Ok(())
    }
}

fn update_one(session: SessionInfo, binary: &Path, timeout: Duration) -> UpdateOutcome {
    let build = option_env!("CAS_GIT_HASH").unwrap_or("unknown");
    if build != "unknown" && session.metadata.daemon_build_hash.as_deref() == Some(build) {
        return UpdateOutcome {
            session: session.name,
            status: "current",
            receipt: None,
            error: None,
        };
    }
    match request_with_timeout(&session.name, binary, timeout) {
        Ok(receipt) => UpdateOutcome {
            session: session.name,
            status: if receipt.fallback {
                "restored_previous"
            } else {
                "updated"
            },
            error: receipt.error.clone(),
            receipt: Some(receipt),
        },
        Err(error) => UpdateOutcome {
            session: session.name,
            status: if error.is::<RelaunchRequired>() {
                "relaunch_required"
            } else {
                "failed"
            },
            receipt: None,
            error: Some(format!("{error:#}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::factory::{SessionState, create_metadata, socket_path};
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    /// cas-e1e9: prowl's sessions waited ~5 min behind 31 project refreshes.
    /// A fast project's session re-execs while a slow project is still
    /// refreshing, the slow one's right after its own, and each exactly once.
    #[test]
    fn each_session_reexecs_right_after_its_own_project_refresh() {
        let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
        let short = tempfile::tempdir_in("/tmp").unwrap();
        let home = short.path().join("h");
        std::os::unix::fs::symlink(env.home(), &home).unwrap();
        env.set("HOME", &home);
        let manager = SessionManager::new();
        manager.ensure_dir().unwrap();
        let mut roots = Vec::new();
        for name in ["fast", "slow"] {
            let project = env.home().join(name);
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let mut metadata = create_metadata(
                name, std::process::id(), "lead", &[], None, Some(project.to_str().unwrap()), None,
            );
            metadata.daemon_build_hash = Some("previous-build".into());
            manager.save_daemon_metadata(metadata, &root).unwrap();
            roots.push(root);
        }
        let sessions = |outcomes: &[UpdateOutcome]| {
            outcomes.iter().map(|outcome| outcome.session.clone()).collect::<Vec<_>>()
        };
        let mut live = LiveDaemonUpdate::with_timeout(Duration::from_millis(50));
        live.after_project(&roots[0]);
        // The slow project's refresh is still running here.
        assert_eq!(sessions(&live.outcomes), ["fast"]);
        live.after_project(&roots[1]);
        assert_eq!(sessions(&live.outcomes), ["fast", "slow"]);
        assert_eq!(sessions(&live.finish()), ["fast", "slow"]);
    }

    #[test]
    fn update_returns_relaunch_required_for_a_chatty_old_daemon() {
        let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
        let short = tempfile::tempdir_in("/tmp").unwrap();
        let home = short.path().join("h");
        std::os::unix::fs::symlink(env.home(), &home).unwrap();
        env.set("HOME", &home);
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        let manager = SessionManager::new();
        manager.ensure_dir().unwrap();
        let session = "chatty-old";
        let _main = UnixListener::bind(socket_path(session)).unwrap();
        let listener = UnixListener::bind(gui_socket_path(session)).unwrap();
        let mut metadata = create_metadata(
            session,
            std::process::id(),
            "lead",
            &[],
            None,
            Some(project.to_str().unwrap()),
            None,
        );
        metadata.daemon_build_hash = Some("old-daemon-without-reexec".into());
        manager.save_daemon_metadata(metadata, &root).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let welcome = DaemonMessage::Welcome {
                session_name: session.into(),
                state: SessionState {
                    focused_pane: None,
                    panes: vec![],
                    epic_id: None,
                    epic_title: None,
                    cols: 80,
                    rows: 24,
                },
                scrollback: None,
                protocol_version: 1,
                capabilities: vec![],
                pane_bootstrap: vec![],
            };
            stream
                .write_all(&encode_message(&welcome).unwrap())
                .unwrap();
            // A real old parser consumes the unknown request without replying.
            let mut header = [0; FRAME_HEADER_SIZE];
            stream.read_exact(&mut header).unwrap();
            let mut body = vec![0; decode_length(&header)];
            stream.read_exact(&mut body).unwrap();
            assert!(
                serde_json::from_slice::<serde_json::Value>(&body)
                    .unwrap()
                    .get("Reexec")
                    .is_some()
            );
            let output = encode_message(&DaemonMessage::Output {
                pane_id: "chatty".into(),
                data: b"still working\r\n".to_vec(),
            })
            .unwrap();
            let end = Instant::now() + Duration::from_millis(300);
            let mut frames = 0;
            while Instant::now() < end && stream.write_all(&output).is_ok() {
                frames += 1;
            }
            frames
        });
        let outcomes = LiveDaemonUpdate::with_timeout(Duration::from_millis(50)).finish();
        let frames = server.join().unwrap();
        assert!(
            frames > 0,
            "the old daemon must keep producing unrelated output"
        );
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, "relaunch_required", "{:?}", outcomes[0]);
        assert!(
            outcomes[0]
                .error
                .as_deref()
                .unwrap()
                .contains("relaunch required")
        );
    }
}
