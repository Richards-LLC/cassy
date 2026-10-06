//! Factory session management
//!
//! Handles daemon discovery for factory sessions. This module provides
//! functionality to find running factory daemons by checking PIDs and sockets.
//!
//! For unified session metadata (worker count, epic ID, etc.), use
//! `cas_factory::SessionSummary` and `cas_factory::UnifiedSessionManager`.

use crate::store::{find_cas_root_from, find_cas_root_ignoring_env, open_agent_store};
use crate::ui::factory::protocol::{AgentInfo, SessionMetadata};
use cas_factory::{SessionState, SessionSummary, SessionType};
use cas_types::{AgentStatus, AgentType};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

mod socket_cleanup;

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

    /// Save session metadata
    pub fn save_metadata(&self, metadata: &SessionMetadata) -> std::io::Result<()> {
        if !valid_session_name(&metadata.name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid session name",
            ));
        }
        self.ensure_dir()?;
        let path = self.sessions_dir.join(format!("{}.json", metadata.name));
        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Load session metadata from a file
    fn load_metadata(&self, path: &Path) -> std::io::Result<SessionMetadata> {
        let json = fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Remove session metadata (called on clean shutdown)
    pub fn remove_metadata(&self, session_name: &str) -> std::io::Result<()> {
        if !valid_session_name(session_name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid session name",
            ));
        }
        // Keep the receipt available while checking socket ownership. A live
        // process, even one with a recycled PID, makes cleanup conservative.
        let base = self
            .sessions_dir
            .parent()
            .ok_or_else(|| std::io::Error::other("missing session base"))?;
        for socket in [
            format!("factory-{session_name}.sock"),
            format!("factory-{session_name}.gui.sock"),
        ] {
            socket_cleanup::remove_if_unheld(&base.join(socket), &self.sessions_dir)?;
        }
        let path = self.sessions_dir.join(format!("{session_name}.json"));
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Clean up stale sessions (daemon not running)
    pub fn cleanup_stale(&self) -> std::io::Result<usize> {
        let sessions = self.list_sessions()?;
        let mut cleaned = 0;

        for session in sessions {
            if !session.is_running {
                if self.remove_metadata(&session.name).is_err() {
                    // Cleanup failed, continue with other sessions
                } else {
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

        Some(
            agents
                .into_iter()
                .filter(|agent| {
                    agent.agent_type == AgentType::Worker
                        && agent.factory_session.as_deref() == Some(self.name.as_str())
                        && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
                        && !agent.is_heartbeat_expired(
                            crate::mcp::tools::service::agent_liveness::WORKER_STALE_SECS,
                        )
                })
                .map(|agent| agent.name)
                .collect(),
        )
    }

    /// Get the number of live workers from the registry, falling back to
    /// daemon metadata only when the registry is unreachable.
    pub fn worker_count(&self) -> usize {
        self.worker_names().len()
    }

    /// Convert to a unified SessionSummary with full metadata.
    ///
    /// This method bridges the daemon discovery info with the unified
    /// session model used by `cas_factory::UnifiedSessionManager`.
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

fn valid_session_name(name: &str) -> bool {
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
        team_name: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::factory::session::*;

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn bind_and_cleanup_preserve_held_sockets_with_spaces_cas_c636() {
        let home = tempfile::tempdir().unwrap();
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
        let live = bind_factory_socket(&path).unwrap();
        assert_eq!(
            bind_factory_socket(&path).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse
        );
        let datagram = std::os::unix::net::UnixDatagram::bind(gui_socket_path("lease")).unwrap();
        manager.cleanup_stale().unwrap();
        assert!(path.exists());
        assert!(gui_socket_path("lease").exists());
        drop((live, datagram));
        manager.cleanup_stale().unwrap();
        assert!(!path.exists());
        assert!(!gui_socket_path("lease").exists());
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
        crate::cli::factory::execute_kill(Some("cli-dead"), true).unwrap();
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
}
