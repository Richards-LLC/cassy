use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique identifier for a factory session
pub type SessionId = String;

/// Session lifecycle state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Session is currently active and running
    Active,
    /// Session is paused (can be resumed)
    Paused,
    /// Session has been archived (read-only history)
    Archived,
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionState::Active => write!(f, "active"),
            SessionState::Paused => write!(f, "paused"),
            SessionState::Archived => write!(f, "archived"),
        }
    }
}

/// Type of session determining its capabilities and behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    /// Multi-agent factory session with supervisor and workers
    #[default]
    Factory,
    /// Single-agent managed session with auto-save features
    Managed,
    /// Session recording/playback mode
    Recording,
}

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionType::Factory => write!(f, "factory"),
            SessionType::Managed => write!(f, "managed"),
            SessionType::Recording => write!(f, "recording"),
        }
    }
}

/// State of an individual agent within a session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is actively processing
    #[default]
    Active,
    /// Agent is idle, waiting for work
    Idle,
    /// Agent is blocked on a dependency or user input
    Blocked,
    /// Agent has exited or been shut down
    Exited,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentState::Active => write!(f, "active"),
            AgentState::Idle => write!(f, "idle"),
            AgentState::Blocked => write!(f, "blocked"),
            AgentState::Exited => write!(f, "exited"),
        }
    }
}

/// Unified session summary with full metadata for Tauri commands.
///
/// This struct provides a serializable view of session state that can be
/// used across different session types (Factory, Managed, Recording).
/// Designed for use in desktop UI and API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Unique session identifier
    pub id: SessionId,

    /// Type of session (Factory, Managed, Recording)
    pub session_type: SessionType,

    /// Current session state (Active, Paused, Archived)
    pub state: SessionState,

    /// Number of active workers in this session
    pub worker_count: usize,

    /// Name of the supervisor agent (for Factory sessions)
    pub supervisor_name: Option<String>,

    /// When the session was created
    pub created_at: DateTime<Utc>,

    /// Project directory for this session
    pub project_dir: PathBuf,

    /// Whether session recording is enabled
    pub recording_enabled: bool,

    /// Associated epic ID (if working on an epic)
    pub epic_id: Option<String>,

    /// Timestamp of the last activity in this session
    pub last_activity: DateTime<Utc>,

    /// Total bytes of output captured across all agents
    pub total_output_bytes: u64,

    /// State of each agent by name
    #[serde(default)]
    pub agent_states: HashMap<String, AgentState>,
}

impl SessionSummary {
    /// Create a new session summary with default values
    pub fn new(id: SessionId, session_type: SessionType, project_dir: PathBuf) -> Self {
        let now = Utc::now();
        Self {
            id,
            session_type,
            state: SessionState::Active,
            worker_count: 0,
            supervisor_name: None,
            created_at: now,
            project_dir,
            recording_enabled: false,
            epic_id: None,
            last_activity: now,
            total_output_bytes: 0,
            agent_states: HashMap::new(),
        }
    }

    /// Create a factory session summary
    pub fn factory(id: SessionId, project_dir: PathBuf, supervisor_name: Option<String>) -> Self {
        let mut summary = Self::new(id, SessionType::Factory, project_dir);
        summary.supervisor_name = supervisor_name;
        summary
    }

    /// Create a managed session summary
    pub fn managed(id: SessionId, project_dir: PathBuf) -> Self {
        Self::new(id, SessionType::Managed, project_dir)
    }

    /// Create a recording session summary
    pub fn recording(id: SessionId, project_dir: PathBuf) -> Self {
        let mut summary = Self::new(id, SessionType::Recording, project_dir);
        summary.recording_enabled = true;
        summary
    }

    /// Update the last activity timestamp
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }

    /// Set the epic ID
    pub fn set_epic(&mut self, epic_id: impl Into<String>) {
        self.epic_id = Some(epic_id.into());
    }

    /// Add or update an agent's state
    pub fn set_agent_state(&mut self, name: impl Into<String>, state: AgentState) {
        let name = name.into();
        if state == AgentState::Exited {
            self.agent_states.remove(&name);
        } else {
            self.agent_states.insert(name, state);
        }
        self.worker_count = self
            .agent_states
            .values()
            .filter(|s| **s != AgentState::Exited)
            .count();
    }

    /// Add bytes to the total output count
    pub fn add_output_bytes(&mut self, bytes: u64) {
        self.total_output_bytes = self.total_output_bytes.saturating_add(bytes);
    }

    /// Check if this is a factory session
    pub fn is_factory(&self) -> bool {
        self.session_type == SessionType::Factory
    }

    /// Check if this is a managed session
    pub fn is_managed(&self) -> bool {
        self.session_type == SessionType::Managed
    }

    /// Check if this is a recording session
    pub fn is_recording(&self) -> bool {
        self.session_type == SessionType::Recording
    }

    /// Get count of active agents (not exited)
    pub fn active_agent_count(&self) -> usize {
        self.agent_states
            .values()
            .filter(|s| **s != AgentState::Exited)
            .count()
    }
}

impl Default for SessionSummary {
    fn default() -> Self {
        Self::new(
            generate_session_id(),
            SessionType::Factory,
            PathBuf::from("."),
        )
    }
}

pub(crate) fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    // Format: fs-<timestamp>-<random>
    format!("fs-{:x}-{:04x}", timestamp, rand_u16())
}

/// Generate a random u16 for session ID uniqueness
fn rand_u16() -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::Instant;

    let mut hasher = DefaultHasher::new();
    Instant::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    hasher.finish() as u16
}
