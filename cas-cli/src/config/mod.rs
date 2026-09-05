//! Configuration management for Cassy

pub mod meta;

pub use meta::{ConfigMeta, ConfigRegistry, ConfigType, Constraint, registry};

// Re-export from cas-factory for backward compatibility
pub use cas_factory::AutoPromptConfig;

use crate::error::MemError;
use crate::ui::theme::ThemeConfig;
use serde::{Deserialize, Serialize};

pub(crate) mod hooks;
mod runtime;
mod settings;

pub use hooks::*;
pub use runtime::*;
pub use settings::*;

/// Main configuration struct
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Sync configuration (rules to .claude/rules/)
    #[serde(default)]
    pub sync: SyncConfig,

    /// Optional skill validation sandbox policy. When omitted, validation
    /// uses the degraded fallback if bubblewrap is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_validation: Option<SkillValidationConfig>,

    /// Optional stack-specific builtin skills enabled for this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillsConfig>,

    /// Cloud sync configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudSyncConfig>,

    /// Hook configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HookConfig>,

    /// Task configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TasksConfig>,

    /// Dev mode configuration for tracing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<DevConfig>,

    /// Daemon maintenance configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonSettings>,

    /// Code indexing configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeConfig>,

    /// Notification configuration for TUI alerts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<NotificationConfig>,

    /// Agent configuration for multi-agent mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,

    /// Coordination configuration for multi-agent mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordination: Option<CoordinationConfig>,

    /// Lease configuration for task claiming
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseConfig>,

    /// Verification configuration for task quality gates
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationConfig>,

    /// Worktree configuration for automatic git worktree management
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees: Option<WorktreesConfig>,

    /// Theme configuration for TUI
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeConfig>,

    /// Orchestration configuration for multi-agent sessions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration: Option<OrchestrationConfig>,

    /// Factory mode configuration for supervisor task assignment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory: Option<FactoryConfig>,

    /// `[staging]` — durable staging, configured agent scratch paths, and
    /// tmpfs/ramfs warning thresholds for hook-side write guardrails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging: Option<StagingConfig>,

    /// Telemetry configuration for anonymous usage tracking and crash reporting
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<TelemetryConfig>,

    /// Logging configuration for file-based logging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<crate::logging::LoggingConfig>,

    /// LLM configuration for harness and model selection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmConfig>,

    /// `[integrations]` — Phase 3 (cas-3efe) doctor + opt-in SessionStart
    /// banner gates for vercel/neon/github auto-integration. Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrations: Option<IntegrationsConfig>,

    /// `[issues]` — project-scoped GitHub repository for Cassy-system bug
    /// intake. No repository is inferred when this is unset because a
    /// downstream project's origin is not necessarily the Cassy upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<IssuesConfig>,

    /// `[memory]` — opt-in auto-extraction via the `session-learn` skill
    /// (cas-39f5, EPIC cas-ebea). Defaults to `None` (i.e. the auto-trigger
    /// from the `Stop` hook is disabled); set `session_learn_auto = true`
    /// to enable classifier-driven memory drafts. Manual skill invocation
    /// is unaffected by this flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryConfig>,

    /// `[release]` — operator policy for release-note routing (cas-37f6).
    /// Currently the account allowlist consulted before the one-shot
    /// `claude -p` route documented by the `cli-routing` skill. Absent by
    /// default, and the gate fails closed when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseConfig>,

    /// `[hub]` — public origin used for Commander reverse pairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubConfig>,

    /// `[project]` — project-scoped configuration (cas-1ced). Holds the
    /// canonical project slug for cloud-sync scoping. Set eagerly by
    /// `cas cloud team set` (auto-derived from git remote) or manually
    /// via `cas cloud project set <canonical-id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectConfig>,
}

impl Config {
    /// Return the complete issue-routing registry for this project. The
    /// project target remains optional, while Cassy's three component targets
    /// always resolve to their compiled defaults unless explicitly overridden.
    pub fn issue_repo_registry(&self) -> IssueRepoRegistry {
        IssueRepoRegistry::from_config(self.issues.as_ref())
    }
}

impl Config {
    /// Merge fields from `other` into `self` where `self` has `None`.
    /// Returns `true` if any field was updated.
    pub fn merge_missing(&mut self, other: &Self) -> bool {
        let mut changed = false;
        macro_rules! merge_option {
            ($field:ident) => {
                if self.$field.is_none() && other.$field.is_some() {
                    self.$field = other.$field.clone();
                    changed = true;
                }
            };
        }
        merge_option!(cloud);
        merge_option!(skill_validation);
        merge_option!(skills);
        merge_option!(hooks);
        merge_option!(tasks);
        merge_option!(dev);
        merge_option!(daemon);
        merge_option!(code);
        merge_option!(notifications);
        merge_option!(agent);
        merge_option!(coordination);
        merge_option!(lease);
        merge_option!(verification);
        merge_option!(worktrees);
        merge_option!(theme);
        merge_option!(orchestration);
        merge_option!(factory);
        merge_option!(staging);
        merge_option!(telemetry);
        merge_option!(logging);
        merge_option!(llm);
        merge_option!(integrations);
        merge_option!(issues);
        merge_option!(memory);
        merge_option!(hub);
        merge_option!(project);
        merge_option!(release);
        changed
    }

    /// Get daemon maintenance config with defaults.
    pub fn daemon(&self) -> DaemonSettings {
        self.daemon.clone().unwrap_or_default()
    }

    /// Whether a probed Claude account e-mail may run the one-shot
    /// `claude -p` route (`release.claude_account_allowlist`).
    ///
    /// The gate fails closed when `[release]` is absent, so no account is
    /// approved until a project configures one.
    pub fn claude_account_allowed(&self, email: &str) -> bool {
        self.release
            .as_ref()
            .is_some_and(|release| release.claude_account_allowed(email))
    }
}

mod access;
pub use access::{
    get_telemetry_consent, global_cas_dir, load_global_config, prompt_telemetry_consent,
    save_global_config, set_telemetry_consent,
};

#[cfg(test)]
mod mod_tests;
