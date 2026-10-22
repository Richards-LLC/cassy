//! Strict read-only inventory and framed Unix intake preflight. No lifecycle
//! cleanup, guessed liveness, remote controls or PATH-based capability claims.
use super::binding_intake::{IntakeScope, PauseRecord};
use crate::ui::factory::daemon::runtime::violet_intake::{PauseRequest, RuntimeIdentity};
use crate::ui::factory::{
    ClientMessage, DaemonMessage, MAX_MESSAGE_SIZE, ProtocolCapability, SessionMetadata,
    encode_message, gui_socket_path, sessions_dir,
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct RuntimeProbe {
    identity: RuntimeIdentity,
    socket: PathBuf,
}
pub(super) struct RuntimeInventory {
    probes: Vec<RuntimeProbe>,
}
impl RuntimeInventory {
    /// A missing registry is an empty inventory; malformed entries are unknown,
    /// never silently omitted as they are in the display-oriented session list.
    pub fn discover(cas_dir: &Path) -> Result<Self, String> {
        let root = cas_dir.canonicalize().map_err(|e| e.to_string())?;
        let directory = sessions_dir();
        let mut probes = Vec::new();
        let mut recorded_sockets = std::collections::HashSet::new();
        let entries =
            match fs::read_dir(&directory) {
                Ok(entries) => Some(entries),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err(
                    "runtime inventory unavailable; inspect factory sessions before reconciliation"
                        .into(),
                ),
            };
        if let Some(entries) = entries {
            for entry in entries {
                let path = entry.map_err(|_| "runtime inventory unreadable")?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let metadata: SessionMetadata = serde_json::from_slice(
                    &fs::read(&path).map_err(|_| "runtime metadata unreadable")?,
                )
                .map_err(
                    |_| "corrupt runtime metadata; inspect factory sessions before reconciliation",
                )?;
                if metadata.name.is_empty()
                    || metadata.name.contains(['/', '\\'])
                    || metadata.name == "."
                    || metadata.name == ".."
                    || metadata.daemon_pid == 0
                    || metadata.daemon_pid > i32::MAX as u32
                {
                    return Err(
                        "unknown runtime identity; inspect factory sessions before reconciliation"
                            .into(),
                    );
                }
                let socket = gui_socket_path(&metadata.name);
                recorded_sockets.insert(socket.clone());
                // Attribute the record before any process/socket refusal: the
                // registry is machine-wide, but this drain is project-scoped.
                if let Some(project) = metadata.project_dir.as_deref() {
                    let session_root = crate::store::find_cas_root_ignoring_env(Path::new(project))
                        .map_err(|_| "runtime project root is unresolved")?
                        .canonicalize()
                        .map_err(|_| "runtime project root unreadable")?;
                    if session_root != root {
                        continue;
                    }
                }
                if !crate::mcp::daemon::pid_alive(metadata.daemon_pid) {
                    if !inactive_endpoint(&socket) {
                        return Err(endpoint_refusal("dead", &metadata.name)?);
                    }
                    continue;
                }
                let actual = crate::mcp::daemon::read_pid_starttime(metadata.daemon_pid);
                if matches!((metadata.daemon_pid_starttime, actual), (Some(expected), Some(actual)) if expected != actual)
                {
                    if !inactive_endpoint(&socket) {
                        return Err(endpoint_refusal("stale", &metadata.name)?);
                    }
                    continue;
                }
                if metadata.daemon_pid_starttime.is_none() || actual.is_none() {
                    return Err("runtime process start identity is unknown".into());
                }
                if metadata.project_dir.is_none() {
                    return Err("running runtime has no project identity".into());
                }
                let identity = RuntimeIdentity {
                    canonical_cas_dir: root.clone(),
                    factory_session: metadata.name,
                    pid: metadata.daemon_pid,
                    pid_starttime: metadata
                        .daemon_pid_starttime
                        .ok_or("runtime process start is unknown")?,
                    build: metadata
                        .daemon_build_hash
                        .ok_or("runtime build is unknown")?,
                };
                identity.validate()?;
                probes.push(RuntimeProbe { identity, socket });
            }
        }
        // An unrecorded GUI endpoint cannot be attributed to an unrelated root.
        // Probe it without unlinking: macOS clean exits can leave inactive
        // pathnames even after their metadata has been removed.
        if let Some(base) = directory.parent() {
            match fs::read_dir(base) {
                Ok(entries) => {
                    for entry in entries {
                        let path = entry
                            .map_err(|_| "runtime socket inventory unreadable")?
                            .path();
                        if path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.starts_with("factory-") && name.ends_with(".gui.sock")
                            })
                            && !recorded_sockets.contains(&path)
                            && !inactive_endpoint(&path)
                        {
                            return Err("unattributed factory GUI socket; inspect factory sessions before reconciliation".into());
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("runtime socket inventory unavailable".into()),
            }
        }
        Ok(Self { probes })
    }
    pub fn drain(&self, scope: &IntakeScope, pause: &PauseRecord) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        for probe in &self.probes {
            probe.identity.validate()?;
            let path = probe.socket.clone();
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("runtime preflight deadline expired")?;
            let mut stream = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "local control connector unavailable")?;
                runtime.block_on(async {
                    tokio::time::timeout(remaining, tokio::net::UnixStream::connect(path))
                        .await
                        .map_err(|_| "runtime Unix connect timed out")?
                        .map_err(|_| "runtime Unix control unavailable")?
                        .into_std()
                        .map_err(|_| "runtime Unix control unavailable")
                })
            })
            .join()
            .map_err(|_| "runtime Unix connector failed")??;
            stream
                .set_nonblocking(false)
                .map_err(|_| "runtime Unix mode unavailable")?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|e| e.to_string())?;
            let capabilities = match receive(&mut stream, deadline)? {
                DaemonMessage::Welcome {
                    session_name,
                    capabilities,
                    ..
                } if session_name == probe.identity.factory_session => capabilities,
                _ => return Err("runtime did not negotiate a matching local Welcome".into()),
            };
            if !capabilities.contains(&ProtocolCapability::BindingIntakeControlV1) {
                return Err("old runtime lacks binding_intake_control_v1; coordinate runtime upgrade before reconciliation".into());
            }
            let request = PauseRequest {
                operation_id: pause.operation_id,
                nonce: uuid::Uuid::new_v4(),
                canonical_cas_dir: probe.identity.canonical_cas_dir.clone(),
                factory_session: probe.identity.factory_session.clone(),
                scope: scope.clone(),
                expected_generation: pause
                    .generation
                    .checked_sub(1)
                    .ok_or("invalid pause generation")?,
            };
            stream
                .write_all(
                    &encode_message(&ClientMessage::BindingIntakePause {
                        request: request.clone(),
                    })
                    .map_err(|e| e.to_string())?,
                )
                .map_err(|_| "runtime control write failed")?;
            // A GUI client also receives every broadcast (pane output, state).
            // Skip anything that is not this control's reply until the deadline.
            let reply = loop {
                if let Ok(DaemonMessage::BindingIntake { reply }) =
                    receive_frame(&mut stream, deadline)?
                {
                    break reply;
                }
            };
            if reply.nonce != request.nonce {
                return Err("runtime drain nonce mismatch".into());
            }
            if let Some(error) = reply.error {
                return Err(error);
            }
            let ack = reply
                .acknowledgement
                .ok_or("runtime returned status without a drain acknowledgement")?;
            ack.verify(&request, &probe.identity, &capabilities)?;
            let state = reply
                .state
                .ok_or("runtime drain omitted durable gate state")?;
            if !state.pauses.contains(pause) {
                return Err("runtime drain did not confirm this durable pause".into());
            }
        }
        Ok(())
    }
}
/// Probe only; never unlink a path based on a refused connection. A listener,
/// timeout, permission error or unavailable connector cannot prove inactivity.
/// A short bound prevents a full backlog from stalling every project's update.
fn inactive_endpoint(path: &Path) -> bool {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return false;
        };
        runtime.block_on(async {
            matches!(
                tokio::time::timeout(
                    Duration::from_millis(250),
                    tokio::net::UnixStream::connect(path),
                )
                .await,
                Ok(Err(error)) if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                )
            )
        })
    })
    .join()
    .unwrap_or(false)
}

fn endpoint_refusal(state: &str, session: &str) -> Result<String, String> {
    let [remedy] = crate::worktree::GitOperations::shell_remedies([("cas", &["kill", session])])?;
    Ok(format!(
        "{state} session {} still has a held or unknown GUI endpoint; run `{remedy}`",
        crate::worktree::GitOperations::visible_git_label(session),
    ))
}

fn receive(stream: &mut UnixStream, deadline: Instant) -> Result<DaemonMessage, String> {
    receive_frame(stream, deadline)?
        .map_err(|_| "unknown/corrupt runtime control response".into())
}

/// One complete frame. The outer error is transport/deadline; the inner one
/// is a frame this build cannot parse (skippable while awaiting a reply).
fn receive_frame(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<Result<DaemonMessage, serde_json::Error>, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("runtime drain deadline expired")?;
    let _ = remaining;
    let mut header = [0; 4];
    read_exact_until(stream, &mut header, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_MESSAGE_SIZE {
        return Err("invalid runtime control frame length".into());
    }
    let mut bytes = vec![0; length];
    read_exact_until(stream, &mut bytes, deadline)?;
    Ok(serde_json::from_slice(&bytes))
}

fn read_exact_until(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), String> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("runtime drain deadline expired")?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let read = stream
            .read(bytes)
            .map_err(|_| "runtime drain read failed/timed out")?;
        if read == 0 {
            return Err("runtime control closed before complete drain".into());
        }
        bytes = &mut bytes[read..];
    }
    Ok(())
}
