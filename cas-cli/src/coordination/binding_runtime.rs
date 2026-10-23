//! Run-owned runtime inventory and framed Unix intake preflight. Ordinary
//! repair is read-only; update may reap independently fenced dead receipts.
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
#[derive(Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl FileIdentity {
    fn read(path: &Path) -> std::io::Result<Option<Self>> {
        use std::os::unix::fs::MetadataExt;
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(Self {
                device: meta.dev(),
                inode: meta.ino(),
                length: meta.len(),
                modified: (meta.mtime(), meta.mtime_nsec()),
                changed: (meta.ctime(), meta.ctime_nsec()),
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}
struct EndpointObservation {
    identity: Option<FileIdentity>,
    inactive: bool,
}
struct RuntimeRecord {
    metadata: SessionMetadata,
    project_root: Option<PathBuf>,
}
struct Snapshot {
    records: Vec<RuntimeRecord>,
    endpoints: std::collections::HashMap<PathBuf, EndpointObservation>,
    orphans: Vec<PathBuf>,
    directory: PathBuf,
    directory_identity: Option<FileIdentity>,
    revision: Option<Vec<u8>>,
    files: Vec<(PathBuf, FileIdentity)>,
}
/// One machine scan owned by a single update invocation, never a global cache.
/// Keep a failed scan too, so each binding reports it without rescanning.
pub(crate) struct RuntimeSnapshot(Result<Snapshot, String>);
impl RuntimeSnapshot {
    pub(crate) fn capture(reap: bool) -> Self {
        Self(Snapshot::scan(reap))
    }
    pub(super) fn for_project(&self, cas_dir: &Path) -> Result<RuntimeInventory, String> {
        let snapshot = self.0.as_ref().map_err(Clone::clone)?;
        snapshot.validate()?;
        let root = cas_dir.canonicalize().map_err(|error| error.to_string())?;
        let mut probes = Vec::new();
        for record in &snapshot.records {
            let metadata = &record.metadata;
            let socket = gui_socket_path(&metadata.name);
            if metadata.project_dir.is_some() {
                match &record.project_root {
                    Some(session_root) if session_root != &root => continue,
                    None if crate::mcp::daemon::pid_alive(metadata.daemon_pid) => {
                        let [remedy] = crate::worktree::GitOperations::shell_remedies([(
                            "cas",
                            &["kill", "--", metadata.name.as_str()],
                        )])?;
                        return Err(format!(
                            "runtime project root is unresolved for live session {}; run `{remedy}`",
                            crate::worktree::GitOperations::visible_git_label(&metadata.name),
                        ));
                    }
                    _ => {}
                }
            }
            if !crate::mcp::daemon::pid_alive(metadata.daemon_pid) {
                if !snapshot.inactive(&socket) {
                    return Err(endpoint_refusal("dead", &metadata.name)?);
                }
                continue;
            }
            let actual = crate::mcp::daemon::read_pid_starttime(metadata.daemon_pid);
            if matches!((metadata.daemon_pid_starttime, actual), (Some(expected), Some(actual)) if expected != actual)
            {
                if !snapshot.inactive(&socket) {
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
                factory_session: metadata.name.clone(),
                pid: metadata.daemon_pid,
                pid_starttime: metadata
                    .daemon_pid_starttime
                    .ok_or("runtime process start is unknown")?,
                build: metadata
                    .daemon_build_hash
                    .clone()
                    .ok_or("runtime build is unknown")?,
            };
            identity.validate()?;
            probes.push(RuntimeProbe { identity, socket });
        }
        for path in &snapshot.orphans {
            if !snapshot.inactive(path) {
                return Err("unattributed factory GUI socket; inspect factory sessions before reconciliation".into());
            }
        }
        Ok(RuntimeInventory { probes })
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.0.as_ref().map_err(Clone::clone)?.validate()
    }
}
impl Snapshot {
    fn revision(directory: &Path) -> std::io::Result<Option<Vec<u8>>> {
        match fs::read(
            directory
                .parent()
                .ok_or_else(|| std::io::Error::other("missing session base"))?
                .join("factory-sockets.lock"),
        ) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn scan(reap: bool) -> Result<Self, String> {
        use crate::ui::factory::SessionManager;
        let directory = sessions_dir();
        let manager = SessionManager::new();
        let _transition = manager
            .lock_inventory(reap)
            .map_err(|_| "runtime inventory lock unavailable")?;
        let original_directory =
            FileIdentity::read(&directory).map_err(|_| "runtime inventory unavailable")?;
        let original_revision =
            Self::revision(&directory).map_err(|_| "runtime inventory revision unavailable")?;
        let mut snapshot = Self {
            records: Vec::new(),
            endpoints: std::collections::HashMap::new(),
            orphans: Vec::new(),
            directory: directory.clone(),
            directory_identity: None,
            revision: None,
            files: Vec::new(),
        };
        let mut recorded = std::collections::HashSet::new();
        let mut first_error = None;
        let entries =
            match fs::read_dir(&directory) {
                Ok(entries) => Some(entries),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err(
                    "runtime inventory unavailable; inspect factory sessions before reconciliation"
                        .into(),
                ),
            };
        for entry in entries.into_iter().flatten() {
            let path = entry.map_err(|_| "runtime inventory unreadable")?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let bytes = fs::read(&path).map_err(|_| "runtime metadata unreadable")?;
            let metadata: SessionMetadata = match serde_json::from_slice(&bytes) {
                Ok(metadata) => metadata,
                Err(_) => {
                    first_error = Some(
                        "corrupt runtime metadata; inspect factory sessions before reconciliation",
                    );
                    continue;
                }
            };
            if metadata.name.is_empty()
                || metadata.name.contains(['/', '\\'])
                || metadata.name == "."
                || metadata.name == ".."
                || metadata.daemon_pid == 0
                || metadata.daemon_pid > i32::MAX as u32
            {
                first_error = Some(
                    "unknown runtime identity; inspect factory sessions before reconciliation",
                );
                continue;
            }
            let socket = gui_socket_path(&metadata.name);
            recorded.insert(socket.clone());
            let dead = !crate::mcp::daemon::pid_alive(metadata.daemon_pid);
            let stale = matches!((metadata.daemon_pid_starttime, crate::mcp::daemon::read_pid_starttime(metadata.daemon_pid)), (Some(expected), Some(actual)) if expected != actual);
            if dead || stale {
                snapshot.observe(&socket);
            }
            if reap && dead && snapshot.inactive(&socket) {
                let identity = snapshot
                    .endpoints
                    .get(&socket)
                    .and_then(|probe| probe.identity.as_ref())
                    .map(|id| (id.device, id.inode));
                match manager.reap_dead_record_locked(&path, &bytes, identity) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => tracing::warn!(%error, "dead factory metadata retained"),
                }
            }
            let identity = FileIdentity::read(&path)
                .map_err(|_| "runtime metadata unreadable")?
                .ok_or("runtime metadata disappeared")?;
            snapshot.files.push((path, identity));
            let project_root = metadata.project_dir.as_deref().and_then(|project| {
                crate::store::find_cas_root_ignoring_env(Path::new(project))
                    .ok()?
                    .canonicalize()
                    .ok()
            });
            snapshot.records.push(RuntimeRecord {
                metadata,
                project_root,
            });
        }
        if let Some(base) = directory.parent() {
            let entries = match fs::read_dir(base) {
                Ok(entries) => Some(entries),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err("runtime socket inventory unavailable".into()),
            };
            for entry in entries.into_iter().flatten() {
                let path = entry
                    .map_err(|_| "runtime socket inventory unreadable")?
                    .path();
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("factory-") && name.ends_with(".gui.sock"))
                    && !recorded.contains(&path)
                {
                    snapshot.observe(&path);
                    snapshot.orphans.push(path);
                }
            }
        }
        snapshot.directory_identity =
            FileIdentity::read(&directory).map_err(|_| "runtime inventory unavailable")?;
        snapshot.revision =
            Self::revision(&directory).map_err(|_| "runtime inventory revision unavailable")?;
        if !reap
            && (snapshot.directory_identity != original_directory
                || snapshot.revision != original_revision)
        {
            return Err("factory sessions changed during inventory; rerun `cas update`".into());
        }
        if let Some(error) = first_error {
            return Err(error.into());
        }
        Ok(snapshot)
    }
    fn observe(&mut self, path: &Path) {
        self.endpoints.entry(path.to_path_buf()).or_insert_with(|| {
            let before = FileIdentity::read(path);
            let inactive = inactive_endpoint(path);
            let after = FileIdentity::read(path);
            let unchanged = matches!((&before, &after), (Ok(before), Ok(after)) if before == after);
            EndpointObservation {
                identity: after.ok().flatten(),
                inactive: inactive && unchanged,
            }
        });
    }
    fn inactive(&self, path: &Path) -> bool {
        self.endpoints.get(path).is_some_and(|probe| {
            probe.inactive
                && FileIdentity::read(path).is_ok_and(|current| current == probe.identity)
        })
    }
    fn validate(&self) -> Result<(), String> {
        let unchanged = FileIdentity::read(&self.directory)
            .is_ok_and(|value| value == self.directory_identity)
            && Self::revision(&self.directory).is_ok_and(|value| value == self.revision)
            && self.files.iter().all(|(path, expected)| {
                FileIdentity::read(path).is_ok_and(|value| value.as_ref() == Some(expected))
            });
        if unchanged {
            Ok(())
        } else {
            Err("factory sessions changed since inventory; rerun `cas update`".into())
        }
    }
}
impl RuntimeInventory {
    /// Test convenience for the ordinary read-only discovery contract.
    #[cfg(test)]
    pub fn discover(cas_dir: &Path) -> Result<Self, String> {
        RuntimeSnapshot::capture(false).for_project(cas_dir)
    }
    pub fn drain(&self, scope: &IntakeScope, pause: &PauseRecord) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        for probe in &self.probes {
            probe.identity.validate()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("runtime preflight deadline expired")?;
            let mut stream = connect_bounded(&probe.socket, remaining)
                .map_err(|_| "runtime Unix control unavailable")?;
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
    matches!(connect_bounded(path, Duration::from_millis(250)), Err(error)
        if matches!(error.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound))
}

/// One deadline-bound blocking call, without a thread or async runtime. A
/// full Unix backlog (EAGAIN), permission error or timeout remains unknown.
fn connect_bounded(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    use std::io;
    use std::os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is plain C data; all fields are initialized below.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Unix socket path",
        ));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as _;
    }
    let length =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    #[cfg(target_os = "macos")]
    {
        address.sun_len = length as _;
    }
    // SAFETY: socket returns a new owned descriptor or -1.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is a newly created descriptor, transferred exactly once.
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: fcntl operates on our live descriptor; flags need no pointer.
    if unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
        || unsafe { libc::fcntl(raw, libc::F_SETFL, libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let deadline = Instant::now() + timeout;
    // SAFETY: address is initialized and valid for length bytes.
    if unsafe { libc::connect(raw, (&address as *const libc::sockaddr_un).cast(), length) } < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut poll = libc::pollfd {
                fd: descriptor.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: poll points at one initialized entry for a bounded wait.
            let ready = unsafe { libc::poll(&mut poll, 1, millis) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let mut error: libc::c_int = 0;
            let mut size = std::mem::size_of_val(&error) as libc::socklen_t;
            // SAFETY: SO_ERROR writes one c_int into our initialized buffer.
            if unsafe {
                libc::getsockopt(
                    raw,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut error as *mut libc::c_int).cast(),
                    &mut size,
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
            break;
        }
    }
    let stream = UnixStream::from(descriptor);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn endpoint_refusal(state: &str, session: &str) -> Result<String, String> {
    let [remedy] =
        crate::worktree::GitOperations::shell_remedies([("cas", &["kill", "--", session])])?;
    Ok(format!(
        "{state} session {} still has a held or unknown GUI endpoint; run `{remedy}`",
        crate::worktree::GitOperations::visible_git_label(session),
    ))
}

fn receive(stream: &mut UnixStream, deadline: Instant) -> Result<DaemonMessage, String> {
    receive_frame(stream, deadline)?.map_err(|_| "unknown/corrupt runtime control response".into())
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
