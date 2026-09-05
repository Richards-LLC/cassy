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

    /// Ensure the sessions directory exists
    pub fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.sessions_dir)
    }

    /// List all active sessions
    pub fn list_sessions(&self) -> std::io::Result<Vec<SessionInfo>> {
        self.ensure_dir()?;

        let mut sessions = Vec::new();

        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json") {
                if let Ok(metadata) = self.load_metadata(&path) {
                    // Check if daemon is still running
                    let is_running = is_process_running(metadata.daemon_pid);
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
        self.ensure_dir()?;
        let path = metadata_path(&metadata.name);
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
        let path = metadata_path(session_name);
        if path.exists() {
            fs::remove_file(path)?;
        }

        // Also remove socket if it exists
        let sock = socket_path(session_name);
        if sock.exists() {
            let _ = fs::remove_file(sock);
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
#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    use std::process::Command;

    // Use kill -0 to check if process exists (sends no signal, just checks)
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_process_running(_pid: u32) -> bool {
    // On non-Unix, assume running if we have the PID
    true
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
        let name = generate_session_name(None);
        // Should be adjective-noun-number format (e.g., "swift-falcon-42")
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 3, "Name should have 3 parts: {name}");
    }

    #[test]
    fn test_generate_session_name_with_project() {
        let name = generate_session_name(Some("/home/user/my-project"));
        // Should be project-adjective-noun-number (e.g., "my-project-swift-falcon-42")
        assert!(
            name.starts_with("my-project-"),
            "Name should start with project name: {name}"
        );
    }

    #[test]
    fn test_session_paths() {
        let name = "test-session";
        let sock = socket_path(name);
        let meta = metadata_path(name);

        assert!(sock.to_string_lossy().contains("factory-test-session.sock"));
        assert!(meta.to_string_lossy().contains("test-session.json"));
    }

    #[test]
    fn test_create_metadata() {
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
