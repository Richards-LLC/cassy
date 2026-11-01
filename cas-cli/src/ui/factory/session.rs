//! Factory session management
//!
//! Handles daemon discovery for factory sessions. This module provides
//! functionality to find running factory daemons by checking PIDs and sockets.
//!
//! For unified session metadata (worker count, epic ID, etc.), use
//! `cas_factory::SessionSummary`.

use crate::store::{find_cas_root_from, find_cas_root_ignoring_env, open_agent_store};
use crate::ui::factory::protocol::{AgentInfo, SessionMetadata};
use cas_factory::{SessionState, SessionSummary, SessionType};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

mod socket_cleanup;

/// Confirmed teardown outcome retained after active metadata is removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionEndOutcome {
    pub at: String,
    pub reason: String,
    pub workers_stopped: Vec<String>,
    pub orphans: Vec<String>,
    /// Missing ownership evidence must never count as a successful stop.
    #[serde(default)]
    pub unverified: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct EndedSession {
    pub metadata: SessionMetadata,
    pub ended: SessionEndOutcome,
}

fn same_session_birth(a: &SessionMetadata, b: &SessionMetadata) -> bool {
    a.name == b.name && a.created_at == b.created_at && a.daemon_pid == b.daemon_pid
        && a.daemon_pid_starttime == b.daemon_pid_starttime
}

fn write_end_receipt(sessions: &Path, receipt: &EndedSession) -> std::io::Result<()> {
    let dir = sessions.join("ended");
    fs::create_dir_all(&dir)?;
    let temporary = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(temporary.as_file(), receipt)?;
    temporary.as_file().sync_all()?;
    temporary.persist(dir.join(format!("{}.json", receipt.metadata.name)))?;
    Ok(())
}

/// Called under the namespace lock, before metadata is unlinked. Retain a
/// richer shutdown receipt for this birth; never attribute an old stop to a
/// replacement by loading the name again after ownership changes.
fn retain_end_receipt(sessions: &Path, metadata: &SessionMetadata) -> std::io::Result<()> {
    match fs::read(sessions.join("ended").join(format!("{}.json", metadata.name))) {
        Ok(bytes) => {
            if let Ok(receipt) = serde_json::from_slice::<EndedSession>(&bytes)
                && same_session_birth(&receipt.metadata, metadata) {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_end_receipt(sessions, &EndedSession {
        metadata: metadata.clone(),
        ended: SessionEndOutcome {
            at: Utc::now().to_rfc3339(), reason: "metadata_removed".into(),
            workers_stopped: Vec::new(), orphans: Vec::new(),
            // Metadata removal proves the session ended, not its workers.
            unverified: metadata.workers.iter().map(|worker| worker.name.clone()).collect(),
        },
    })
}

/// Match end evidence to the daemon's owned supervisor generation.
pub(crate) fn supervisor_end_is_current(
    metadata: &SessionMetadata,
    owned_pid: Option<u32>,
    recorded_pid: Option<u32>,
    recorded_starttime: Option<u64>,
    registered_at: &str,
) -> bool {
    if let Some(owned_pid) = owned_pid {
        if recorded_pid != Some(owned_pid) {
            return false;
        }
        if let Some((supervisor_start, daemon_start)) = recorded_starttime.zip(metadata.daemon_pid_starttime) {
            // SessionStart can finish before metadata is written. The owned
            // PTY's PID and boot-relative birth identify its generation without
            // depending on the wall-clock ordering of those two writes.
            return supervisor_start >= daemon_start;
        }
    }
    // Older receipts without process fingerprints retain the conservative
    // timestamp check; missing evidence must never borrow an earlier reason.
    chrono::DateTime::parse_from_rfc3339(registered_at).ok().zip(
        chrono::DateTime::parse_from_rfc3339(&metadata.created_at).ok())
        .is_some_and(|(registered, created)| registered >= created)
}

/// Directory for factory session data
const SESSIONS_DIR: &str = "sessions";
/// Directory for factory logs (under ~/.cas)
const LOGS_DIR: &str = "logs/factory";

/// Get the sessions directory path
pub fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cas")
        .join(SESSIONS_DIR)
}

/// Get the base logs directory path
pub fn logs_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cas")
        .join(LOGS_DIR)
}

/// Get the log directory for a specific session
pub fn session_log_dir(session_name: &str) -> PathBuf {
    logs_dir().join(session_name)
}

/// Log file path for daemon stderr
pub fn daemon_log_path(session_name: &str) -> PathBuf {
    session_log_dir(session_name).join("daemon.log")
}

/// Log file path for daemon tracing
pub fn daemon_trace_log_path(session_name: &str) -> PathBuf {
    session_log_dir(session_name).join("daemon-trace.log")
}

/// Append one `daemon_lifecycle` record to the session's daemon.log and
/// daemon-trace.log (GH #1363).
///
/// The daemon's tracing goes to the project runtime log, so before this
/// record a session that spawned no worker left both files empty and read
/// as a launch that died. Every start, exit and forced exit writes one line
/// here, synchronously, so it survives an immediate process exit. The write
/// is best effort.
pub(crate) fn record_daemon_lifecycle(session_name: &str, stage: &str, detail: &str) {
    let record = serde_json::json!({
        "timestamp": Utc::now().to_rfc3339(),
        "event": "daemon_lifecycle",
        "factory_session": session_name,
        "pid": std::process::id(),
        "stage": stage,
        "detail": detail,
    });
    let line = format!("{record}\n");
    for path in [
        daemon_log_path(session_name),
        daemon_trace_log_path(session_name),
    ] {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = std::io::Write::write_all(&mut file, line.as_bytes());
        }
    }
}

/// Log file path for server stderr
pub fn server_log_path(session_name: &str) -> PathBuf {
    session_log_dir(session_name).join("server.log")
}

/// Log file path for server tracing
pub fn server_trace_log_path(session_name: &str) -> PathBuf {
    session_log_dir(session_name).join("server-trace.log")
}

/// Log file path for TUI tracing
pub fn tui_log_path(session_name: &str) -> PathBuf {
    session_log_dir(session_name).join("tui.log")
}

/// Log file path for panic backtraces
pub fn panic_log_path(session_name: &str) -> PathBuf {
    session_log_dir(session_name).join("panic.log")
}

/// Native pathname address capacity, including the terminating NUL.
/// Linux supplies 108 bytes and macOS 104; derive it from the target ABI.
fn unix_socket_capacity() -> usize {
    std::mem::size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path)
}

fn validate_socket_path(path: &Path, capacity: usize, label: &str) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= capacity || bytes.contains(&0) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!(
            "Factory startup refused: {label} Unix socket path is invalid.\nUse a shorter HOME or session name, then retry.\nPath: {} bytes; maximum: {} bytes (address capacity {capacity}).",
            bytes.len(), capacity.saturating_sub(1),
        )));
    }
    Ok(())
}

/// An inherited override must belong to the declared launch project. Git and
/// Cassy worktree aliases still resolve to their shared main-project store.
pub(crate) fn validate_factory_project(cwd: &Path) -> anyhow::Result<()> {
    let Some(selected) = cas_core::env_overlay::var("CAS_ROOT")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    else {
        return Ok(());
    };
    let own = crate::store::find_cas_root_ignoring_env(cwd)
        .ok()
        .and_then(|path| path.canonicalize().ok());
    if !selected.canonicalize().ok().is_some_and(|root| Some(root) == own) {
        anyhow::bail!("Factory startup refused: CAS_ROOT selects another project.\nRetry after running: unset CAS_ROOT\nThe --cwd project must own its store before agents can start.");
    }
    Ok(())
}

/// Read-only admission shared by CLI, fork-first and legacy startup paths.
/// Run before logging, directory creation, process spawn or registration.
pub(crate) fn validate_daemon_start(session: &str, cwd: &Path) -> anyhow::Result<()> {
    let capacity = unix_socket_capacity();
    validate_socket_path(&gui_socket_path(session), capacity, "GUI")?;
    validate_socket_path(&socket_path(session), capacity, "TUI")?;
    validate_factory_project(cwd)
}

/// Get the socket path for a session
pub fn socket_path(session_name: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cas")
        .join(format!("factory-{session_name}.sock"))
}

/// Get the GUI socket path for a session (used by desktop GUI clients)
pub fn gui_socket_path(session_name: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cas")
        .join(format!("factory-{session_name}.gui.sock"))
}

#[cfg(unix)]
pub(super) fn bind_factory_socket(
    path: &Path,
) -> std::io::Result<std::os::unix::net::UnixListener> {
    validate_socket_path(path, unix_socket_capacity(), "Factory")?;
    let base = sessions_dir();
    if path.parent() != base.parent()
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("factory-") && name.ends_with(".sock"))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid factory socket path",
        ));
    }
    socket_cleanup::bind(path, &base)
}

/// Get the metadata file path for a session
pub fn metadata_path(session_name: &str) -> PathBuf {
    sessions_dir().join(format!("{session_name}.json"))
}

/// cas-cef2 (GH #1242): the session daemon's pinned provisioner, a link to
/// (or copy of) the executable the daemon started from. Worker provisioning
/// runs this file, so `cas update` replacing the installed binary cannot
/// change the build a running session provisions with.
pub fn provisioner_pin_path(session_name: &str) -> PathBuf {
    provisioner_pin_dir(&sessions_dir(), session_name).join("cas")
}

fn provisioner_pin_dir(sessions_dir: &Path, session_name: &str) -> PathBuf {
    sessions_dir.join(format!("{session_name}.provisioner"))
}

/// Generate a unique session name using project + friendly adjective-noun format
///
/// Produces names like "cas-internal-swift-falcon-42" when given a project dir,
/// or "swift-falcon-42" without one.
pub fn generate_session_name(project_dir: Option<&str>) -> String {
    use crate::orchestration::names;

    let prefix = project_dir
        .map(|p| {
            Path::new(p)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .filter(|s| !s.is_empty());

    // Generate names until we find one not already in use
    for _ in 0..100 {
        let friendly = names::generate();
        let name = match &prefix {
            Some(p) => format!("{p}-{friendly}"),
            None => friendly,
        };
        if !metadata_path(&name).exists() {
            return name;
        }
    }

    // Fallback: append timestamp suffix for guaranteed uniqueness
    let friendly = names::generate();
    let ts = chrono::Local::now().format("%H%M%S");
    match &prefix {
        Some(p) => format!("{p}-{friendly}-{ts}"),
        None => format!("{friendly}-{ts}"),
    }
}

/// Session manager for factory sessions
pub struct SessionManager {
    /// Base directory for session data
    sessions_dir: PathBuf,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Self {
        Self {
            sessions_dir: sessions_dir(),
        }
    }

    pub(crate) fn for_home_read_only(home: &Path) -> Self {
        Self {
            sessions_dir: home.join(".cas").join(SESSIONS_DIR),
        }
    }

    /// Ensure the sessions directory exists
    pub fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.sessions_dir)
    }

    /// List all active sessions
    pub fn list_sessions(&self) -> std::io::Result<Vec<SessionInfo>> {
        self.ensure_dir()?;
        self.cleanup_orphan_sockets()?;
        self.list_sessions_read_only()
    }

    /// A status receipt must not create session state just to inspect it.
    pub(crate) fn list_sessions_read_only(&self) -> std::io::Result<Vec<SessionInfo>> {
        let entries = match fs::read_dir(&self.sessions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        let mut sessions = Vec::new();

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json") {
                if let Ok(metadata) = self.load_metadata(&path) {
                    // Check if daemon is still running
                    let is_running = daemon_identity_is_live(&metadata);
                    let socket_exists = Path::new(&metadata.socket_path).exists();

                    sessions.push(SessionInfo {
                        name: metadata.name.clone(),
                        metadata,
                        is_running,
                        socket_exists,
                    });
                }
            }
        }

        // Sort by creation time (newest first)
        sessions.sort_by(|a, b| b.metadata.created_at.cmp(&a.metadata.created_at));

        Ok(sessions)
    }

    /// Find a running session by name (or return the most recent if no name given)
    pub fn find_session(&self, name: Option<&str>) -> std::io::Result<Option<SessionInfo>> {
        let sessions = self.list_sessions()?;

        if let Some(name) = name {
            // Find by exact name
            Ok(sessions.into_iter().find(|s| s.name == name))
        } else {
            // Return the most recent running session
            Ok(sessions.into_iter().find(|s| s.can_attach()))
        }
    }

    /// Find a running session for the current project
    pub fn find_session_for_project(
        &self,
        name: Option<&str>,
        project_dir: &str,
    ) -> std::io::Result<Option<SessionInfo>> {
        let sessions = self.list_sessions()?;

        if let Some(name) = name {
            // Find by exact name (ignore project filter when name is explicit)
            Ok(sessions.into_iter().find(|s| s.name == name))
        } else {
            // Return the most recent running session that matches this project
            Ok(sessions.into_iter().find(|s| {
                s.can_attach()
                    && s.metadata
                        .project_dir
                        .as_ref()
                        .is_some_and(|p| p == project_dir)
            }))
        }
    }

    /// Save the metadata a running daemon publishes, naming the store it
    /// tracks its worker process groups in. Every daemon start path saves
    /// through here, so `cas kill` can reap a SIGKILLed daemon's workers
    /// even when it ran under CAS_ROOT (cas-4b42, cas-fcb0).
    pub(crate) fn save_daemon_metadata(
        &self,
        mut metadata: SessionMetadata,
        cas_dir: &Path,
    ) -> std::io::Result<SessionMetadata> {
        metadata.cas_root = Some(cas_dir.to_string_lossy().to_string());
        self.save_metadata(&metadata)?;
        Ok(metadata)
    }

    /// Separate from active discovery, so a receipt never becomes a daemon candidate.
    pub(crate) fn record_ended(&self, name: &str, ended: SessionEndOutcome) -> std::io::Result<()> {
        if !valid_session_name(name) {
            return Err(std::io::Error::other("invalid session name"));
        }
        let metadata = self.load_metadata(&self.sessions_dir.join(format!("{name}.json")))?;
        write_end_receipt(&self.sessions_dir, &EndedSession { metadata, ended })
    }

    pub(crate) fn ended_sessions(&self) -> std::io::Result<Vec<EndedSession>> {
        let entries = match fs::read_dir(self.sessions_dir.join("ended")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut receipts: Vec<EndedSession> = entries.filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()?.to_str()? != "json" { return None; }
            serde_json::from_slice(&fs::read(path).ok()?).ok()
        }).collect();
        receipts.sort_by(|a, b| b.ended.at.cmp(&a.ended.at));
        receipts.truncate(100);
        Ok(receipts)
    }

    /// Save session metadata
    pub fn save_metadata(&self, metadata: &SessionMetadata) -> std::io::Result<()> {
        if !valid_session_name(&metadata.name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid session name",
            ));
        }
        self.ensure_dir()?;
        let base = self
            .sessions_dir
            .parent()
            .ok_or_else(|| std::io::Error::other("missing session base"))?;
        let _lock = socket_cleanup::namespace_lock(base)?;
        let path = self.sessions_dir.join(format!("{}.json", metadata.name));
        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    pub(crate) fn lock_inventory(&self, reap: bool) -> std::io::Result<Option<fs::File>> {
        socket_cleanup::inventory_lock(&self.sessions_dir, reap)
    }

    pub(crate) fn reap_dead_record_locked(
        &self,
        path: &Path,
        expected: &[u8],
        gui_identity: Option<(u64, u64)>,
    ) -> std::io::Result<bool> {
        socket_cleanup::reap_metadata_locked(&self.sessions_dir, path, expected, gui_identity)
    }

    /// Load session metadata from a file
    fn load_metadata(&self, path: &Path) -> std::io::Result<SessionMetadata> {
        let json = fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Remove session metadata (called on clean shutdown).
    pub fn remove_metadata(&self, session_name: &str) -> std::io::Result<()> {
        self.remove_metadata_matching(session_name, None, false).map(|_| ())
    }

    /// A lifecycle caller already inspected this birth. A replacement saved
    /// while it waited must keep both its metadata and its live status.
    pub(crate) fn remove_metadata_if_current(&self, expected: &SessionMetadata) -> std::io::Result<bool> {
        self.remove_metadata_matching(&expected.name, Some(expected), false)
    }

    fn remove_metadata_matching(&self, session_name: &str, expected: Option<&SessionMetadata>, require_dead: bool) -> std::io::Result<bool> {
        if !valid_session_name(session_name) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid session name"));
        }
        let base = self.sessions_dir.parent().ok_or_else(|| std::io::Error::other("missing session base"))?;
        // Socket cleanup supplies its own namespace lock and native holder
        // proof. Acquire the metadata lock only after those operations finish.
        for socket in [format!("factory-{session_name}.sock"), format!("factory-{session_name}.gui.sock")] {
            socket_cleanup::remove_if_unheld(&base.join(socket), &self.sessions_dir)?;
        }
        if !self.sessions_dir.exists() { return Ok(false); }
        let _lock = socket_cleanup::namespace_lock(base)?;
        let path = self.sessions_dir.join(format!("{session_name}.json"));
        let _record_lock = socket_cleanup::metadata_lock(&path)?;
        let removed = match self.load_metadata(&path) {
            Ok(metadata) => {
                if expected.is_some_and(|expected| !same_session_birth(expected, &metadata))
                    || (require_dead && daemon_identity_is_live(&metadata)) {
                    return Ok(false);
                }
                retain_end_receipt(&self.sessions_dir, &metadata)?;
                fs::remove_file(path)?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        // The pinned provisioner lives exactly as long as its session.
        match fs::remove_dir_all(provisioner_pin_dir(&self.sessions_dir, session_name)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        Ok(removed)
    }

    /// Clean up stale sessions (daemon not running)
    pub fn cleanup_stale(&self) -> std::io::Result<usize> {
        let sessions = self.list_sessions()?;
        let mut cleaned = 0;

        for session in sessions {
            if !session.is_running {
                if self.remove_metadata_matching(&session.name, Some(&session.metadata), true).unwrap_or(false) {
                    cleaned += 1;
                }
            }
        }

        Ok(cleaned)
    }

    /// Reclaim only owned, unheld factory socket entries. Dead records remain
    /// visible as dead until explicitly cleaned; read-only discovery never GCs.
    fn cleanup_orphan_sockets(&self) -> std::io::Result<()> {
        let Some(base) = self.sessions_dir.parent() else {
            return Ok(());
        };
        for entry in fs::read_dir(base)? {
            let entry = entry?;
            let name = entry.file_name();
            if name
                .to_str()
                .is_some_and(|name| name.starts_with("factory-") && name.ends_with(".sock"))
            {
                socket_cleanup::remove_if_unheld(&entry.path(), &self.sessions_dir)?;
            }
        }
        Ok(())
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a factory session
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Session name
    pub name: String,
    /// Full metadata
    pub metadata: SessionMetadata,
    /// Whether the daemon process is running
    pub is_running: bool,
    /// Whether the socket file exists
    pub socket_exists: bool,
}

/// Whether an ambient `CAS_ROOT` may redirect a session's registry lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootOverride {
    /// Single-project callers keep the process-wide override.
    Honor,
    /// Multi-project callers resolve from the session's own project directory.
    Ignore,
}

impl SessionInfo {
    /// The same lifecycle label drives both the terminal and JSON receipts.
    pub(crate) fn status_label(&self) -> &'static str {
        if !self.is_running {
            "dead"
        } else if self.can_attach() {
            "running"
        } else if !self.socket_exists {
            "orphaned"
        } else {
            "starting"
        }
    }
    /// Check if this session can be attached to
    ///
    /// A session can be attached if the daemon is running AND either:
    /// - Has a WebSocket port configured (preferred)
    /// - Has a Unix socket file (legacy mode)
    pub fn can_attach(&self) -> bool {
        self.is_running && (self.metadata.ws_port.is_some() || self.socket_exists)
    }

    /// Get the socket path for this session
    pub fn socket_path(&self) -> &str {
        &self.metadata.socket_path
    }

    /// Get the names of live workers from the registry, falling back to the
    /// daemon roster when the session's registry cannot be read.
    ///
    /// An ambient `CAS_ROOT` wins here, matching every other single-project
    /// caller in this process.
    pub fn worker_names(&self) -> Vec<String> {
        self.registry_worker_names(RootOverride::Honor)
    }

    /// The roster for a caller that serves several projects at once.
    ///
    /// The Commander hub lists every session on the machine, and the process
    /// that launched it carries one project's `CAS_ROOT`. Honouring that
    /// override would read a gabber-studio session's roster out of cas-src's
    /// registry and report it as worker-less, so this resolution starts from
    /// the session's own `project_dir`.
    pub fn project_worker_names(&self) -> Vec<String> {
        self.registry_worker_names(RootOverride::Ignore)
    }

    fn registry_worker_names(&self, override_policy: RootOverride) -> Vec<String> {
        self.live_registry_worker_names(override_policy)
            .unwrap_or_else(|| {
                self.metadata
                    .workers
                    .iter()
                    .map(|w| w.name.clone())
                    .collect()
            })
    }

    fn live_registry_worker_names(&self, override_policy: RootOverride) -> Option<Vec<String>> {
        let project_dir = self.metadata.project_dir.as_deref()?;
        let cas_root = match override_policy {
            RootOverride::Honor => find_cas_root_from(Path::new(project_dir)).ok()?,
            RootOverride::Ignore => find_cas_root_ignoring_env(Path::new(project_dir)).ok()?,
        };
        let agent_store = open_agent_store(&cas_root).ok()?;
        let agents = agent_store.list(None).ok()?;

        // A catalog poll uses the same heartbeat + harness-process evidence
        // as worker_status. A quiet live harness is still a lane, not a
        // transiently empty roster every time its heartbeat ages out.
        let mut names: std::collections::BTreeSet<_> = agents
            .iter()
            .filter(|agent| {
                agent.factory_session.as_deref() == Some(self.name.as_str())
                    && crate::mcp::tools::service::agent_liveness::is_live_factory_worker(agent)
            })
            .map(|agent| agent.name.clone())
            .collect();
        // The daemon's actual pane roster spans the registration gap during
        // a same-name recycle. A dead metadata PID cannot keep a lane alive.
        names.extend(
            self.metadata.workers.iter()
                .filter(|worker| worker.pid.is_some_and(is_process_running))
                .map(|worker| worker.name.clone()),
        );
        Some(names.into_iter().collect())
    }

    /// Get the number of live workers from the registry, falling back to
    /// daemon metadata only when the registry is unreachable.
    pub fn worker_count(&self) -> usize {
        self.worker_names().len()
    }

    /// Convert to a unified SessionSummary with full metadata.
    ///
    /// This method bridges the daemon discovery info with the unified
    /// `cas_factory::SessionSummary` model.
    pub fn to_session_summary(&self) -> SessionSummary {
        let created_at = chrono::DateTime::parse_from_rfc3339(&self.metadata.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        SessionSummary {
            id: self.name.clone(),
            session_type: SessionType::Factory,
            state: if self.is_running {
                SessionState::Active
            } else {
                SessionState::Paused
            },
            worker_count: self.worker_count(),
            supervisor_name: Some(self.metadata.supervisor.name.clone()),
            created_at,
            project_dir: self
                .metadata
                .project_dir
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_default(),
            recording_enabled: false,
            epic_id: self.metadata.epic_id.clone(),
            last_activity: created_at,
            total_output_bytes: 0,
            agent_states: HashMap::new(),
        }
    }
}

/// Check if a process is running by PID
fn is_process_running(pid: u32) -> bool {
    // Never let an invalid PID turn kill(0) into a process-group query.
    if pid == 0 || pid > i32::MAX as u32 || !crate::mcp::daemon::pid_alive(pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(')')
            .is_some_and(|(_, tail)| matches!(tail.trim_start().chars().next(), Some('Z' | 'X')))
        {
            return false;
        }
    }
    true
}

pub(crate) fn daemon_identity_is_live(metadata: &SessionMetadata) -> bool {
    is_process_running(metadata.daemon_pid)
        && metadata.daemon_pid_starttime.is_none_or(|expected| {
            // An unreadable identity is ambiguous, rather than evidence of death.
            crate::mcp::daemon::read_pid_starttime(metadata.daemon_pid)
                .is_none_or(|actual| actual == expected)
        })
}

/// One filename component, shared by the CLI registry and the hub's session operations.
pub(crate) fn valid_session_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// Create initial session metadata
pub fn create_metadata(
    session_name: &str,
    daemon_pid: u32,
    supervisor_name: &str,
    worker_names: &[String],
    epic_id: Option<&str>,
    project_dir: Option<&str>,
    ws_port: Option<u16>,
) -> SessionMetadata {
    use chrono::Local;

    let log_dir = session_log_dir(session_name);
    let _ = fs::create_dir_all(&log_dir);
    // cas-60dd: an in-place daemon restart keeps deliberate worker holds for
    // the same factory session. Intersect with the workers being restored so
    // a stale name can never leak into a different worker roster. Clean
    // shutdown removes the metadata file, which clears the entire set.
    let known_workers: std::collections::HashSet<&str> =
        worker_names.iter().map(String::as_str).collect();
    let previous_metadata = fs::read_to_string(metadata_path(session_name))
        .ok()
        .and_then(|json| serde_json::from_str::<SessionMetadata>(&json).ok());
    let mut held_workers = previous_metadata
        .as_ref()
        .map(|metadata| {
            metadata
                .held_workers
                .clone()
                .into_iter()
                .filter(|name| known_workers.contains(name.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let delivery_mode = previous_metadata
        .as_ref()
        .map(|metadata| metadata.delivery_mode)
        .unwrap_or_default();
    let last_supervisor_mcp_call_at = previous_metadata
        .as_ref()
        .and_then(|metadata| metadata.last_supervisor_mcp_call_at);
    let supervisor_stall = previous_metadata
        .as_ref()
        .map(|metadata| metadata.supervisor_stall.clone())
        .unwrap_or_default();
    held_workers.sort();
    held_workers.dedup();

    SessionMetadata {
        name: session_name.to_string(),
        created_at: Local::now().to_rfc3339(),
        daemon_pid,
        daemon_build_hash: Some(option_env!("CAS_GIT_HASH").unwrap_or("unknown").to_string()),
        daemon_pid_starttime: crate::mcp::daemon::read_pid_starttime(daemon_pid),
        socket_path: socket_path(session_name).to_string_lossy().to_string(),
        ws_port,
        log_dir: Some(log_dir.to_string_lossy().to_string()),
        daemon_log_path: Some(daemon_log_path(session_name).to_string_lossy().to_string()),
        daemon_trace_log_path: Some(
            daemon_trace_log_path(session_name)
                .to_string_lossy()
                .to_string(),
        ),
        server_log_path: Some(server_log_path(session_name).to_string_lossy().to_string()),
        server_trace_log_path: Some(
            server_trace_log_path(session_name)
                .to_string_lossy()
                .to_string(),
        ),
        tui_log_path: Some(tui_log_path(session_name).to_string_lossy().to_string()),
        panic_log_path: Some(panic_log_path(session_name).to_string_lossy().to_string()),
        supervisor: AgentInfo {
            name: supervisor_name.to_string(),
            pid: None,
            worktree_path: None,
        },
        workers: worker_names
            .iter()
            .map(|name| AgentInfo {
                name: name.clone(),
                pid: None,
                worktree_path: None,
            })
            .collect(),
        epic_id: epic_id.map(|s| s.to_string()),
        pinned_epic_id: None,
        delivery_mode,
        held_workers,
        last_supervisor_mcp_call_at,
        supervisor_stall,
        project_dir: project_dir.map(|s| s.to_string()),
        cas_root: None,
        team_name: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::factory::session::*;

    #[test]
    fn cec1_end_reason_matches_owned_process_even_when_registration_precedes_metadata() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let mut metadata = create_metadata("session-a", 100, "supervisor-a", &[], None, None, None);
        metadata.created_at = "2026-10-28T18:41:32.004138274Z".into();
        metadata.daemon_pid_starttime = Some(54141857);
        // Measured real /exit: registration preceded metadata by 18.06 ms.
        for (pid, start, registered, expected) in [
            (Some(2729301), Some(54141868), "2026-10-28T18:41:31.986078318Z", true),
            (Some(2729301), Some(54141856), "2026-10-28T18:41:31Z", false),
            (Some(2729302), Some(54141868), "2026-10-28T18:41:33Z", false),
            (Some(2729301), None, "2026-10-28T18:41:31Z", false),
        ] {
            assert_eq!(supervisor_end_is_current(&metadata, Some(2729301), pid, start, registered), expected,
                "pid={pid:?} start={start:?} registered={registered}");
        }
    }

    #[test]
    fn stale_removal_retains_end_evidence() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = SessionManager::new();
        let old = create_metadata("crashed", i32::MAX as u32, "supervisor", &[], None, None, None);
        manager.save_metadata(&old).unwrap();
        assert_eq!(manager.cleanup_stale().unwrap(), 1);
        let ended = manager.ended_sessions().unwrap();
        assert_eq!(ended.len(), 1, "metadata cleanup must not erase positive stop evidence");
        assert_eq!(ended[0].metadata.created_at, old.created_at);
        assert!(!ended[0].ended.at.is_empty());

        let mut replacement = old.clone();
        replacement.daemon_pid = std::process::id();
        replacement.created_at = Utc::now().to_rfc3339();
        manager.save_metadata(&replacement).unwrap();
        assert!(!manager.remove_metadata_if_current(&old).unwrap());
        assert_eq!(manager.find_session(Some("crashed")).unwrap().unwrap().metadata.daemon_pid, replacement.daemon_pid);
        assert_eq!(manager.ended_sessions().unwrap()[0].metadata.daemon_pid, old.daemon_pid);
        let rich = SessionEndOutcome {
            at: Utc::now().to_rfc3339(), reason: "prompt_input_exit".into(),
            workers_stopped: vec!["worker".into()], orphans: Vec::new(), unverified: Vec::new(),
        };
        manager.record_ended(&replacement.name, rich.clone()).unwrap();
        assert!(manager.remove_metadata_if_current(&replacement).unwrap());
        assert_eq!(manager.ended_sessions().unwrap()[0].ended, rich);

    }

    #[test]
    fn failed_end_receipt_keeps_stale_metadata_for_retry() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = SessionManager::new();
        manager.save_metadata(&create_metadata("crashed", i32::MAX as u32, "supervisor", &[], None, None, None)).unwrap();
        let blocked = manager.sessions_dir.join("ended");
        fs::write(&blocked, "not a directory").unwrap();
        assert_eq!(manager.cleanup_stale().unwrap(), 0);
        assert!(manager.find_session(Some("crashed")).unwrap().is_some());
        fs::remove_file(blocked).unwrap();
        assert_eq!(manager.cleanup_stale().unwrap(), 1);
        assert_eq!(manager.ended_sessions().unwrap().len(), 1);
    }

    /// cas-fcb0: `cas kill` reaps a SIGKILLed daemon's workers from the store
    /// it recorded. FactoryDaemon::new, the path hub, bridge and Commander
    /// launches take, saved metadata without one, so a CAS_ROOT daemon's
    /// workers were never found. Daemon start paths save through
    /// `save_daemon_metadata`, which always records the store.
    #[test]
    fn daemon_metadata_names_the_daemons_store() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let store = tempfile::tempdir().unwrap();
        let manager = SessionManager::new();
        manager
            .save_daemon_metadata(
                create_metadata("rooted", std::process::id(), "supervisor", &[], None, Some("/project"), None),
                store.path(),
            )
            .unwrap();
        let saved = manager.find_session(Some("rooted")).unwrap().unwrap().metadata;
        assert_eq!(saved.cas_root.as_deref(), store.path().to_str());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn stale_socket(path: &Path) {
        drop(std::os::unix::net::UnixListener::bind(path).unwrap());
    }

    #[cfg(target_os = "linux")]
    fn recorded_session(name: &str, pid: u32) -> SessionManager {
        let manager = SessionManager::new();
        manager
            .save_metadata(&create_metadata(
                name,
                pid,
                "supervisor",
                &[],
                None,
                None,
                None,
            ))
            .unwrap();
        manager
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dead_cleanup_removes_both_sockets_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = recorded_session("dead", i32::MAX as u32);
        stale_socket(&socket_path("dead"));
        stale_socket(&gui_socket_path("dead"));
        assert_eq!(manager.cleanup_stale().unwrap(), 1);
        assert!(!metadata_path("dead").exists());
        assert!(!socket_path("dead").exists());
        assert!(!gui_socket_path("dead").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn orphan_cleanup_preserves_live_listener_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = SessionManager::new();
        manager.ensure_dir().unwrap();
        let live =
            std::os::unix::net::UnixListener::bind(socket_path("unregistered-live")).unwrap();
        stale_socket(&socket_path("unregistered-dead"));
        stale_socket(&gui_socket_path("unregistered-dead"));
        manager.cleanup_stale().unwrap();
        assert!(socket_path("unregistered-live").exists());
        assert!(!socket_path("unregistered-dead").exists());
        assert!(!gui_socket_path("unregistered-dead").exists());
        drop(live);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dead_record_cannot_unlink_live_listener_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = recorded_session("replaced", i32::MAX as u32);
        let main = std::os::unix::net::UnixListener::bind(socket_path("replaced")).unwrap();
        let gui = std::os::unix::net::UnixListener::bind(gui_socket_path("replaced")).unwrap();
        manager.cleanup_stale().unwrap();
        assert!(socket_path("replaced").exists());
        assert!(gui_socket_path("replaced").exists());
        drop((main, gui));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_pid_preserves_even_unheld_sockets_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = recorded_session("alive", std::process::id());
        stale_socket(&socket_path("alive"));
        stale_socket(&gui_socket_path("alive"));
        assert_eq!(manager.cleanup_stale().unwrap(), 0);
        assert!(metadata_path("alive").exists());
        assert!(socket_path("alive").exists());
        assert!(gui_socket_path("alive").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kill_stale_session_removes_gui_socket_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        recorded_session("killed", i32::MAX as u32);
        stale_socket(&socket_path("killed"));
        stale_socket(&gui_socket_path("killed"));
        assert_eq!(
            crate::cli::factory::end_session_by_name("killed").unwrap(),
            crate::cli::factory::EndSessionOutcome::CleanedStale
        );
        assert!(!socket_path("killed").exists());
        assert!(!gui_socket_path("killed").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cleanup_refuses_symlinks_regular_files_and_bad_receipts_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = SessionManager::new();
        manager.ensure_dir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("socket");
        stale_socket(&target);
        std::os::unix::fs::symlink(&target, socket_path("symlink")).unwrap();
        fs::write(socket_path("regular"), "operator file").unwrap();
        stale_socket(&socket_path("uncertain"));
        fs::write(metadata_path("uncertain"), "incomplete receipt").unwrap();
        manager.cleanup_stale().unwrap();
        assert!(target.exists());
        assert!(socket_path("symlink").is_symlink());
        assert_eq!(
            fs::read_to_string(socket_path("regular")).unwrap(),
            "operator file"
        );
        assert!(socket_path("uncertain").exists());
        assert!(manager.remove_metadata("../escape").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kill_waits_for_exit_and_removes_both_sockets_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        recorded_session("terminating", child.id());
        stale_socket(&socket_path("terminating"));
        stale_socket(&gui_socket_path("terminating"));
        let outcome = crate::cli::factory::end_session_by_name("terminating");
        // Always reap our child, even if the assertion fails.
        let _ = child.kill();
        child.wait().unwrap();
        assert_eq!(
            outcome.unwrap(),
            crate::cli::factory::EndSessionOutcome::Ended
        );
        assert!(!metadata_path("terminating").exists());
        assert!(!socket_path("terminating").exists());
        assert!(!gui_socket_path("terminating").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reused_pid_never_signalled_or_unlinked_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = recorded_session("recycled", std::process::id());
        let mut metadata = manager
            .find_session(Some("recycled"))
            .unwrap()
            .unwrap()
            .metadata;
        metadata.daemon_pid_starttime = Some(metadata.daemon_pid_starttime.unwrap() + 1);
        manager.save_metadata(&metadata).unwrap();
        stale_socket(&socket_path("recycled"));
        stale_socket(&gui_socket_path("recycled"));
        assert_eq!(
            manager
                .find_session(Some("recycled"))
                .unwrap()
                .unwrap()
                .status_label(),
            "dead"
        );
        assert_eq!(
            crate::cli::factory::end_session_by_name("recycled").unwrap(),
            crate::cli::factory::EndSessionOutcome::CleanedStale
        );
        assert!(socket_path("recycled").exists());
        assert!(gui_socket_path("recycled").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readonly_discovery_retains_stale_sockets_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = recorded_session("inspect", i32::MAX as u32);
        stale_socket(&socket_path("inspect"));
        assert!(!manager.list_sessions_read_only().unwrap()[0].is_running);
        assert!(socket_path("inspect").exists());
        assert!(metadata_path("inspect").exists());
        manager.list_sessions().unwrap();
        assert!(!socket_path("inspect").exists());
        assert!(metadata_path("inspect").exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn bind_and_cleanup_preserve_held_sockets_with_spaces_cas_c636() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::ffi::OsStrExt;

        // Darwin's sockaddr_un path is only 104 bytes; keep the disposable
        // HOME short while still exercising spaces in the bound pathname.
        let home = tempfile::tempdir_in("/tmp").unwrap();
        let spaced_home = home.path().join("home with spaces");
        fs::create_dir(&spaced_home).unwrap();
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "HOME",
            spaced_home.to_str().unwrap(),
        )]);
        let manager = SessionManager::new();
        manager.ensure_dir().unwrap();
        let path = socket_path("lease");
        stale_socket(&path);
        let inventory = crate::coordination::binding_repair::RuntimeSnapshot::capture(false);
        let live = bind_factory_socket(&path).unwrap();
        assert!(
            inventory.validate().is_err(),
            "socket binding invalidates inventory"
        );
        assert_eq!(
            bind_factory_socket(&path).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse
        );
        let datagram = std::os::unix::net::UnixDatagram::bind(gui_socket_path("lease")).unwrap();
        let nonlistener_path = socket_path("bound");
        // A bound stream socket need not listen. A refused connect must never
        // be mistaken for proof that its filesystem entry has no holder.
        // SAFETY: AF_UNIX and SOCK_STREAM are valid socket arguments; the
        // return value is checked before it is used as a file descriptor.
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        assert!(raw >= 0, "{}", std::io::Error::last_os_error());
        // SAFETY: socket returned a fresh descriptor that is owned only here;
        // this transfers it exactly once into OwnedFd.
        let nonlistener = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: an all-zero sockaddr_un is a valid base for assigning its
        // family, optional Darwin length, and NUL-terminated pathname below.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as _;
        let bytes = nonlistener_path.as_os_str().as_bytes();
        assert!(bytes.len() < address.sun_path.len());
        for (to, from) in address.sun_path.iter_mut().zip(bytes) {
            *to = *from as _;
        }
        let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
        #[cfg(target_os = "macos")]
        {
            address.sun_len = length as _;
        }
        assert_eq!(
            // SAFETY: nonlistener owns a live descriptor; address is initialized
            // and length stays within sockaddr_un because bytes fit sun_path.
            unsafe {
                libc::bind(
                    nonlistener.as_raw_fd(),
                    (&address as *const libc::sockaddr_un).cast(),
                    length as _,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        assert!(std::os::unix::net::UnixStream::connect(&nonlistener_path).is_err());
        manager.cleanup_stale().unwrap();
        assert!(path.exists());
        assert!(gui_socket_path("lease").exists());
        assert!(nonlistener_path.exists());
        drop((live, datagram, nonlistener));
        manager.cleanup_stale().unwrap();
        assert!(!path.exists());
        assert!(!gui_socket_path("lease").exists());
        assert!(!nonlistener_path.exists());
    }

    #[test]
    fn remove_absent_metadata_is_idempotent_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        SessionManager::new().remove_metadata("absent").unwrap();
        assert!(!sessions_dir().exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_kill_cleans_main_and_gui_cas_c636() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        recorded_session("cli-dead", i32::MAX as u32);
        stale_socket(&socket_path("cli-dead"));
        stale_socket(&gui_socket_path("cli-dead"));
        crate::cli::factory::execute_kill(Some("cli-dead"), true, false).unwrap();
        assert!(!metadata_path("cli-dead").exists());
        assert!(!socket_path("cli-dead").exists());
        assert!(!gui_socket_path("cli-dead").exists());
    }

    #[test]
    fn create_metadata_preserves_only_same_session_roster_holds_cas_60dd() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::test_support::TestEnvGuard::with_vars(&[(
            "HOME",
            home.path().to_str().unwrap(),
        )]);
        let session = "restartable-factory";
        let held = "lively-crow";
        let first = create_metadata(
            session,
            1,
            "supervisor",
            &[held.to_string()],
            None,
            None,
            None,
        );
        let mut first = first;
        first.held_workers.push(held.to_string());
        let path = metadata_path(session);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&first).unwrap()).unwrap();

        let restarted = create_metadata(
            session,
            2,
            "supervisor",
            &[held.to_string()],
            None,
            None,
            None,
        );
        assert_eq!(restarted.held_workers, vec![held.to_string()]);

        let different_roster = create_metadata(
            session,
            3,
            "supervisor",
            &["new-crow".to_string()],
            None,
            None,
            None,
        );
        assert!(
            different_roster.held_workers.is_empty(),
            "a stale friendly name must not leak into a different worker roster"
        );

        SessionManager::new().remove_metadata(session).unwrap();
        assert!(
            !path.exists(),
            "clean session shutdown removes the metadata file and every persisted hold"
        );
    }

    #[test]
    fn test_generate_session_name_without_project() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let name = generate_session_name(None);
        // Should be adjective-noun-number format (e.g., "swift-falcon-42")
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 3, "Name should have 3 parts: {name}");
    }

    #[test]
    fn test_generate_session_name_with_project() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let name = generate_session_name(Some("/home/user/my-project"));
        // Should be project-adjective-noun-number (e.g., "my-project-swift-falcon-42")
        assert!(
            name.starts_with("my-project-"),
            "Name should start with project name: {name}"
        );
    }

    #[test]
    fn test_session_paths() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let name = "test-session";
        let sock = socket_path(name);
        let meta = metadata_path(name);

        assert!(sock.to_string_lossy().contains("factory-test-session.sock"));
        assert!(meta.to_string_lossy().contains("test-session.json"));
    }

    #[test]
    fn test_create_metadata() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let meta = create_metadata(
            "test-session",
            12345,
            "supervisor",
            &["worker-1".to_string(), "worker-2".to_string()],
            Some("epic-123"),
            Some("/home/user/my-project"),
            Some(8080),
        );

        assert_eq!(meta.name, "test-session");
        assert_eq!(meta.daemon_pid, 12345);
        assert_eq!(meta.supervisor.name, "supervisor");
        assert_eq!(meta.workers.len(), 2);
        assert_eq!(meta.epic_id, Some("epic-123".to_string()));
        assert_eq!(meta.pinned_epic_id, None);
        assert_eq!(meta.project_dir, Some("/home/user/my-project".to_string()));
        assert_eq!(meta.ws_port, Some(8080));
    }

    #[test]
    fn test_find_session_for_project() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = SessionManager::new();

        // When no sessions exist, should return None
        let result = manager
            .find_session_for_project(None, "/some/project")
            .unwrap();
        assert!(
            result.is_none()
                || result.unwrap().metadata.project_dir != Some("/some/project".to_string())
        );
    }

    #[test]
    fn worker_count_uses_live_factory_registry_workers() {
        use crate::store::{AgentStore, SqliteAgentStore, init_cas_dir};
        use cas_types::{AgentRole, AgentStatus, AgentType};

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let cas_root = init_cas_dir(project.path()).unwrap();
        // `worker_names` resolves with RootOverride::Honor, so CAS_ROOT decides
        // which registry it reads. The guard now pins that at a hermetic empty
        // root (cas-4ccc); point it at the project this test just created,
        // which is what the assertion has always meant.
        env.set("CAS_ROOT", &cas_root);
        let session_name = "factory-registry-count";
        let metadata = create_metadata(
            session_name,
            std::process::id(),
            "supervisor",
            &[],
            None,
            Some(project.path().to_str().unwrap()),
            None,
        );
        let session = SessionInfo {
            name: session_name.to_string(),
            metadata,
            is_running: true,
            socket_exists: false,
        };
        let agents = SqliteAgentStore::open(&cas_root).unwrap();
        agents.init().unwrap();

        for index in 0..5 {
            let mut worker = cas_types::Agent::new(
                format!("registry-worker-{index}"),
                format!("worker-{index}"),
            );
            worker.agent_type = AgentType::Worker;
            worker.role = AgentRole::Worker;
            worker.factory_session = Some(session_name.to_string());
            agents.register(&worker).unwrap();
        }

        assert_eq!(
            session.worker_count(),
            5,
            "live registry workers must replace an empty metadata roster"
        );
        assert_eq!(session.to_session_summary().worker_count, 5);

        let mut shutdown = agents.get("registry-worker-0").unwrap();
        shutdown.status = AgentStatus::Shutdown;
        agents.update(&shutdown).unwrap();
        assert_eq!(session.worker_count(), 4, "shutdown workers are not live");

        let mut stale = agents.get("registry-worker-1").unwrap();
        stale.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(31);
        agents.update(&stale).unwrap();
        assert_eq!(
            session.worker_count(),
            3,
            "workers with stale heartbeats are not live"
        );

        // A quiet but live harness must not disappear and return on catalog
        // polls as its MCP heartbeat crosses the thirty-second boundary.
        stale.pid = Some(std::process::id());
        for seconds in (31..=91).step_by(5) {
            stale.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(seconds);
            agents.update(&stale).unwrap();
            assert_eq!(session.worker_count(), 4, "live harness vanished at {seconds}s");
            assert!(session.project_worker_names().contains(&stale.name));
        }

        let fallback_metadata = create_metadata(
            "factory-registry-unavailable",
            std::process::id(),
            "supervisor",
            &[
                "metadata-worker-a".to_string(),
                "metadata-worker-b".to_string(),
            ],
            None,
            None,
            None,
        );
        let fallback = SessionInfo {
            name: "factory-registry-unavailable".to_string(),
            metadata: fallback_metadata,
            is_running: true,
            socket_exists: false,
        };
        assert_eq!(
            fallback.worker_count(),
            2,
            "metadata is retained when the registry cannot be reached"
        );

        drop(env);
    }

    /// cas-cef2: a session's pinned provisioner is removed with its metadata.
    #[test]
    fn removing_session_metadata_removes_its_pinned_provisioner_cas_cef2() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let manager = SessionManager::new();
        manager.ensure_dir().unwrap();
        let session = "pinned-session-cef2";
        let pin = provisioner_pin_path(session);
        std::fs::create_dir_all(pin.parent().unwrap()).unwrap();
        std::fs::write(&pin, "daemon build").unwrap();
        std::fs::write(metadata_path(session), "{}").unwrap();
        manager.remove_metadata(session).unwrap();
        assert!(!pin.parent().unwrap().exists());
        assert!(!metadata_path(session).exists());
        // A session that never pinned is removed without error.
        manager.remove_metadata("unpinned-session-cef2").unwrap();
    }
}
