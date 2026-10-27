//! Configuration management for Cassy

pub mod meta;
mod accounts;
pub use accounts::{AccountSelection, AccountsConfig};

pub use meta::{ConfigMeta, ConfigRegistry, ConfigType, Constraint, registry};

// Re-export from cas-factory for backward compatibility
pub use cas_factory::AutoPromptConfig;

use crate::error::MemError;
use crate::ui::theme::ThemeConfig;
use serde::{Deserialize, Serialize};

pub(crate) mod hooks;
pub mod operator_policy;
mod runtime;
mod settings;
mod qa_capture;
pub use qa_capture::QaCaptureConfig;
mod upkeep;
pub use upkeep::{ProjectOverviewConfig, UpkeepConfig, UpkeepStage};
pub(crate) use upkeep::refresh_upkeep;

pub use hooks::*;
pub use runtime::*;
pub use settings::*;

/// Main configuration struct
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Shared account recommendation policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<AccountsConfig>,
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

    /// User-facing QA gate configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qa: Option<QaConfig>,

    /// Paths and directory globs used by overview freshness and G6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_overview: Option<ProjectOverviewConfig>,

    /// Delivery-scoped documentation upkeep stages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upkeep: Option<UpkeepConfig>,

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

    /// Retired `[jev]` section (the Jev client was removed by cas-4971).
    /// Accepted so older config files still load, never written back, and
    /// noted once per process by [`Config::load`].
    #[serde(default, rename = "jev", skip_serializing)]
    pub(crate) retired_jev: Option<serde::de::IgnoredAny>,

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

    /// `[history]` — GitHub repository for the code-history document index.
    /// When unset, history uses the checkout's GitHub `origin`; this setting
    /// must not be confused with `[issues] repo`, which is bug intake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryConfig>,

    /// `[memory]` — opt-in auto-extraction via the `session-learn` skill
    /// (cas-39f5, EPIC cas-ebea). Defaults to `None` (i.e. the auto-trigger
    /// from the `Stop` hook is disabled); set `session_learn_auto = true`
    /// to enable classifier-driven memory drafts. Manual skill invocation
    /// is unaffected by this flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryConfig>,

    /// Slack transport policy, enabled for every session by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slack: Option<SlackConfig>,

    /// `[peers]` — same-repo peer supervisor scope (cas-d58d).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peers: Option<PeersConfig>,

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

    /// Machine-wide control for the managed Viktor upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viktor: Option<ViktorConfig>,
}

/// Host `.cas` directory used for settings that apply across projects.
pub fn host_config_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".cas")
}

impl Config {
    /// Return the complete issue-routing registry for this project. The
    /// project target remains optional, while Cassy's three component targets
    /// always resolve to their compiled defaults unless explicitly overridden.
    pub fn issue_repo_registry(&self) -> IssueRepoRegistry {
        IssueRepoRegistry::from_config(self.issues.as_ref())
    }

    /// Stored legacy destination selected by an explicitly blank Cassy component.
    /// Bug filing never infers an origin; CLI provenance must use the same rule.
    pub(crate) fn cassy_issue_legacy_repo(&self) -> Option<&str> {
        let issues = self.issues.as_ref()?;
        issues.components.as_ref()?.cassy.as_deref()
            .filter(|repo| repo.trim().is_empty())
            .and(issues.repo.as_deref())
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
    }

    /// Resolve the project destination in the context of a local checkout.
    /// This read-only view never materializes inferred values in config.toml.
    pub fn issue_repo_registry_for_project(&self, cas_root: &std::path::Path) -> IssueRepoRegistry {
        let mut registry = self.issue_repo_registry();
        if registry.project.is_none() {
            registry.project = crate::cloud::git_origin_url(cas_root)
                .and_then(|url| crate::cli::integrate::github::parse_origin_url(&url))
                .map(|repo| repo.full_name())
                .filter(|repo| crate::gh_graphql::split_repo(repo).is_ok());
        }
        registry
    }

    /// Effective config view for commands with a known project root. Stored
    /// values remain available through `get` for editing and serialization.
    pub fn get_resolved(&self, key: &str, cas_root: &std::path::Path) -> Option<String> {
        if key == "issues.repo" {
            Some(
                self.issue_repo_registry_for_project(cas_root)
                    .project
                    .unwrap_or_default(),
            )
        } else {
            self.get(key)
        }
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
        merge_option!(qa);
        merge_option!(upkeep);
        merge_option!(project_overview);
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
        merge_option!(history);
        merge_option!(memory);
        merge_option!(hub);
        merge_option!(project);
        merge_option!(release);
        merge_option!(slack);
        merge_option!(viktor);
        changed
    }

    /// Get daemon maintenance config with defaults.
    pub fn daemon(&self) -> DaemonSettings {
        self.daemon.clone().unwrap_or_default()
    }

    /// Managed Viktor is enabled by default for existing installations.
    pub fn viktor_enabled(&self) -> bool {
        self.viktor.as_ref().is_none_or(ViktorConfig::enabled)
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
pub(crate) use access::{ConfigRetirement, retire_config};
pub(crate) use access::validate_notification_webhook_url;
pub(crate) use access::{RetiredConfigNotice, RetiredNoticeScope, represented_retired_config};
pub use access::{
    get_telemetry_consent, global_cas_dir, load_global_config, prompt_telemetry_consent,
    save_global_config, set_telemetry_consent,
};

pub(crate) use access::io::{
    atomic_replace_project_config, commander_mirror_resume_floor, lock_project_config,
};

#[cfg(test)]
pub(crate) use access::io::{
    atomic_replace_project_config_via, atomic_replace_project_config_via_with_created_hook,
};

#[cfg(test)]
mod mod_tests;
