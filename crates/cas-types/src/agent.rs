//! Agent type definitions for multi-agent coordination
//!
//! Agents represent Claude Code instances that can claim and work on tasks.
//! The agent ID is the Claude Code session ID - a 1:1 mapping between
//! Claude Code sessions and CAS agents.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;

/// Status of an agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Agent is active and can accept work
    #[default]
    Active,
    /// Agent is idle (no recent heartbeat but not yet expired)
    Idle,
    /// Agent has gone stale (heartbeat expired, can be revived)
    /// Renamed from Dead - agents can now be revived on MCP tool use
    Stale,
    /// Agent has gracefully shut down
    Shutdown,
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentStatus::Active => write!(f, "active"),
            AgentStatus::Idle => write!(f, "idle"),
            AgentStatus::Stale => write!(f, "stale"),
            AgentStatus::Shutdown => write!(f, "shutdown"),
        }
    }
}

impl FromStr for AgentStatus {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "active" => Ok(AgentStatus::Active),
            "idle" => Ok(AgentStatus::Idle),
            "stale" => Ok(AgentStatus::Stale),
            "dead" => Ok(AgentStatus::Stale), // backward compatibility
            "shutdown" => Ok(AgentStatus::Shutdown),
            _ => Err(TypeError::Parse(format!("invalid agent status: {s}"))),
        }
    }
}

/// Type of agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// Primary Claude Code instance (human-interactive)
    #[default]
    Primary,
    /// Sub-agent spawned by Task tool (has own session, linked via parent_id)
    SubAgent,
    /// Background worker (daemon, maintenance)
    Worker,
    /// CI/CD agent
    CI,
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentType::Primary => write!(f, "primary"),
            AgentType::SubAgent => write!(f, "sub_agent"),
            AgentType::Worker => write!(f, "worker"),
            AgentType::CI => write!(f, "ci"),
        }
    }
}

impl FromStr for AgentType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "primary" => Ok(AgentType::Primary),
            "sub_agent" | "subagent" => Ok(AgentType::SubAgent),
            "worker" => Ok(AgentType::Worker),
            "ci" => Ok(AgentType::CI),
            _ => Err(TypeError::Parse(format!("invalid agent type: {s}"))),
        }
    }
}

/// Role of an agent in a factory session
///
/// Roles define the agent's responsibilities in a hierarchical multi-agent system:
/// - Standard: Default role for regular agents (human-interactive or standalone)
/// - Director: Always-on TUI that monitors events, batches notifications, routes to supervisor
/// - Supervisor: Plans epics, assigns tasks, handles merges, ensures quality
/// - Worker: Executes single tasks in isolated worktrees
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Default role for regular agents
    #[default]
    Standard,
    /// Always-on TUI that monitors and routes events
    Director,
    /// Plans epics, assigns tasks, handles merges
    Supervisor,
    /// Executes single tasks in worktrees
    Worker,
}

impl fmt::Display for AgentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentRole::Standard => write!(f, "standard"),
            AgentRole::Director => write!(f, "director"),
            AgentRole::Supervisor => write!(f, "supervisor"),
            AgentRole::Worker => write!(f, "worker"),
        }
    }
}

impl FromStr for AgentRole {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "standard" | "" => Ok(AgentRole::Standard),
            "director" => Ok(AgentRole::Director),
            "supervisor" => Ok(AgentRole::Supervisor),
            "worker" => Ok(AgentRole::Worker),
            _ => Err(TypeError::Parse(format!("invalid agent role: {s}"))),
        }
    }
}

/// An agent that can claim and work on tasks.
///
/// Agent identification uses PPID + machine hash for stability:
/// - Format: `cc-{ppid}-{machine_hash}` (e.g., `cc-12345-a8f3b2c1`)
/// - PPID = Claude Code's PID (since MCP runs as subprocess)
/// - Machine hash = hash of hostname + username for cross-machine uniqueness
///
/// This is stable across `/compact`, `--resume`, and multi-terminal scenarios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// PPID-based agent identifier (cc-{ppid}-{machine_hash})
    pub id: String,

    /// Human-readable name (e.g., "Claude Code (PID 12345)")
    pub name: String,

    /// Type of agent
    pub agent_type: AgentType,

    /// Role in factory sessions (Standard, Director, Supervisor, Worker)
    #[serde(default)]
    pub role: AgentRole,

    /// Current status
    pub status: AgentStatus,

    /// MCP server process ID (for debugging, not identification)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,

    /// Claude Code's PID (the parent process of MCP server)
    /// Used for process liveness checks and debugging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ppid: Option<u32>,

    /// Original Claude Code session ID (for correlation/audit)
    /// Kept for backward compatibility and cloud sync correlation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc_session_id: Option<String>,

    /// Owning factory session for agents spawned inside a factory.
    ///
    /// This is distinct from `cc_session_id`: each agent has its own Claude
    /// Code session, while all agents in the same factory share one
    /// `CAS_FACTORY_SESSION` value. `None` preserves legacy/non-factory rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory_session: Option<String>,

    /// Parent agent ID (for Task tool sub-agents, links to parent's agent ID)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Machine identifier (hostname or unique machine ID)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,

    /// When the agent was registered
    pub registered_at: DateTime<Utc>,

    /// Last heartbeat timestamp
    pub last_heartbeat: DateTime<Utc>,

    /// Number of tasks currently claimed by this agent
    #[serde(default)]
    pub active_tasks: u32,

    /// PID-reuse fingerprint — the `starttime` field (field 22) from
    /// `/proc/<pid>/stat`, clock ticks since boot. Paired with
    /// [`Agent::pid`] (and used by the daemon's liveness gate) to
    /// distinguish "our PID is still alive" from "our PID died and the
    /// kernel recycled it to an unrelated process".
    ///
    /// Promoted from `metadata[PID_STARTTIME_KEY]` to a typed field
    /// (cas-b157 / EPIC cas-9508). The metadata key is still written as
    /// a shadow entry for one release so agents registered on an older
    /// binary and revived mid-flight keep working; the liveness gate
    /// prefers this typed field and falls back to metadata when it is
    /// `None`. Callers should populate via
    /// `cas_cli::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid)`,
    /// which updates both the typed field and the metadata shadow
    /// atomically.
    ///
    /// Available on Linux and macOS. `None` on hosts without a supported
    /// stable process-start API and on pre-migration legacy agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_starttime: Option<u64>,

    /// Optional metadata (capabilities, version, etc.)
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl Agent {
    /// Create a new agent with the given ID and name
    ///
    /// The `id` should be a PPID-based ID (cc-{ppid}-{machine_hash}) for normal agents.
    /// For CLI/worker agents without a Claude Code parent, use `generate_fallback_id()`.
    pub fn new(id: String, name: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            agent_type: AgentType::Primary,
            role: AgentRole::Standard,
            status: AgentStatus::Active,
            pid: None,
            ppid: None,
            cc_session_id: None,
            factory_session: None,
            parent_id: None,
            machine_id: None,
            registered_at: now,
            last_heartbeat: now,
            active_tasks: 0,
            pid_starttime: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Create a new agent with PPID information
    ///
    /// Use this when creating agents from MCP server context where
    /// PPID (Claude Code's PID) and session_id are known.
    pub fn new_with_ppid(
        id: String,
        name: String,
        ppid: u32,
        cc_session_id: Option<String>,
    ) -> Self {
        let mut agent = Self::new(id, name);
        agent.ppid = Some(ppid);
        agent.cc_session_id = cc_session_id;
        agent
    }

    /// Create a new sub-agent with a parent (for Task tool spawned agents)
    ///
    /// Sub-agents have their own PPID-based ID but are linked to their parent.
    pub fn new_sub_agent(id: String, name: String, parent_id: String) -> Self {
        let mut agent = Self::new(id, name);
        agent.agent_type = AgentType::SubAgent;
        agent.parent_id = Some(parent_id);
        agent
    }

    /// Create a new agent with a specific role (for factory sessions)
    ///
    /// Use this when creating agents in a factory session with defined roles.
    pub fn new_with_role(id: String, name: String, role: AgentRole) -> Self {
        let mut agent = Self::new(id, name);
        agent.role = role;
        agent
    }

    /// Set the agent's role
    pub fn with_role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    /// Whether this agent is visible to a caller in the given factory session.
    ///
    /// `Some(session)` enforces strict same-session visibility. `None` preserves
    /// legacy/non-factory behavior and does not filter by factory ownership.
    pub fn visible_to_factory_session(&self, session: Option<&str>) -> bool {
        match session {
            Some(session) => self.factory_session.as_deref() == Some(session),
            None => true,
        }
    }

    /// Generate a fallback ID for agents without a Claude Code session
    ///
    /// Use this only for CLI tools, workers, or CI agents that don't have
    /// a Claude Code session ID. Normal Claude Code agents should use the
    /// session ID as their agent ID.
    pub fn generate_fallback_id() -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        Utc::now().timestamp_nanos_opt().hash(&mut hasher);
        // Add randomness (not PID - that's not meaningful)
        let random: u64 = rand::random();
        random.hash(&mut hasher);

        let hash = hasher.finish();
        format!("cli-{:08x}", hash as u32)
    }

    /// Update the heartbeat timestamp
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = Utc::now();
        if self.status == AgentStatus::Idle {
            self.status = AgentStatus::Active;
        }
    }

    /// Check if the agent's heartbeat has expired
    pub fn is_heartbeat_expired(&self, timeout_secs: i64) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.last_heartbeat)
            .num_seconds();
        elapsed > timeout_secs
    }

    /// Mark the agent as stale (heartbeat expired, can be revived)
    pub fn mark_stale(&mut self) {
        self.status = AgentStatus::Stale;
    }

    /// Mark the agent as gracefully shutdown
    pub fn mark_shutdown(&mut self) {
        self.status = AgentStatus::Shutdown;
    }

    /// Check if the agent is alive (active or idle)
    pub fn is_alive(&self) -> bool {
        matches!(self.status, AgentStatus::Active | AgentStatus::Idle)
    }

    /// Get the machine ID, generating one if needed
    pub fn get_or_generate_machine_id() -> String {
        // Try to get hostname first
        if let Ok(hostname) = hostname::get() {
            if let Some(name) = hostname.to_str() {
                return name.to_string();
            }
        }

        // Fallback to a hash of system info
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        std::env::consts::OS.hash(&mut hasher);
        std::env::consts::ARCH.hash(&mut hasher);
        if let Ok(cwd) = std::env::current_dir() {
            cwd.hash(&mut hasher);
        }
        format!("machine-{:08x}", hasher.finish() as u32)
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new(Self::generate_fallback_id(), "Unnamed Agent".to_string())
    }
}

/// Default heartbeat timeout in seconds (5 minutes)
pub const DEFAULT_HEARTBEAT_TIMEOUT_SECS: i64 = 300;

/// Default heartbeat interval in seconds (30 seconds)
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Default lease duration in seconds (10 minutes)
pub const DEFAULT_LEASE_DURATION_SECS: i64 = 600;

/// Maximum concurrent tasks per agent (default)
pub const DEFAULT_MAX_CONCURRENT_TASKS: u32 = 5;

/// Capabilities that an agent can have
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// General coding tasks
    Coding,
    /// Code review
    Review,
    /// Testing and test writing
    Testing,
    /// Documentation
    Documentation,
    /// DevOps and infrastructure
    DevOps,
    /// Bug fixing
    BugFix,
    /// Research and exploration
    Research,
    /// Refactoring
    Refactor,
}

impl fmt::Display for AgentCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentCapability::Coding => write!(f, "coding"),
            AgentCapability::Review => write!(f, "review"),
            AgentCapability::Testing => write!(f, "testing"),
            AgentCapability::Documentation => write!(f, "documentation"),
            AgentCapability::DevOps => write!(f, "devops"),
            AgentCapability::BugFix => write!(f, "bugfix"),
            AgentCapability::Research => write!(f, "research"),
            AgentCapability::Refactor => write!(f, "refactor"),
        }
    }
}

impl FromStr for AgentCapability {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "coding" | "code" => Ok(AgentCapability::Coding),
            "review" => Ok(AgentCapability::Review),
            "testing" | "test" => Ok(AgentCapability::Testing),
            "documentation" | "docs" => Ok(AgentCapability::Documentation),
            "devops" | "ops" | "infra" => Ok(AgentCapability::DevOps),
            "bugfix" | "bug" | "fix" => Ok(AgentCapability::BugFix),
            "research" | "explore" => Ok(AgentCapability::Research),
            "refactor" | "refactoring" => Ok(AgentCapability::Refactor),
            _ => Err(TypeError::Parse(format!("invalid capability: {s}"))),
        }
    }
}

impl AgentCapability {
    /// Get all available capabilities
    pub fn all() -> Vec<Self> {
        vec![
            Self::Coding,
            Self::Review,
            Self::Testing,
            Self::Documentation,
            Self::DevOps,
            Self::BugFix,
            Self::Research,
            Self::Refactor,
        ]
    }

    /// Get default capabilities for an agent type
    pub fn defaults_for(agent_type: AgentType) -> Vec<Self> {
        match agent_type {
            AgentType::Primary => Self::all(),
            AgentType::SubAgent => vec![Self::Coding, Self::Research],
            AgentType::Worker => vec![Self::Testing, Self::Documentation],
            AgentType::CI => vec![Self::Testing, Self::DevOps],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::*;

    #[test]
    fn test_agent_status_from_str() {
        assert_eq!(
            AgentStatus::from_str("active").unwrap(),
            AgentStatus::Active
        );
        assert_eq!(AgentStatus::from_str("idle").unwrap(), AgentStatus::Idle);
        assert_eq!(AgentStatus::from_str("stale").unwrap(), AgentStatus::Stale);
        assert_eq!(AgentStatus::from_str("dead").unwrap(), AgentStatus::Stale); // backward compat
        assert_eq!(
            AgentStatus::from_str("shutdown").unwrap(),
            AgentStatus::Shutdown
        );
        assert!(AgentStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_agent_type_from_str() {
        assert_eq!(AgentType::from_str("primary").unwrap(), AgentType::Primary);
        assert_eq!(
            AgentType::from_str("sub_agent").unwrap(),
            AgentType::SubAgent
        );
        assert_eq!(AgentType::from_str("worker").unwrap(), AgentType::Worker);
        assert_eq!(AgentType::from_str("ci").unwrap(), AgentType::CI);
    }

    #[test]
    fn test_agent_new() {
        let agent = Agent::new("agent-test".to_string(), "Test Agent".to_string());
        assert_eq!(agent.id, "agent-test");
        assert_eq!(agent.name, "Test Agent");
        assert_eq!(agent.agent_type, AgentType::Primary);
        assert_eq!(agent.status, AgentStatus::Active);
        assert!(agent.is_alive());
    }

    #[test]
    fn test_agent_sub_agent() {
        let agent = Agent::new_sub_agent(
            "agent-sub".to_string(),
            "Sub Agent".to_string(),
            "agent-parent".to_string(),
        );
        assert_eq!(agent.agent_type, AgentType::SubAgent);
        assert_eq!(agent.parent_id, Some("agent-parent".to_string()));
    }

    #[test]
    fn test_agent_fallback_id_generation() {
        let id1 = Agent::generate_fallback_id();
        let id2 = Agent::generate_fallback_id();
        assert!(id1.starts_with("cli-"));
        assert!(id2.starts_with("cli-"));
        // IDs should be different (with very high probability)
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_heartbeat() {
        let mut agent = Agent::new("test".to_string(), "Test".to_string());
        let old_heartbeat = agent.last_heartbeat;
        std::thread::sleep(std::time::Duration::from_millis(10));
        agent.heartbeat();
        assert!(agent.last_heartbeat > old_heartbeat);
    }

    #[test]
    fn test_heartbeat_expiry() {
        let mut agent = Agent::new("test".to_string(), "Test".to_string());
        // Set heartbeat to 10 seconds ago
        agent.last_heartbeat = Utc::now() - chrono::Duration::seconds(10);
        assert!(agent.is_heartbeat_expired(5));
        assert!(!agent.is_heartbeat_expired(15));
    }

    #[test]
    fn test_agent_role_from_str() {
        assert_eq!(
            AgentRole::from_str("standard").unwrap(),
            AgentRole::Standard
        );
        assert_eq!(AgentRole::from_str("").unwrap(), AgentRole::Standard);
        assert_eq!(
            AgentRole::from_str("director").unwrap(),
            AgentRole::Director
        );
        assert_eq!(
            AgentRole::from_str("supervisor").unwrap(),
            AgentRole::Supervisor
        );
        assert_eq!(AgentRole::from_str("worker").unwrap(), AgentRole::Worker);
        assert!(AgentRole::from_str("invalid").is_err());
    }

    #[test]
    fn test_agent_role_display() {
        assert_eq!(AgentRole::Standard.to_string(), "standard");
        assert_eq!(AgentRole::Director.to_string(), "director");
        assert_eq!(AgentRole::Supervisor.to_string(), "supervisor");
        assert_eq!(AgentRole::Worker.to_string(), "worker");
    }

    #[test]
    fn test_agent_new_with_role() {
        let agent = Agent::new_with_role(
            "supervisor-1".to_string(),
            "Supervisor Agent".to_string(),
            AgentRole::Supervisor,
        );
        assert_eq!(agent.role, AgentRole::Supervisor);
        assert_eq!(agent.agent_type, AgentType::Primary);
    }

    #[test]
    fn test_agent_with_role_builder() {
        let agent = Agent::new("worker-1".to_string(), "Worker Agent".to_string())
            .with_role(AgentRole::Worker);
        assert_eq!(agent.role, AgentRole::Worker);
    }

    #[test]
    fn test_agent_default_role() {
        let agent = Agent::new("test".to_string(), "Test".to_string());
        assert_eq!(agent.role, AgentRole::Standard);
    }
}
