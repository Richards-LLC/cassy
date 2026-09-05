//! Configuration types for factory mode.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Optional server-side enrichment for Commander session cards.
///
/// Disabled by default because enabling it sends a redacted excerpt of
/// terminal output to the configured model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiEnrichmentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_summary_provider")]
    pub provider: String,
    #[serde(default = "default_summary_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_summary_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_summary_model")]
    pub model: String,
    #[serde(default = "default_summary_effort")]
    pub effort: String,
}

fn default_summary_provider() -> String {
    "openai".to_string()
}
fn default_summary_endpoint() -> String {
    "https://api.openai.com/v1/responses".to_string()
}
fn default_summary_api_key_env() -> String {
    "OPENAI_API_KEY".to_string()
}
fn default_summary_model() -> String {
    "gpt-5.6-luna".to_string()
}
fn default_summary_effort() -> String {
    "low".to_string()
}

impl Default for AiEnrichmentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_summary_provider(),
            endpoint: default_summary_endpoint(),
            api_key_env: default_summary_api_key_env(),
            model: default_summary_model(),
            effort: default_summary_effort(),
        }
    }
}

/// State of the current epic workflow
#[derive(Debug, Clone, Default)]
pub enum EpicState {
    /// No epic active
    #[default]
    Idle,
    /// Epic is in progress
    Active { epic_id: String, epic_title: String },
    /// All tasks done, awaiting merge
    Completing { epic_id: String, epic_title: String },
}

impl EpicState {
    /// Get the current epic ID, if any
    pub fn epic_id(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Active { epic_id, .. } => Some(epic_id),
            Self::Completing { epic_id, .. } => Some(epic_id),
        }
    }

    /// Get the current epic title, if any
    pub fn epic_title(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Active { epic_title, .. } => Some(epic_title),
            Self::Completing { epic_title, .. } => Some(epic_title),
        }
    }

    /// Check if an epic is active (either in progress or completing)
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// Notification backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyBackend {
    /// Native cross-platform notifications via notify-rust
    /// Works on macOS, Linux, and Windows
    Native,
    /// Terminal bell (\x07)
    Bell,
    /// iTerm2 inline notification (\e]9;...\a)
    ITerm2,
}

impl NotifyBackend {
    /// Detect the best notification backend for the current system
    pub fn detect() -> Self {
        // Check for iTerm2 first (inline terminal notifications)
        if std::env::var("TERM_PROGRAM")
            .map(|v| v == "iTerm.app")
            .unwrap_or(false)
        {
            return Self::ITerm2;
        }

        // Use native notifications on supported platforms
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            return Self::Native;
        }

        // Fall back to terminal bell on unsupported platforms
        #[allow(unreachable_code)]
        Self::Bell
    }
}

/// Notification configuration
#[derive(Debug, Clone)]
pub struct NotifyConfig {
    /// Whether notifications are enabled
    pub enabled: bool,
    /// Backend to use
    pub backend: NotifyBackend,
    /// Also ring terminal bell (in addition to system notification)
    pub also_bell: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: NotifyBackend::detect(),
            also_bell: false,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Default `[factory] stall_threshold_secs` — how long a worker can go with
/// an in-progress task, a fresh heartbeat, but zero observable activity
/// before the director emits a `WorkerStalled` notification (cas-9829).
///
/// 300s (5 minutes) is "a few minutes" per the reported bug: the stalled
/// worker in the wild sat quiet for 10+ minutes before a supervisor caught
/// it manually, so 5 minutes gives one auto-nudge and one escalation cycle
/// well before that pain threshold, without false-firing on a worker mid
/// long-running build/test step (`RECENT_ACTIVITY_SECS` in
/// `events.rs` already tolerates gaps up to 120s for the idle gate).
pub const DEFAULT_STALL_THRESHOLD_SECS: u64 = 300;

/// Default supervisor-silence window before actionable epic state wakes the
/// supervisor. This is intentionally distinct from worker stall detection.
pub const DEFAULT_SUPERVISOR_STALL_AFTER_SECS: u64 = 10 * 60;

/// Auto-prompting configuration for factory mode events
///
/// Controls which events trigger automatic prompt injection into agent terminals.
/// Set `enabled = false` to disable all auto-prompting globally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoPromptConfig {
    /// Master switch: enable/disable all auto-prompting (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Prompt worker when a task is assigned to them (default: true)
    #[serde(default = "default_true")]
    pub on_task_assigned: bool,

    /// Prompt supervisor when a worker completes a task (default: true)
    #[serde(default = "default_true")]
    pub on_task_completed: bool,

    /// Prompt supervisor when a worker is blocked (default: true)
    #[serde(default = "default_true")]
    pub on_task_blocked: bool,

    /// Prompt supervisor when a worker becomes idle (default: true)
    #[serde(default = "default_true")]
    pub on_worker_idle: bool,

    /// Prompt supervisor when an epic is completed (default: true)
    #[serde(default = "default_true")]
    pub on_epic_completed: bool,

    /// Prompt supervisor when a worker registers and is ready (default: true)
    #[serde(default = "default_true")]
    pub on_worker_ready: bool,

    /// Notify supervisor when a worker stalls (alive heartbeat, in-progress
    /// task, no observable activity past the stall threshold) (default: true)
    #[serde(default = "default_true")]
    pub on_worker_stalled: bool,
}

impl Default for AutoPromptConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_task_assigned: true,
            on_task_completed: true,
            on_task_blocked: true,
            on_worker_idle: true,
            on_epic_completed: true,
            on_worker_ready: true,
            on_worker_stalled: true,
        }
    }
}

/// Configuration for the factory TUI
#[derive(Debug, Clone)]
pub struct FactoryConfig {
    /// Working directory for all agents (main repo)
    pub cwd: PathBuf,
    /// Number of worker agents
    pub workers: usize,
    /// Custom worker names (optional)
    pub worker_names: Vec<String>,
    /// Custom supervisor name (optional)
    pub supervisor_name: Option<String>,
    /// Supervisor CLI (codex, claude, or pi)
    pub supervisor_cli: cas_mux::SupervisorCli,
    /// Worker CLI (codex, claude, or pi)
    pub worker_cli: cas_mux::SupervisorCli,
    /// Model for supervisor (passed as --model flag to CLI)
    pub supervisor_model: Option<String>,
    /// Model for workers (passed as --model flag to CLI)
    pub worker_model: Option<String>,
    /// Reasoning effort for supervisor (passed as --effort flag; defaults to "xhigh")
    pub supervisor_effort: Option<String>,
    /// Reasoning effort for workers (passed as --effort flag; defaults to "high")
    pub worker_effort: Option<String>,
    /// Enable worktree-based worker isolation (default: true)
    pub enable_worktrees: bool,
    /// Directory where worker worktrees are created (default: ../cas-worktrees)
    pub worktree_root: Option<PathBuf>,
    /// Notification configuration
    pub notify: NotifyConfig,
    /// Use tabbed worker view instead of side-by-side (default: false)
    pub tabbed_workers: bool,
    /// Auto-prompting configuration
    pub auto_prompt: AutoPromptConfig,
    /// Enable terminal recording for time-travel playback
    pub record: bool,
    /// Session identifier for recordings (set when record=true)
    pub session_id: Option<String>,
    /// Per-agent Teams spawn configs (agent_name -> config).
    /// When set, agents are spawned with native Agent Teams CLI flags.
    pub teams_configs: std::collections::HashMap<String, cas_mux::TeamsSpawnConfig>,
    /// UUID for the team lead's Claude Code session.
    /// Used as `leadSessionId` in config.json and passed as `--session-id` to the supervisor.
    pub lead_session_id: Option<String>,
    /// Use Minions theme for boot screen, names, and colors
    pub minions_theme: bool,
    /// Resolved per-worker specs (populated by the cascade resolver).
    ///
    /// Empty until `cas_factory::spec_resolver::resolve_specs` is called and
    /// its output stored here.  Not yet consumed by spawn logic — wiring
    /// happens in a later subtask (cas-34f7f / cas-1948).
    pub resolved_worker_specs: Vec<cas_mux::WorkerSpec>,
    /// Resolved supervisor spec (populated by the cascade resolver).
    pub resolved_supervisor_spec: Option<cas_mux::WorkerSpec>,
    /// Seconds a worker may hold an in-progress task with a fresh heartbeat
    /// but no observable activity before the director flags it stalled
    /// (cas-9829). Sourced from `.cas/config.toml` `[factory]
    /// stall_threshold_secs`; defaults to [`DEFAULT_STALL_THRESHOLD_SECS`].
    pub stall_threshold_secs: u64,
    /// Seconds of supervisor MCP silence required before the forward-motion
    /// detector wakes the supervisor for an actionable focused epic.
    pub stall_after_secs: u64,
    /// Shared AI enrichment for Commander summaries and event enrichment.
    /// Default off because terminal excerpts may be sent to a third-party API.
    pub ai_enrichment: AiEnrichmentConfig,
}

impl Default for FactoryConfig {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            workers: 0, // Supervisor-only by default for EPIC planning
            worker_names: vec![],
            supervisor_name: None,
            supervisor_cli: cas_mux::SupervisorCli::Claude,
            worker_cli: cas_mux::SupervisorCli::Claude,
            supervisor_model: None,
            worker_model: None,
            supervisor_effort: None,
            worker_effort: None,
            enable_worktrees: true,
            worktree_root: None,
            notify: NotifyConfig::default(),
            tabbed_workers: false, // Side-by-side by default
            auto_prompt: AutoPromptConfig::default(),
            record: false,
            session_id: None,
            teams_configs: std::collections::HashMap::new(),
            lead_session_id: None,
            minions_theme: false,
            resolved_worker_specs: vec![],
            resolved_supervisor_spec: None,
            stall_threshold_secs: DEFAULT_STALL_THRESHOLD_SECS,
            stall_after_secs: DEFAULT_SUPERVISOR_STALL_AFTER_SECS,
            ai_enrichment: AiEnrichmentConfig::default(),
        }
    }
}
