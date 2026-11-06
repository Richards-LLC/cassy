//! Local factory daemon controls used by both the CLI and post-install update.
use crate::cli::Cli;
use crate::ui::factory::daemon::reexec::ReexecReceipt;
use crate::ui::factory::{
    ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE, decode_length,
    encode_message,
};
use crate::ui::factory::{SessionManager, gui_socket_path};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

fn read_frame(stream: &mut UnixStream) -> Result<DaemonMessage> {
    let mut header = [0; FRAME_HEADER_SIZE];
    stream.read_exact(&mut header)?;
    let length = decode_length(&header);
    ensure!(
        length <= MAX_MESSAGE_SIZE,
        "Daemon response exceeds the protocol limit"
    );
    let mut data = vec![0; length];
    stream.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}

pub(crate) fn request(session: &str, binary: &Path) -> Result<ReexecReceipt> {
    request_with_timeout(session, binary, Duration::from_secs(60))
}

fn request_with_timeout(session: &str, binary: &Path, _timeout: Duration) -> Result<ReexecReceipt> {
    let session_info = SessionManager::new()
        .list_sessions_read_only()?
        .into_iter()
        .find(|item| item.name == session && item.is_running)
        .with_context(|| format!("No live factory session named {session}"))?;
    let pid = session_info.metadata.daemon_pid;
    let mut stream = UnixStream::connect(gui_socket_path(session)).with_context(|| {
        format!("Could not connect to factory {session}; an older daemon may require relaunch")
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    ensure!(
        matches!(read_frame(&mut stream)?, DaemonMessage::Welcome { .. }),
        "Daemon did not send its welcome"
    );
    let nonce = uuid::Uuid::new_v4().to_string();
    stream.write_all(&encode_message(&ClientMessage::Reexec {
        binary: binary.to_path_buf(),
        version: env!("CARGO_PKG_VERSION").into(),
        nonce: nonce.clone(),
    })?)?;
    loop {
        match read_frame(&mut stream)? {
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

pub(crate) fn update_live_daemons() -> Vec<UpdateOutcome> {
    update_live_daemons_with_timeout(Duration::from_secs(60))
}

fn update_live_daemons_with_timeout(timeout: Duration) -> Vec<UpdateOutcome> {
    let sessions = match SessionManager::new().list_sessions_read_only() {
        Ok(sessions) => sessions,
        Err(error) => {
            return vec![UpdateOutcome {
                session: "discovery".into(),
                status: "failed",
                receipt: None,
                error: Some(error.to_string()),
            }];
        }
    };
    let binary = match std::env::current_exe() {
        Ok(binary) => binary,
        Err(error) => {
            return vec![UpdateOutcome {
                session: "binary".into(),
                status: "failed",
                receipt: None,
                error: Some(error.to_string()),
            }];
        }
    };
    sessions
        .into_iter()
        .filter(|session| session.is_running)
        .map(|session| {
            let build = option_env!("CAS_GIT_HASH").unwrap_or("unknown");
            if build != "unknown" && session.metadata.daemon_build_hash.as_deref() == Some(build) {
                return UpdateOutcome {
                    session: session.name,
                    status: "current",
                    receipt: None,
                    error: None,
                };
            }
            match request_with_timeout(&session.name, &binary, timeout) {
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
                    status: "failed",
                    receipt: None,
                    error: Some(format!("{error:#}")),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::factory::{SessionState, create_metadata, socket_path};
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

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
        let mut metadata = create_metadata(session, std::process::id(), "lead", &[], None,
            Some(project.to_str().unwrap()), None);
        metadata.daemon_build_hash = Some("old-daemon-without-reexec".into());
        manager.save_daemon_metadata(metadata, &root).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
            let welcome = DaemonMessage::Welcome {
                session_name: session.into(),
                state: SessionState { focused_pane: None, panes: vec![], epic_id: None,
                    epic_title: None, cols: 80, rows: 24 },
                scrollback: None, protocol_version: 1, capabilities: vec![], pane_bootstrap: vec![],
            };
            stream.write_all(&encode_message(&welcome).unwrap()).unwrap();
            // A real old parser consumes the unknown request without replying.
            let mut header = [0; FRAME_HEADER_SIZE];
            stream.read_exact(&mut header).unwrap();
            let mut body = vec![0; decode_length(&header)];
            stream.read_exact(&mut body).unwrap();
            assert!(serde_json::from_slice::<serde_json::Value>(&body).unwrap().get("Reexec").is_some());
            let output = encode_message(&DaemonMessage::Output {
                pane_id: "chatty".into(), data: b"still working\r\n".to_vec(),
            }).unwrap();
            let end = Instant::now() + Duration::from_millis(300);
            let mut frames = 0;
            while Instant::now() < end && stream.write_all(&output).is_ok() { frames += 1; }
            frames
        });
        let outcomes = update_live_daemons_with_timeout(Duration::from_millis(50));
        let frames = server.join().unwrap();
        assert!(frames > 0, "the old daemon must keep producing unrelated output");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, "relaunch_required", "{:?}", outcomes[0]);
        assert!(outcomes[0].error.as_deref().unwrap().contains("relaunch required"));
    }
}
