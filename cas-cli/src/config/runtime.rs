use serde::{Deserialize, Serialize};

/// Worktree configuration for automatic git worktree management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreesConfig {
    /// Whether automatic worktree creation is enabled on task start
    #[serde(default)]
    pub enabled: bool,

    /// Base directory for worktrees (relative to repo root's parent)
    /// Supports {project} placeholder for project name
    #[serde(default = "default_worktree_base_path")]
    pub base_path: String,

    /// Prefix for branch names (e.g., "cas/")
    #[serde(default = "default_worktree_branch_prefix")]
    pub branch_prefix: String,

    /// Auto-merge branch on task close (if no conflicts)
    #[serde(default)]
    pub auto_merge: bool,

    /// Auto-cleanup worktree directory on task close
    #[serde(default = "default_true")]
    pub cleanup_on_close: bool,

    /// Promote entries with positive feedback to global scope on merge
    #[serde(default = "default_true")]
    pub promote_entries_on_merge: bool,

    /// Require worktree merge before epic close (triggers worktree-merger jail)
    /// When true (default), closing an epic with an active worktree requires
    /// spawning the worktree-merger agent first. Set to false to allow closing
    /// epics without merging their worktrees.
    #[serde(default = "default_true")]
    pub require_merge_on_epic_close: bool,

    /// Hours a worktree can sit without heartbeat activity before the daemon
    /// reaper considers it abandoned and eligible for cleanup.
    #[serde(default = "default_abandon_ttl_hours")]
    pub abandon_ttl_hours: u32,

    /// Debounce interval for the opportunistic cross-repo global sweep,
    /// in seconds. The sweep is skipped if the last successful run is newer
    /// than this many seconds.
    #[serde(default = "default_global_sweep_debounce_secs")]
    pub global_sweep_debounce_secs: u64,

    /// Whether the opportunistic sweep reclaims `<repo>/.claude/worktrees/agent-*`
    /// directories in addition to `<repo>/.cas/worktrees/*`. Default: true.
    /// Set to false if Claude Code's worktree semantics change and you need
    /// to opt out without rolling Cassy back.
    #[serde(default = "default_true")]
    pub sweep_claude_agent_dirs: bool,
}

fn default_worktree_base_path() -> String {
    "{project}/.cas/worktrees".to_string()
}

fn default_worktree_branch_prefix() -> String {
    "cas/".to_string()
}

fn default_abandon_ttl_hours() -> u32 {
    24
}

fn default_global_sweep_debounce_secs() -> u64 {
    3600
}

impl Default for WorktreesConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            base_path: default_worktree_base_path(),
            branch_prefix: default_worktree_branch_prefix(),
            auto_merge: false,
            cleanup_on_close: true,
            promote_entries_on_merge: true,
            require_merge_on_epic_close: true, // Require merge by default
            abandon_ttl_hours: default_abandon_ttl_hours(),
            global_sweep_debounce_secs: default_global_sweep_debounce_secs(),
            sweep_claude_agent_dirs: true,
        }
    }
}

impl WorktreesConfig {
    /// Resolve base_path with {project} placeholder substitution.
    ///
    /// The {project} placeholder is replaced with the project directory name.
    /// Relative paths are resolved relative to the project's parent directory.
    pub fn resolve_base_path(&self, project_dir: &std::path::Path) -> std::path::PathBuf {
        let project_name = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");

        let resolved = self.base_path.replace("{project}", project_name);
        let path = std::path::PathBuf::from(&resolved);

        if path.is_absolute() {
            path
        } else {
            project_dir
                .parent()
                .map(|p| p.join(&path))
                .unwrap_or_else(|| project_dir.join(&path))
        }
    }
}

/// Verification configuration for task quality gates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationConfig {
    /// Whether verification is enabled on task close
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Model to use for verification (Haiku recommended for speed)
    #[serde(default = "default_verification_model")]
    pub model: String,

    /// Allow force bypass of verification (--force flag)
    #[serde(default)]
    pub force_bypass_allowed: bool,

    /// Maximum time for verification (seconds)
    #[serde(default = "default_verification_timeout")]
    pub timeout_secs: u64,

    /// Pattern detection configuration
    #[serde(default)]
    pub patterns: VerificationPatterns,
}

/// Patterns to detect during verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationPatterns {
    /// Detect TODO/FIXME comments
    #[serde(default = "default_true")]
    pub todo_comments: bool,

    /// Detect temporal shortcuts ("for now", "temporarily", etc.)
    #[serde(default = "default_true")]
    pub temporal_shortcuts: bool,

    /// Detect stub implementations
    #[serde(default = "default_true")]
    pub stub_implementations: bool,

    /// Detect empty catch/error blocks
    #[serde(default = "default_true")]
    pub empty_error_handlers: bool,

    /// Custom patterns to detect (regex)
    #[serde(default)]
    pub custom: Vec<String>,
}

fn default_verification_model() -> String {
    "claude-haiku-4-5".to_string()
}

fn default_verification_timeout() -> u64 {
    120 // 2 minutes
}

impl Default for VerificationPatterns {
    fn default() -> Self {
        Self {
            todo_comments: true,
            temporal_shortcuts: true,
            stub_implementations: true,
            empty_error_handlers: true,
            custom: vec![],
        }
    }
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: default_verification_model(),
            force_bypass_allowed: false,
            timeout_secs: default_verification_timeout(),
            patterns: VerificationPatterns::default(),
        }
    }
}

/// Agent configuration for multi-agent coordination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Human-readable name for this agent
    #[serde(default = "default_agent_name")]
    pub name: String,

    /// Maximum concurrent tasks this agent can claim
    #[serde(default = "default_max_concurrent_tasks")]
    pub max_concurrent_tasks: u32,

    /// Capabilities this agent advertises
    #[serde(default)]
    pub capabilities: Vec<String>,
}

fn default_agent_name() -> String {
    std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "Claude".to_string())
}

fn default_max_concurrent_tasks() -> u32 {
    std::env::var("CAS_AGENT_MAX_TASKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            name: default_agent_name(),
            max_concurrent_tasks: default_max_concurrent_tasks(),
            capabilities: vec!["coding".to_string()],
        }
    }
}

/// Coordination mode for multi-agent setup
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CoordinationMode {
    /// Local SQLite-based coordination (single machine)
    Local,
    /// Cloud PostgreSQL-based coordination (distributed)
    Cloud,
}

impl Default for CoordinationMode {
    fn default() -> Self {
        match std::env::var("CAS_COORDINATION_MODE").as_deref() {
            Ok("cloud") => Self::Cloud,
            _ => Self::Local,
        }
    }
}

/// Coordination configuration for multi-agent mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationConfig {
    /// Coordination mode: local (SQLite) or cloud (PostgreSQL)
    #[serde(default)]
    pub mode: CoordinationMode,

    /// Cloud coordination URL (only used when mode is Cloud)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_url: Option<String>,
}

impl Default for CoordinationConfig {
    fn default() -> Self {
        Self {
            mode: CoordinationMode::default(),
            cloud_url: std::env::var("CAS_CLOUD_URL").ok(),
        }
    }
}

/// Lease configuration for task claiming
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseConfig {
    /// Default lease duration in minutes
    #[serde(default = "default_lease_duration")]
    pub default_duration_mins: u32,

    /// Maximum lease duration in minutes
    #[serde(default = "default_max_lease_duration")]
    pub max_duration_mins: u32,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u32,

    /// Grace period after expiry before reclaiming (seconds)
    #[serde(default = "default_expiry_grace")]
    pub expiry_grace_secs: u32,
}

fn default_lease_duration() -> u32 {
    std::env::var("CAS_LEASE_DURATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
}

fn default_max_lease_duration() -> u32 {
    240 // 4 hours
}

fn default_heartbeat_interval() -> u32 {
    300 // 5 minutes
}

fn default_expiry_grace() -> u32 {
    120 // 2 minutes
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            default_duration_mins: default_lease_duration(),
            max_duration_mins: default_max_lease_duration(),
            heartbeat_interval_secs: default_heartbeat_interval(),
            expiry_grace_secs: default_expiry_grace(),
        }
    }
}

fn default_true() -> bool {
    true
}
