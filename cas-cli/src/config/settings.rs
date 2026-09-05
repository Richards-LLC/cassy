use cas_factory::AutoPromptConfig;
use serde::{Deserialize, Serialize};

/// Hub origin configuration. Lives at `[hub]` in `.cas/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubConfig {
    /// Public origin used when authorizing a Commander page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
}

/// Project-scoped GitHub issue intake configuration. Lives at `[issues]` in
/// `.cas/config.toml`.
///
/// `repo` is deliberately optional: Cassy installations do not share one
/// upstream repository, and inferring the current git origin would route Cassy
/// bugs into a downstream consumer's issue tracker.
pub const DEFAULT_CASSY_ISSUES_REPO: &str = "Richards-LLC/cassy";
pub const DEFAULT_MECHA_CASSY_ISSUES_REPO: &str = "Richards-LLC/mecha-cassy";
pub const DEFAULT_CLOUD_ISSUES_REPO: &str = "Richards-LLC/petra-stella-cloud";

/// Optional overrides for the issue repositories of Cassy's component
/// projects. The compiled defaults remain authoritative when a field is
/// absent, so fresh projects can route reports without gaining generated
/// configuration keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueComponentsConfig {
    /// Repository for Cassy runtime, hooks, MCP, factory, and skills bugs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cassy: Option<String>,
    /// Repository for MechaCassy Slack hub bugs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mecha_cassy: Option<String>,
    /// Repository for Cassy Cloud sync, hub relay, and pairing bugs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
}

/// Resolved issue destinations exposed to user-facing diagnostics and
/// directives. `project` is intentionally optional because it has no safe
/// compiled default; the three Cassy component repositories always resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRepoRegistry {
    pub project: Option<String>,
    pub cassy: String,
    pub mecha_cassy: String,
    pub cloud: String,
}

impl IssueComponentsConfig {
    fn resolved_value(value: Option<&String>, default: &'static str) -> String {
        value
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default)
            .to_string()
    }

    fn resolved_registry(&self, project: Option<&String>) -> IssueRepoRegistry {
        IssueRepoRegistry {
            project: project
                .map(String::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            cassy: Self::resolved_value(self.cassy.as_ref(), DEFAULT_CASSY_ISSUES_REPO),
            mecha_cassy: Self::resolved_value(
                self.mecha_cassy.as_ref(),
                DEFAULT_MECHA_CASSY_ISSUES_REPO,
            ),
            cloud: Self::resolved_value(self.cloud.as_ref(), DEFAULT_CLOUD_ISSUES_REPO),
        }
    }
}

impl IssueRepoRegistry {
    /// Resolve a registry from an optional issues config. Component defaults
    /// intentionally live here rather than in serde defaults so serialization
    /// does not materialize them in a project's config.toml.
    pub fn from_config(config: Option<&IssuesConfig>) -> Self {
        let Some(config) = config else {
            return IssueComponentsConfig::default().resolved_registry(None);
        };
        config
            .components
            .as_ref()
            .cloned()
            .unwrap_or_default()
            .resolved_registry(config.repo.as_ref())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssuesConfig {
    /// GitHub repository in `owner/repo` form used by Cassy-system bug filing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,

    /// Optional per-component issue repository overrides. Omitted fields use
    /// the compiled defaults in [`IssueRepoRegistry::from_config`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<IssueComponentsConfig>,
}

impl IssuesConfig {
    /// Resolve this project's issue destinations, retaining the optional
    /// project repository semantics of the legacy `issues.repo` setting.
    pub fn resolved_registry(&self) -> IssueRepoRegistry {
        IssueRepoRegistry::from_config(Some(self))
    }
}

/// Release-routing configuration. Lives at `[release]` in `.cas/config.toml`.
///
/// The one-shot `claude -p` route used for release-note posting is gated on the
/// account that would make the call (`claude auth status --json`). Which
/// accounts are approved is operator policy, not a property of Cassy, so it is
/// configured here instead of being written into the shipped `cli-routing`
/// skill (cas-37f6). The list is empty by default and the gate fails closed:
/// an unconfigured project approves no Claude account at all.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseConfig {
    /// E-mail addresses approved for the one-shot Claude route. Compared
    /// case-insensitively against the probed `email` field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude_account_allowlist: Vec<String>,
}

impl ReleaseConfig {
    /// Whether a probed account e-mail is on the configured allowlist.
    ///
    /// Fails closed: an empty allowlist approves nobody, so a project that has
    /// not opted in cannot spend a Claude account by accident.
    pub fn claude_account_allowed(&self, email: &str) -> bool {
        let probed = email.trim().to_ascii_lowercase();
        if probed.is_empty() {
            return false;
        }
        self.claude_account_allowlist
            .iter()
            .any(|allowed| allowed.trim().to_ascii_lowercase() == probed)
    }
}

/// Project-scoped configuration. Lives at `[project]` in `.cas/config.toml`.
///
/// `canonical_id` is the project's canonical slug used to scope cloud-sync
/// pushes/pulls. When set, it takes precedence over the working-directory
/// folder-name fallback baked into `resolve_canonical_id()`. Set eagerly
/// by `cas cloud team set` (auto-derived from git remote when possible)
/// or manually via `cas cloud project set <canonical-id>`. Closes the
/// onboarding gap (cas-1ced / EPIC cas-ffc4 hypothesis #3) where a clone
/// directory named differently from the canonical slug routed
/// pushes/pulls to a phantom project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Canonical project slug (e.g. `github.com/owner/repo`). When absent,
    /// the resolver falls back to the parent-directory folder name, then
    /// to a path-hash slug for the fs-root edge case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
}

/// Sync configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Whether auto-sync is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Target directory for synced rules (relative to project root)
    #[serde(default = "default_target")]
    pub target: String,

    /// Minimum helpful votes before syncing
    #[serde(default = "default_min_helpful")]
    pub min_helpful: i32,

    /// Minimum evidence events before a Draft or Stale rule can become Proven.
    /// Existing Proven rules keep their status when this setting changes.
    #[serde(default = "default_promotion_threshold")]
    pub promotion_threshold: i32,

    /// Minimum negative evidence events before a Proven rule becomes Stale.
    /// Existing Proven rules are not evaluated until new negative evidence is
    /// observed or an explicit sync reads retrieval outcomes.
    #[serde(default = "default_demotion_threshold")]
    pub demotion_threshold: i32,

    /// Evidence sources eligible to satisfy `promotion_threshold`. Supported
    /// values are `helpful` (explicit rule feedback) and `retrieval`
    /// (privacy-safe retrieval outcome aggregates).
    #[serde(default = "default_promotion_evidence")]
    pub promotion_evidence: Vec<String>,
}

/// Project-scoped optional builtin skills. Skills in this list are enabled
/// even when stack detection does not find their usual language/framework.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Optional builtin skill ids, such as `fallow` or
    /// `cas-nuxt-playwright`.
    #[serde(default)]
    pub optional: Vec<String>,
}

/// Skill validation sandbox policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillValidationConfig {
    /// Require bubblewrap on platforms where it is supported instead of
    /// allowing the env-scrubbed plain-shell fallback.
    #[serde(default)]
    pub require_sandbox: bool,
}

impl Default for SkillValidationConfig {
    fn default() -> Self {
        Self {
            require_sandbox: false,
        }
    }
}

/// Task configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksConfig {
    /// Nudge to commit changes when closing a task
    #[serde(default)]
    pub commit_nudge_on_close: bool,

    /// Block agent exit while open tasks remain (claimed tasks, epic subtasks, session-created)
    #[serde(default = "default_true")]
    pub block_exit_on_open: bool,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            commit_nudge_on_close: false,
            block_exit_on_open: true,
        }
    }
}

/// Factory configuration for multi-agent sessions (native TUI)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationConfig {
    /// Default number of worker agents (default: 0 for supervisor-only startup)
    #[serde(default = "default_orchestration_pane_count")]
    pub default_workers: u8,

    /// Auto-prompting configuration for factory events
    #[serde(default)]
    pub auto_prompt: AutoPromptConfig,
}

fn default_orchestration_pane_count() -> u8 {
    0 // Supervisor-only by default for EPIC planning
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            default_workers: default_orchestration_pane_count(),
            auto_prompt: AutoPromptConfig::default(),
        }
    }
}

/// Factory mode configuration for supervisor task assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryConfig {
    /// Durable, per-task proof/artifact directory. Factory workers may write
    /// under this root in addition to their worktree. If unset, the hook
    /// resolves a real-disk fallback under `~/.cas/artifacts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts_root: Option<String>,

    /// Warn when assigning tasks to workers with stale worktrees
    #[serde(default = "default_true")]
    pub warn_stale_assignment: bool,

    /// Block task assignment to workers with stale worktrees (if commits behind >= threshold)
    #[serde(default)]
    pub block_stale_assignment: bool,

    /// Number of commits behind the sync target before considering a worktree stale
    #[serde(default = "default_stale_threshold")]
    pub stale_threshold_commits: u32,

    /// Cap on `CARGO_BUILD_JOBS` exported into each factory worker's env.
    ///
    /// Purpose: prevent the multi-worker cargo thundering-herd that
    /// saturates the host and wedges Claude Code workers in the JS
    /// crash-screen state (cas-4513 + cas-0bf4). Each worker runs its
    /// own `target/` dir and its own rustc jobs; without this cap,
    /// peak concurrency is `workers × num_cpus` rustc threads.
    ///
    /// - `"auto"` (default): cas-pty computes
    ///   `max(2, available_parallelism() / 4)`. The "÷4" assumes up to
    ///   4 concurrent workers — the common factory-mode topology on a
    ///   16-thread dev box. Override via `CAS_FACTORY_CARGO_BUILD_JOBS`
    ///   if the supervisor's scale differs.
    /// - Any numeric string (e.g. `"4"`): exported verbatim.
    #[serde(default = "default_auto")]
    pub cargo_build_jobs: String,

    /// When true, prefix each worker's spawn command with `nice -n 10`
    /// so cargo-driven rustc jobs run at a lower scheduling priority
    /// than the supervisor's Claude Code event loop. Workers still
    /// contend equally among themselves, but the supervisor pane
    /// stays responsive under load — which is what keeps the factory
    /// steerable when a worker storm starts.
    ///
    /// Default `true`. Flip to `false` for single-worker sessions or
    /// when benchmarking, since the priority drop does slow individual
    /// cargo builds under contention.
    #[serde(default = "default_true")]
    pub nice_cargo: bool,

    /// Seconds a worker may hold an in-progress task with a fresh heartbeat
    /// but zero observable activity (no file edits, commits, or subagent
    /// events) before the director flags it `WorkerStalled` and notifies
    /// the supervisor (cas-9829). Distinct from heartbeat liveness — a
    /// worker can heartbeat every tick while having produced nothing for
    /// the current task; this is the "alive but stuck" signal that gap
    /// left silent.
    ///
    /// Default [`cas_factory::DEFAULT_STALL_THRESHOLD_SECS`] (5 minutes,
    /// "a few minutes" per the bug report).
    #[serde(default = "default_stall_threshold_secs")]
    pub stall_threshold_secs: u64,

    /// Seconds before an unread critical/high-priority coordination message
    /// bounces a delivery-stalled notice back to its sender.
    #[serde(default = "default_delivery_stalled_priority_secs")]
    pub delivery_stalled_priority_secs: u64,

    /// Seconds before an unread normal-priority coordination message bounces a
    /// delivery-stalled notice back to its sender.
    #[serde(default = "default_delivery_stalled_normal_secs")]
    pub delivery_stalled_normal_secs: u64,

    /// Base branch for epic auto-branch creation, and the default
    /// `sync_all_workers` / worker-spawn base when no epic is pinned.
    ///
    /// `None` (default) falls back to the repo's detected default branch
    /// (`detect_default_branch()` — origin/HEAD, then `init.defaultBranch`,
    /// then common names). Staging-first shops set
    /// `epic_base_branch = "staging"` so epic/worker branches are cut from
    /// staging without a manual `git branch -f epic/... origin/staging`
    /// correction after the fact (BUG/FEATURE 2026-07-08, cas-b082).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epic_base_branch: Option<String>,

    /// Refuse to silently fall back when a resolved worker/supervisor spec
    /// requests Codex but Codex is unavailable — bail with an actionable
    /// error instead of rewriting to Claude (cas-7199 / cas-a487). OR'd
    /// with the `--strict-cli` CLI flag; either being true enables strict
    /// mode. Default `false`: fall back to Claude with a loud warning.
    #[serde(default)]
    pub strict_cli: bool,

    /// Filesystem usage percentage that surfaces factory Cargo cache pressure.
    /// This is intentionally below ENOSPC so operators have time to inspect a
    /// dry-run and reclaim stale, regenerable `target/` trees.
    #[serde(default = "default_target_cache_high_watermark_percent")]
    pub target_cache_high_watermark_percent: u8,

    /// Cleanup stops once projected filesystem usage reaches this percentage.
    #[serde(default = "default_target_cache_low_watermark_percent")]
    pub target_cache_low_watermark_percent: u8,

    /// A cache with a write newer than this many seconds is never reclaimed.
    #[serde(default = "default_target_cache_min_idle_secs")]
    pub target_cache_min_idle_secs: u64,

    /// Number of the newest otherwise-stale worker caches retained as warm
    /// build caches even while the filesystem is above the high watermark.
    #[serde(default = "default_target_cache_retention_count")]
    pub target_cache_retention_count: usize,

    /// AI enrichment for Commander summaries and events. DEFAULT OFF: enabling this sends
    /// redacted terminal transcript excerpts to a third-party API from a
    /// machine that may hold secrets. Configure an OpenAI-compatible local
    /// endpoint when transcripts must not leave the machine or tailnet.
    #[serde(default)]
    pub ai_enrichment: cas_factory::AiEnrichmentConfig,
}

/// Durable staging configuration for large generated artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingConfig {
    /// Preferred durable directory for large temporary artifacts. When set,
    /// tmpfs guardrail warnings tell agents to restate this approved location
    /// before continuing large writes.
    #[serde(
        default,
        alias = "large_artifact_dir",
        skip_serializing_if = "Option::is_none"
    )]
    pub staging_dir: Option<String>,

    /// Root for ephemeral agent-created files such as logs, generated fixtures,
    /// and scratch checkouts. When configured, the PreToolUse workspace guard
    /// permits writes below this root in addition to the worktree and durable
    /// factory artifacts root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch_root: Option<String>,

    /// Warn when cumulative session writes or tmpfs usage growth crosses this
    /// many bytes on a tmpfs/ramfs-backed mount. Default: 1 GiB.
    #[serde(default = "default_tmpfs_warning_threshold_bytes")]
    pub tmpfs_warning_threshold_bytes: u64,
}

pub const DEFAULT_TMPFS_WARNING_THRESHOLD_BYTES: u64 = 1024 * 1024 * 1024;

fn default_tmpfs_warning_threshold_bytes() -> u64 {
    DEFAULT_TMPFS_WARNING_THRESHOLD_BYTES
}

impl Default for StagingConfig {
    fn default() -> Self {
        Self {
            staging_dir: None,
            scratch_root: None,
            tmpfs_warning_threshold_bytes: default_tmpfs_warning_threshold_bytes(),
        }
    }
}

fn default_stale_threshold() -> u32 {
    1
}

fn default_auto() -> String {
    "auto".to_string()
}

fn default_stall_threshold_secs() -> u64 {
    cas_factory::DEFAULT_STALL_THRESHOLD_SECS
}

fn default_delivery_stalled_priority_secs() -> u64 {
    10 * 60
}

fn default_delivery_stalled_normal_secs() -> u64 {
    30 * 60
}

fn default_target_cache_high_watermark_percent() -> u8 {
    85
}

fn default_target_cache_low_watermark_percent() -> u8 {
    75
}

fn default_target_cache_min_idle_secs() -> u64 {
    60 * 60
}

fn default_target_cache_retention_count() -> usize {
    1
}

impl Default for FactoryConfig {
    fn default() -> Self {
        Self {
            artifacts_root: None,
            warn_stale_assignment: true,
            block_stale_assignment: true,
            stale_threshold_commits: default_stale_threshold(),
            cargo_build_jobs: default_auto(),
            nice_cargo: true,
            stall_threshold_secs: default_stall_threshold_secs(),
            delivery_stalled_priority_secs: default_delivery_stalled_priority_secs(),
            delivery_stalled_normal_secs: default_delivery_stalled_normal_secs(),
            epic_base_branch: None,
            strict_cli: false,
            target_cache_high_watermark_percent: default_target_cache_high_watermark_percent(),
            target_cache_low_watermark_percent: default_target_cache_low_watermark_percent(),
            target_cache_min_idle_secs: default_target_cache_min_idle_secs(),
            target_cache_retention_count: default_target_cache_retention_count(),
            ai_enrichment: cas_factory::AiEnrichmentConfig::default(),
        }
    }
}

/// Resolve the durable artifact parent shared by the factory workspace
/// contract and completion-receipt boundary. `~/.cas/artifacts` is a
/// real-disk fallback, never `/tmp`.
pub fn resolved_factory_artifacts_root(configured: Option<&str>) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match configured.map(str::trim).filter(|value| !value.is_empty()) {
        Some("~") => home.unwrap_or_else(|| std::path::PathBuf::from(".cas/artifacts")),
        Some(value) if value.starts_with("~/") => home
            .map(|base| base.join(&value[2..]))
            .unwrap_or_else(|| std::path::PathBuf::from(value)),
        Some(value) => std::path::PathBuf::from(value),
        None => home
            .map(|base| base.join(".cas/artifacts"))
            .unwrap_or_else(|| std::path::PathBuf::from(".cas/artifacts")),
    }
}

/// Code indexing configuration for background code indexing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeConfig {
    /// Whether background code indexing is enabled.
    ///
    /// cas-499c (operator ruling): default **true**, no opt-in. The symbol index had never run
    /// on any install because this defaulted to false, so `code_search` was permanently a stub.
    /// Cost is bounded by the daemon's idleness gate, which is deliberately retained.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Paths to watch for code changes (relative to project root)
    #[serde(default = "default_code_watch_paths")]
    pub watch_paths: Vec<String>,

    /// Glob patterns for directories/files to exclude from indexing
    #[serde(default = "default_code_exclude_patterns")]
    pub exclude_patterns: Vec<String>,

    /// File extensions to index (without leading dot)
    #[serde(default = "default_code_extensions")]
    pub extensions: Vec<String>,

    /// How often to run full code indexing (seconds)
    #[serde(default = "default_code_index_interval")]
    pub index_interval_secs: u64,

    /// Debounce time for file watcher events (milliseconds)
    #[serde(default = "default_code_debounce")]
    pub debounce_ms: u64,
}

fn default_code_watch_paths() -> Vec<String> {
    vec!["src".into(), "lib".into(), "crates".into()]
}

fn default_code_exclude_patterns() -> Vec<String> {
    vec![
        "target/**".into(),
        "node_modules/**".into(),
        ".git/**".into(),
        "dist/**".into(),
        "build/**".into(),
        "_build/**".into(),
        "deps/**".into(),
        "vendor/**".into(),
    ]
}

fn default_code_extensions() -> Vec<String> {
    vec![
        "rs".into(),
        "ts".into(),
        "tsx".into(),
        "js".into(),
        "jsx".into(),
        "py".into(),
        "go".into(),
        "ex".into(),
        "exs".into(),
        "rb".into(),
        "java".into(),
        "kt".into(),
        "swift".into(),
    ]
}

fn default_code_index_interval() -> u64 {
    60 // 1 minute
}

fn default_code_debounce() -> u64 {
    500 // 500ms
}

impl Default for CodeConfig {
    fn default() -> Self {
        Self {
            // cas-499c: on by default; the idle gate (mcp/daemon.rs) is what keeps it polite.
            enabled: true,
            watch_paths: default_code_watch_paths(),
            exclude_patterns: default_code_exclude_patterns(),
            extensions: default_code_extensions(),
            index_interval_secs: default_code_index_interval(),
            debounce_ms: default_code_debounce(),
        }
    }
}

/// Notification configuration for TUI alerts and hook notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Master switch for notifications
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Play terminal bell on new notifications
    #[serde(default = "default_true")]
    pub sound_enabled: bool,

    /// How long to display notifications (seconds)
    #[serde(default = "default_display_duration")]
    pub display_duration_secs: u64,

    /// Maximum notifications to display at once
    #[serde(default = "default_max_visible")]
    pub max_visible: usize,

    /// Task notification settings
    #[serde(default)]
    pub tasks: TaskNotifications,

    /// Entry/memory notification settings
    #[serde(default)]
    pub entries: EntryNotifications,

    /// Rule notification settings
    #[serde(default)]
    pub rules: RuleNotifications,

    /// Skill notification settings
    #[serde(default)]
    pub skills: SkillNotifications,

    // === Hook notification settings (for Notification hook) ===
    /// Notify on permission prompts (Claude needs user approval)
    #[serde(default)]
    pub on_permission_prompt: bool,

    /// Notify when Claude is idle and waiting for input
    #[serde(default)]
    pub on_idle_prompt: bool,

    /// Notify on successful authentication
    #[serde(default)]
    pub on_auth_success: bool,

    /// Optional webhook URL for Slack/Discord integration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
}

/// Task-specific notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotifications {
    /// Notify when a task is created
    #[serde(default = "default_true")]
    pub on_created: bool,

    /// Notify when a task is started
    #[serde(default = "default_true")]
    pub on_started: bool,

    /// Notify when a task is closed
    #[serde(default = "default_true")]
    pub on_closed: bool,

    /// Notify when a task is updated (off by default - too noisy)
    #[serde(default)]
    pub on_updated: bool,
}

impl Default for TaskNotifications {
    fn default() -> Self {
        Self {
            on_created: true,
            on_started: true,
            on_closed: true,
            on_updated: false,
        }
    }
}

/// Entry/memory notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryNotifications {
    /// Notify when an entry is added
    #[serde(default = "default_true")]
    pub on_added: bool,

    /// Notify when an entry is updated (off by default)
    #[serde(default)]
    pub on_updated: bool,

    /// Notify when an entry is deleted
    #[serde(default = "default_true")]
    pub on_deleted: bool,
}

impl Default for EntryNotifications {
    fn default() -> Self {
        Self {
            on_added: true,
            on_updated: false,
            on_deleted: true,
        }
    }
}

/// Rule notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleNotifications {
    /// Notify when a rule is created
    #[serde(default = "default_true")]
    pub on_created: bool,

    /// Notify when a rule is promoted to Proven
    #[serde(default = "default_true")]
    pub on_promoted: bool,

    /// Notify when a rule is demoted (off by default)
    #[serde(default)]
    pub on_demoted: bool,
}

impl Default for RuleNotifications {
    fn default() -> Self {
        Self {
            on_created: true,
            on_promoted: true,
            on_demoted: false,
        }
    }
}

/// Skill notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillNotifications {
    /// Notify when a skill is created
    #[serde(default = "default_true")]
    pub on_created: bool,

    /// Notify when a skill is enabled
    #[serde(default = "default_true")]
    pub on_enabled: bool,

    /// Notify when a skill is disabled (off by default)
    #[serde(default)]
    pub on_disabled: bool,
}

impl Default for SkillNotifications {
    fn default() -> Self {
        Self {
            on_created: true,
            on_enabled: true,
            on_disabled: false,
        }
    }
}

fn default_display_duration() -> u64 {
    5 // 5 seconds
}

fn default_max_visible() -> usize {
    3
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sound_enabled: true,
            display_duration_secs: default_display_duration(),
            max_visible: default_max_visible(),
            tasks: TaskNotifications::default(),
            entries: EntryNotifications::default(),
            rules: RuleNotifications::default(),
            skills: SkillNotifications::default(),
            // Hook notification settings (disabled by default)
            on_permission_prompt: false,
            on_idle_prompt: false,
            on_auth_success: false,
            webhook_url: None,
        }
    }
}

/// Cloud sync configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSyncConfig {
    /// Whether auto-sync is enabled (when logged in)
    #[serde(default = "default_true")]
    pub auto_sync: bool,

    /// How often to sync (seconds)
    #[serde(default = "default_cloud_sync_interval")]
    pub interval_secs: u64,

    /// Pull from cloud on MCP server startup
    #[serde(default = "default_true")]
    pub pull_on_start: bool,

    /// Maximum retry attempts for failed syncs
    #[serde(default = "default_max_retries")]
    pub max_retries: i32,

    /// Warn when the number of retryable local changes reaches this count.
    #[serde(default = "default_cloud_queue_pending_warning")]
    pub queue_pending_warning: usize,

    /// Warn when the oldest retryable local change is this old (seconds).
    #[serde(default = "default_cloud_queue_oldest_warning_secs")]
    pub queue_oldest_warning_secs: u64,
}

fn default_cloud_sync_interval() -> u64 {
    60 // 1 minute
}

fn default_max_retries() -> i32 {
    5
}

fn default_cloud_queue_pending_warning() -> usize {
    200
}

fn default_cloud_queue_oldest_warning_secs() -> u64 {
    6 * 60 * 60
}

impl Default for CloudSyncConfig {
    fn default() -> Self {
        Self {
            auto_sync: true,
            interval_secs: default_cloud_sync_interval(),
            pull_on_start: true,
            max_retries: default_max_retries(),
            queue_pending_warning: default_cloud_queue_pending_warning(),
            queue_oldest_warning_secs: default_cloud_queue_oldest_warning_secs(),
        }
    }
}

/// Development mode configuration for tracing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevConfig {
    /// Enable dev mode tracing
    #[serde(default)]
    pub dev_mode: bool,

    /// Trace CLI command executions
    #[serde(default = "default_true")]
    pub trace_commands: bool,

    /// Trace store operations (add, update, delete, get)
    #[serde(default = "default_true")]
    pub trace_store_ops: bool,

    /// Trace Claude API calls with full prompts/responses
    #[serde(default = "default_true")]
    pub trace_claude_api: bool,

    /// Trace hook events
    #[serde(default = "default_true")]
    pub trace_hooks: bool,

    /// Days to retain traces before auto-cleanup
    #[serde(default = "default_trace_retention")]
    pub trace_retention_days: i64,
}

fn default_trace_retention() -> i64 {
    7
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            dev_mode: false,
            trace_commands: true,
            trace_store_ops: true,
            trace_claude_api: true,
            trace_hooks: true,
            trace_retention_days: 7,
        }
    }
}

/// Daemon maintenance configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSettings {
    /// Maximum total bytes for compressed event/recording archives. Oldest
    /// archive files are evicted first when this cap is exceeded.
    #[serde(default = "default_archive_max_bytes")]
    pub archive_max_bytes: u64,

    /// Days to retain compressed event/recording archives. Zero keeps them
    /// forever (legacy compatibility; maintenance now uses archive_max_bytes).
    #[serde(default)]
    pub archive_retention_days: u64,

    /// Enable the bounded injected-relevance sampling pass.
    #[serde(default = "default_relevance_sampling_enabled")]
    pub relevance_sampling_enabled: bool,

    /// Minimum cadence between injected-relevance sampling passes, in seconds.
    #[serde(default = "default_relevance_sampling_interval_secs")]
    pub relevance_sampling_interval_secs: u64,

    /// Maximum number of injected result rows offered to the judge per pass.
    #[serde(default = "default_relevance_sampling_sample_size")]
    pub relevance_sampling_sample_size: usize,
}

fn default_archive_max_bytes() -> u64 {
    cas_store::DEFAULT_TRACE_ARCHIVE_MAX_BYTES
}

fn default_relevance_sampling_enabled() -> bool {
    true
}

fn default_relevance_sampling_interval_secs() -> u64 {
    7 * 24 * 60 * 60
}

fn default_relevance_sampling_sample_size() -> usize {
    20
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            archive_max_bytes: default_archive_max_bytes(),
            archive_retention_days: 0,
            relevance_sampling_enabled: default_relevance_sampling_enabled(),
            relevance_sampling_interval_secs: default_relevance_sampling_interval_secs(),
            relevance_sampling_sample_size: default_relevance_sampling_sample_size(),
        }
    }
}

/// Telemetry configuration for anonymous usage tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether telemetry is enabled (default: false, opt-in via CAS_TELEMETRY=1)
    #[serde(default)]
    pub enabled: bool,

    /// Anonymous user ID (generated on first run)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous_id: Option<String>,

    /// Whether user has given consent for telemetry (None = not asked yet)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consent_given: Option<bool>,
}

/// Stock worker model used as the final fallback for `[llm.worker.model]`.
///
/// Applied by `LlmConfig::model_for_role("worker")` when both the role-
/// specific override (`[llm.worker.model]`) and the top-level fallback
/// (`[llm.model]`) are unset. New installs and upgraders without an
/// explicit `[llm.worker]` block pick up this model automatically; users
/// who want a different worker model add an explicit `[llm.worker] model
/// = "..."` to their `.cas/config.toml`.
///
/// The standard worker tier is Luna/xhigh; Sol/high is reserved for explicit
/// heavy and frontier routing. Terra is suspended as a routing target pending
/// an explicit operator re-enable. See cas-05e3, cas-fbac, cas-e352.
pub const STOCK_WORKER_MODEL: &str = "gpt-5.6-luna";

/// Stock worker reasoning effort used as the final fallback for
/// `[llm.worker.reasoning_effort]`. Same chain rules as
/// [`STOCK_WORKER_MODEL`]: applied only when both the role override and
/// the top-level `[llm] reasoning_effort` are unset. Luna is only used at its
/// current maximum Cassy effort, xhigh. See cas-05e3, cas-fbac, cas-e352.
pub const STOCK_WORKER_REASONING_EFFORT: &str = "xhigh";

/// Stock worker harness used as the final fallback for `[llm.worker.harness]`.
/// Same chain rules as [`STOCK_WORKER_MODEL`]: applied only when both the
/// role override and the top-level `[llm] harness` are unset.
///
/// Added in cas-fbac alongside the Codex stock-model flip. The model constant
/// alone cannot flip the harness — [`harness_for_role`][LlmConfig::harness_for_role]
/// used to resolve unconditionally to `"claude"` for every role (the
/// top-level `harness` field was a plain `String`, never `None`), so a
/// worker with no explicit config would have come up as the **Claude**
/// harness attempting to run a Codex model string — broken.
/// This constant plus the `Option<String>` top-level `harness` field give
/// the worker role its own stock floor, exactly mirroring the model/effort
/// treatment, while the supervisor and any other role keep resolving to the
/// literal `"claude"` default when unset.
pub const STOCK_WORKER_HARNESS: &str = "codex";

/// LLM configuration for harness and model selection
///
/// Controls which CLI harness (Claude or Codex) is used and which model
/// each harness runs. Per-role overrides allow different configurations
/// for supervisor vs worker agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Which CLI harness to use: "claude" or "codex".
    ///
    /// `None` = use the backend's own default via [`LlmConfig::harness_for_role`]:
    /// the literal `"claude"` for supervisor/unknown roles, or
    /// [`STOCK_WORKER_HARNESS`] for the worker role. Mirrors [`model`]'s
    /// `Option<String>` shape (added in cas-fbac) so "unset" is
    /// distinguishable from an explicit `harness = "claude"`.
    ///
    /// [`model`]: LlmConfig::model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,

    /// Model to use within the harness (e.g., "claude-sonnet-4-5-20250929", "gpt-5.3-codex")
    /// If not set, the harness uses its default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Reasoning effort level: "minimal", "low", "medium", "high", or
    /// "xhigh" (only supported by some models)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,

    /// Override configuration for supervisor agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor: Option<LlmRoleConfig>,

    /// Override configuration for worker agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<LlmRoleConfig>,
}

/// Per-role LLM overrides (supervisor or worker)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmRoleConfig {
    /// Override harness for this role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,

    /// Override model for this role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Override reasoning effort for this role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            harness: None,
            model: None,
            reasoning_effort: None,
            supervisor: None,
            worker: None,
        }
    }
}

impl LlmConfig {
    /// Resolve the harness for a given role.
    ///
    /// Three-step fallback chain (cas-fbac, mirroring [`model_for_role`]):
    /// 1. `[llm.<role>.harness]` — role-specific override (highest priority).
    /// 2. `[llm.harness]` — top-level fallback.
    /// 3. **Worker-only stock floor:** if `role == "worker"` AND both above
    ///    are unset, returns [`STOCK_WORKER_HARNESS`] (`"codex"`). Every
    ///    other role falls back to the literal `"claude"` — harness (unlike
    ///    model/effort) always resolves to a concrete CLI, never "use the
    ///    backend's own default", so there is no `None` case here.
    ///
    /// [`model_for_role`]: LlmConfig::model_for_role
    pub fn harness_for_role(&self, role: &str) -> &str {
        let role_override = match role {
            "supervisor" => self.supervisor.as_ref().and_then(|r| r.harness.as_deref()),
            "worker" => self.worker.as_ref().and_then(|r| r.harness.as_deref()),
            _ => None,
        };
        let resolved = role_override.or(self.harness.as_deref());
        match (resolved, role) {
            (Some(h), _) => h,
            (None, "worker") => STOCK_WORKER_HARNESS,
            (None, _) => "claude",
        }
    }

    /// Resolve the model for a given role.
    ///
    /// Three-step fallback chain (cas-05e3):
    /// 1. `[llm.<role>.model]` — role-specific override (highest priority).
    /// 2. `[llm.model]` — top-level fallback (preserves the existing-user
    ///    case where a single top-level model is meant to apply to all roles).
    /// 3. **Worker-only stock floor:** if `role == "worker"` AND both above
    ///    are unset, returns [`STOCK_WORKER_MODEL`]. Other roles still
    ///    return `None` at step 3 — the stock is deliberately scoped to the
    ///    worker lane.
    pub fn model_for_role(&self, role: &str) -> Option<&str> {
        let role_override = match role {
            "supervisor" => self.supervisor.as_ref().and_then(|r| r.model.as_deref()),
            "worker" => self.worker.as_ref().and_then(|r| r.model.as_deref()),
            _ => None,
        };
        let resolved = role_override.or(self.model.as_deref());
        match (resolved, role) {
            (Some(_), _) => resolved,
            (None, "worker") => Some(STOCK_WORKER_MODEL),
            (None, _) => None,
        }
    }

    /// Resolve the reasoning effort for a given role.
    ///
    /// Three-step fallback chain (cas-05e3):
    /// 1. `[llm.<role>.reasoning_effort]` — role-specific override.
    /// 2. `[llm.reasoning_effort]` — top-level fallback.
    /// 3. **Worker-only stock floor:** if `role == "worker"` AND both above
    ///    are unset, returns [`STOCK_WORKER_REASONING_EFFORT`]. Other roles
    ///    still return `None` at step 3.
    pub fn reasoning_effort_for_role(&self, role: &str) -> Option<&str> {
        let role_override = match role {
            "supervisor" => self
                .supervisor
                .as_ref()
                .and_then(|r| r.reasoning_effort.as_deref()),
            "worker" => self
                .worker
                .as_ref()
                .and_then(|r| r.reasoning_effort.as_deref()),
            _ => None,
        };
        let resolved = role_override.or(self.reasoning_effort.as_deref());
        match (resolved, role) {
            (Some(_), _) => resolved,
            (None, "worker") => Some(STOCK_WORKER_REASONING_EFFORT),
            (None, _) => None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_target() -> String {
    ".claude/rules/cas".to_string()
}

fn default_min_helpful() -> i32 {
    1
}

fn default_promotion_threshold() -> i32 {
    2
}

fn default_demotion_threshold() -> i32 {
    2
}

fn default_promotion_evidence() -> Vec<String> {
    vec!["helpful".to_string()]
}

/// Normalize and validate the configurable rule-promotion evidence sources.
pub(crate) fn parse_promotion_evidence(value: &str) -> Result<Vec<String>, String> {
    let sources: Vec<String> = value
        .split(',')
        .map(|source| source.trim().to_ascii_lowercase())
        .filter(|source| !source.is_empty())
        .collect();

    if sources.is_empty() {
        return Err("promotion_evidence must name at least one source".to_string());
    }

    if let Some(unknown) = sources
        .iter()
        .find(|source| !matches!(source.as_str(), "helpful" | "retrieval"))
    {
        return Err(format!(
            "unknown promotion evidence source '{unknown}'; expected helpful or retrieval"
        ));
    }

    Ok(sources)
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            target: ".claude/rules/cas".to_string(),
            min_helpful: 1,
            promotion_threshold: default_promotion_threshold(),
            demotion_threshold: default_demotion_threshold(),
            promotion_evidence: default_promotion_evidence(),
        }
    }
}

/// `[memory]` — gates auto-extraction behavior for the session-learn skill
/// (cas-39f5, EPIC cas-ebea). Default-off so the v1 rollout opts in per-user
/// rather than spending a Haiku call (~$0.001, ~1–3 s) on every `Stop` hook
/// invocation without explicit consent.
///
/// ```toml
/// [memory]
/// session_learn_auto = true   # opt in to auto-extraction on Stop
/// ```
///
/// Manual invocation of the `session-learn` skill works regardless of this
/// flag — the flag only gates the auto-trigger from the `Stop` hook
/// handler.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryConfig {
    /// When `true`, the `Stop` hook runs the `session-learn` classifier
    /// against the session transcript and writes draft memories through
    /// the existing `mcp__cas__memory remember` overlap-detection gate.
    /// Defaults to `false` — v1 ships as opt-in until the false-positive
    /// rate is measured against real session traffic.
    #[serde(default)]
    pub session_learn_auto: bool,

    /// Curated-memory decay and access-promotion policy.
    #[serde(default)]
    pub decay: MemoryDecayConfig,
}

/// Memory lifecycle policy used by background decay and access paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDecayConfig {
    /// Importance at or above which stability decay cannot demote an entry
    /// below the working tier. Defaults to the measured retrieval knee, 0.9.
    #[serde(default = "default_curated_importance_floor")]
    pub curated_importance_floor: f32,

    /// Promote cold/archive tier entries to working when accessed.
    #[serde(default = "default_promote_on_access")]
    pub promote_on_access: bool,
}

fn default_curated_importance_floor() -> f32 {
    0.9
}

fn default_promote_on_access() -> bool {
    true
}

impl Default for MemoryDecayConfig {
    fn default() -> Self {
        Self {
            curated_importance_floor: default_curated_importance_floor(),
            promote_on_access: default_promote_on_access(),
        }
    }
}

/// `[integrations]` — gates Phase-3 doctor-and-banner behavior for the
/// vercel/neon/github auto-integration family (EPIC cas-b65f). Default-off
/// across the board so an absent or empty section preserves the prior UX.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationsConfig {
    /// When true, the SessionStart hook surfaces a low-severity banner if
    /// any platform reports stale IDs. Default `false` — the codemap
    /// freshness banner already occupies the SessionStart slot, and
    /// stacking another banner there erodes its signal. Opt-in only.
    #[serde(default)]
    pub session_start_warn: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-0bf4: defaults for the two resource-contention knobs must stay
    /// stable — they ship as the on-by-default mitigation for factory
    /// worker wedges. A careless refactor that flipped `nice_cargo` to
    /// `false` or `cargo_build_jobs` to `""` would silently disable the
    /// cap on every new install.
    #[test]
    fn factory_config_defaults_cargo_contention_knobs() {
        let fc = FactoryConfig::default();
        assert_eq!(
            fc.cargo_build_jobs, "auto",
            "cargo_build_jobs default must be 'auto' so cas-pty computes the cap"
        );
        assert!(
            fc.nice_cargo,
            "nice_cargo default must be true so workers run niced relative to supervisor"
        );
    }

    /// Round-trip: a persisted config with no factory section deserializes
    /// to the same defaults as `Default::default()`. Guards against a
    /// future serde attribute mismatch that would change read-vs-default
    /// divergence (the classic "silent config drift" bug).
    #[test]
    fn factory_config_roundtrips_through_toml_empty_section() {
        let toml_str = "[factory]\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert_eq!(
            fc.cargo_build_jobs,
            FactoryConfig::default().cargo_build_jobs
        );
        assert_eq!(fc.nice_cargo, FactoryConfig::default().nice_cargo);
        assert_eq!(
            fc.stall_threshold_secs,
            FactoryConfig::default().stall_threshold_secs
        );
        assert_eq!(
            fc.epic_base_branch,
            FactoryConfig::default().epic_base_branch
        );
        assert_eq!(fc.epic_base_branch, None);
        assert_eq!(fc.target_cache_high_watermark_percent, 85);
        assert_eq!(fc.target_cache_low_watermark_percent, 75);
        assert_eq!(fc.target_cache_min_idle_secs, 3600);
        assert_eq!(fc.target_cache_retention_count, 1);
    }

    #[test]
    fn factory_target_cache_policy_is_configurable() {
        let toml_str = "[factory]\ntarget_cache_high_watermark_percent = 90\ntarget_cache_low_watermark_percent = 70\ntarget_cache_min_idle_secs = 7200\ntarget_cache_retention_count = 2\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert_eq!(fc.target_cache_high_watermark_percent, 90);
        assert_eq!(fc.target_cache_low_watermark_percent, 70);
        assert_eq!(fc.target_cache_min_idle_secs, 7200);
        assert_eq!(fc.target_cache_retention_count, 2);
    }

    /// cas-9829: `stall_threshold_secs` defaults to a few minutes
    /// (`cas_factory::DEFAULT_STALL_THRESHOLD_SECS`) and is overridable via
    /// `.cas/config.toml`. A regression that silently ignored the TOML
    /// value would leave every operator stuck with the default, unable to
    /// tune stall detection for their workload.
    #[test]
    fn factory_config_stall_threshold_secs_configurable() {
        let toml_str = "[factory]\nstall_threshold_secs = 120\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert_eq!(fc.stall_threshold_secs, 120);
        assert_eq!(
            FactoryConfig::default().stall_threshold_secs,
            cas_factory::DEFAULT_STALL_THRESHOLD_SECS
        );
    }

    #[test]
    fn factory_config_delivery_stalled_thresholds_are_configurable() {
        let toml_str =
            "[factory]\ndelivery_stalled_priority_secs = 45\ndelivery_stalled_normal_secs = 120\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert_eq!(fc.delivery_stalled_priority_secs, 45);
        assert_eq!(fc.delivery_stalled_normal_secs, 120);
        assert_eq!(
            FactoryConfig::default().delivery_stalled_priority_secs,
            10 * 60
        );
        assert_eq!(
            FactoryConfig::default().delivery_stalled_normal_secs,
            30 * 60
        );
    }

    /// cas-7199 / cas-a487: `[factory] strict_cli` defaults to `false`
    /// (fall back to Claude with a warning) and is overridable to `true`
    /// (bail instead of falling back on a missing Codex install/login).
    #[test]
    fn factory_config_strict_cli_configurable() {
        assert!(!FactoryConfig::default().strict_cli);

        let toml_str = "[factory]\nstrict_cli = true\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert!(fc.strict_cli);
    }

    /// cas-b082: `epic_base_branch` defaults to `None` (repo default branch)
    /// and is overridable via `.cas/config.toml` — staging-first shops set
    /// `epic_base_branch = "staging"` so epic auto-branch creation and
    /// worker-spawn base resolution stop needing a manual
    /// `git branch -f epic/... origin/staging` correction after the fact.
    #[test]
    fn factory_config_epic_base_branch_configurable() {
        let toml_str = "[factory]\nepic_base_branch = \"staging\"\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert_eq!(fc.epic_base_branch.as_deref(), Some("staging"));
        assert_eq!(FactoryConfig::default().epic_base_branch, None);
    }

    #[test]
    fn staging_config_defaults_to_one_gib_threshold() {
        let sc = StagingConfig::default();
        assert_eq!(
            sc.tmpfs_warning_threshold_bytes,
            DEFAULT_TMPFS_WARNING_THRESHOLD_BYTES
        );
        assert_eq!(sc.staging_dir, None);
        assert_eq!(sc.scratch_root, None);
    }

    #[test]
    fn staging_config_roundtrips_staging_dir_and_threshold() {
        let toml_str = "[staging]\nstaging_dir = \"/mnt/durable/cas-staging\"\nscratch_root = \"/mnt/durable/cas-scratch\"\ntmpfs_warning_threshold_bytes = 2048\n";
        let parsed: std::collections::HashMap<String, StagingConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let sc = parsed.get("staging").expect("section present");
        assert_eq!(sc.staging_dir.as_deref(), Some("/mnt/durable/cas-staging"));
        assert_eq!(sc.scratch_root.as_deref(), Some("/mnt/durable/cas-scratch"));
        assert_eq!(sc.tmpfs_warning_threshold_bytes, 2048);
    }

    #[test]
    fn staging_config_accepts_large_artifact_dir_alias() {
        let toml_str = "[staging]\nlarge_artifact_dir = \"/mnt/datacube/staging\"\n";
        let parsed: std::collections::HashMap<String, StagingConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let sc = parsed.get("staging").expect("section present");
        assert_eq!(sc.staging_dir.as_deref(), Some("/mnt/datacube/staging"));
        assert_eq!(
            sc.tmpfs_warning_threshold_bytes,
            DEFAULT_TMPFS_WARNING_THRESHOLD_BYTES
        );
    }

    /// `Config::configured_epic_base_branch` is the shared read path used by
    /// both epic-branch creation and worker-spawn base resolution — assert
    /// it actually reads the TOML-persisted value end to end (not just that
    /// `FactoryConfig` deserializes correctly in isolation).
    #[test]
    fn configured_epic_base_branch_reads_persisted_toml_value() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        std::fs::write(
            cas_dir.join("config.toml"),
            "[factory]\nepic_base_branch = \"staging\"\n",
        )
        .unwrap();

        assert_eq!(
            crate::config::Config::configured_epic_base_branch(temp.path()),
            Some("staging".to_string())
        );
    }

    /// No `.cas/config.toml` at all (fresh repo) must resolve to `None`, not
    /// error — callers fall back to `detect_default_branch()`.
    #[test]
    fn configured_epic_base_branch_none_when_config_absent() {
        let temp = tempfile::TempDir::new().unwrap();
        assert_eq!(
            crate::config::Config::configured_epic_base_branch(temp.path()),
            None
        );
    }

    /// cas-39f5: the `[memory]` section must default `session_learn_auto`
    /// to `false`. A regression that flips the default to `true` would
    /// silently start spending Haiku tokens on every `Stop` hook in every
    /// install — exactly the failure mode the kill-switch was designed
    /// to prevent. Round-trip an empty `[memory]` section to confirm the
    /// serde default, and parse the explicit-true form to confirm the
    /// opt-in path still works.
    #[test]
    fn memory_config_defaults_session_learn_auto_off() {
        let default_cfg = MemoryConfig::default();
        assert!(
            !default_cfg.session_learn_auto,
            "MemoryConfig::default().session_learn_auto must be false — the \
             v1 rollout is opt-in. Flipping this default is the wrong way to \
             enable session-learn; users must set the flag in .cas/config.toml."
        );
    }

    #[test]
    fn memory_config_roundtrips_through_toml_empty_section() {
        let toml_str = "[memory]\n";
        let parsed: std::collections::HashMap<String, MemoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let mc = parsed.get("memory").expect("section present");
        assert!(
            !mc.session_learn_auto,
            "empty [memory] section must deserialize with session_learn_auto = false"
        );
    }

    #[test]
    fn memory_config_roundtrips_explicit_opt_in() {
        let toml_str = "[memory]\nsession_learn_auto = true\n";
        let parsed: std::collections::HashMap<String, MemoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let mc = parsed.get("memory").expect("section present");
        assert!(
            mc.session_learn_auto,
            "explicit session_learn_auto = true must deserialize as opt-in"
        );
    }

    // ── LlmConfig::reasoning_effort_for_role ─────────────────────────────────
    // cas-9393: critical-path method feeds supervisor_effort and worker_effort
    // through the factory spawn pipeline — must have full coverage.

    /// No config at all → supervisor returns None, worker returns the stock
    /// default. cas-05e3 split this from a single combined assertion: workers
    /// now have a stock-fallback floor so new installs spawn with a sensible
    /// model + effort without any `.cas/config.toml` editing.
    #[test]
    fn reasoning_effort_for_role_no_config_returns_none_for_supervisor() {
        let llm = LlmConfig::default();
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            None,
            "supervisor must NOT receive the worker stock-default — \
             regressions that bleed it across roles must fail this test"
        );
    }

    /// Top-level `reasoning_effort` is the fallback when no per-role override
    /// is present. Both supervisor and worker should see it.
    #[test]
    fn reasoning_effort_for_role_top_level_fallback() {
        let llm = LlmConfig {
            reasoning_effort: Some("medium".to_string()),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("medium"),
            "supervisor should fall back to top-level reasoning_effort"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("medium"),
            "worker should fall back to top-level reasoning_effort"
        );
    }

    /// A supervisor-specific override shadows the top-level value for the
    /// supervisor role, while the worker still sees the top-level value.
    #[test]
    fn reasoning_effort_for_role_supervisor_override() {
        let llm = LlmConfig {
            reasoning_effort: Some("high".to_string()),
            supervisor: Some(LlmRoleConfig {
                reasoning_effort: Some("low".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("low"),
            "supervisor override must shadow top-level value"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker must still see top-level when no worker override is set"
        );
    }

    /// A worker-specific override shadows the top-level value for the worker
    /// role, while the supervisor still sees the top-level value.
    #[test]
    fn reasoning_effort_for_role_worker_override() {
        let llm = LlmConfig {
            reasoning_effort: Some("medium".to_string()),
            worker: Some(LlmRoleConfig {
                reasoning_effort: Some("high".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker override must shadow top-level value"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("medium"),
            "supervisor must still see top-level when no supervisor override is set"
        );
    }

    /// Per-role overrides are independent: supervisor and worker can each have
    /// their own distinct effort level without interfering with each other.
    #[test]
    fn reasoning_effort_for_role_independent_overrides() {
        let llm = LlmConfig {
            reasoning_effort: None,
            supervisor: Some(LlmRoleConfig {
                reasoning_effort: Some("low".to_string()),
                ..LlmRoleConfig::default()
            }),
            worker: Some(LlmRoleConfig {
                reasoning_effort: Some("high".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("low"),
            "supervisor-only override must not bleed into worker"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker-only override must not bleed into supervisor"
        );
    }

    /// An unknown / unrecognised role is treated as having no per-role override.
    /// It falls back to the top-level value, or None if the top-level is unset.
    #[test]
    fn reasoning_effort_for_role_unknown_role_falls_back_to_top_level() {
        let llm_with_top = LlmConfig {
            reasoning_effort: Some("medium".to_string()),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm_with_top.reasoning_effort_for_role("orchestrator"),
            Some("medium"),
            "unknown role must fall back to top-level reasoning_effort"
        );

        let llm_no_top = LlmConfig::default();
        assert_eq!(
            llm_no_top.reasoning_effort_for_role("orchestrator"),
            None,
            "unknown role with no top-level must return None"
        );
    }

    /// A per-role block may exist (e.g. to override harness or model) without
    /// setting `reasoning_effort`. In that case the top-level value must still
    /// be returned — the `and_then` short-circuit must not swallow the fallback.
    #[test]
    fn reasoning_effort_for_role_partial_override_falls_back_to_top_level() {
        let llm = LlmConfig {
            reasoning_effort: Some("high".to_string()),
            supervisor: Some(LlmRoleConfig {
                harness: Some("codex".to_string()),
                reasoning_effort: None, // effort NOT set in the role block
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("high"),
            "partial role override (harness set, effort absent) must fall back to top-level"
        );
    }

    /// Round-trip via TOML deserialization: verifies that the serde attributes
    /// on `LlmRoleConfig::reasoning_effort` are correct and that the field is
    /// not silently dropped during deserialization.
    #[test]
    fn reasoning_effort_for_role_toml_roundtrip() {
        let toml_str = r#"
[llm]
reasoning_effort = "medium"

[llm.supervisor]
reasoning_effort = "low"

[llm.worker]
reasoning_effort = "high"
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            llm: LlmConfig,
        }
        let parsed: Wrapper = toml::from_str(toml_str).expect("valid toml");
        let llm = parsed.llm;
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("low"),
            "supervisor reasoning_effort must survive TOML deserialization"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker reasoning_effort must survive TOML deserialization"
        );
        // Top-level fallback still works after round-trip
        assert_eq!(
            llm.reasoning_effort_for_role("orchestrator"),
            Some("medium"),
            "top-level reasoning_effort must survive TOML deserialization"
        );
    }

    /// Round-trip via TOML deserialization for the new `Option<String>`
    /// top-level `harness` field (cas-fbac). Explicit values at every level
    /// (top-level + both role overrides) must survive the String -> Option
    /// type change unchanged.
    #[test]
    fn harness_for_role_toml_roundtrip() {
        let toml_str = r#"
[llm]
harness = "codex"

[llm.supervisor]
harness = "claude"

[llm.worker]
harness = "codex"
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            llm: LlmConfig,
        }
        let parsed: Wrapper = toml::from_str(toml_str).expect("valid toml");
        let llm = parsed.llm;
        assert_eq!(
            llm.harness_for_role("supervisor"),
            "claude",
            "supervisor harness override must survive TOML deserialization"
        );
        assert_eq!(
            llm.harness_for_role("worker"),
            "codex",
            "worker harness override must survive TOML deserialization"
        );
        // Top-level fallback still works after round-trip
        assert_eq!(
            llm.harness_for_role("orchestrator"),
            "codex",
            "top-level harness must survive TOML deserialization"
        );
    }

    /// cas-fbac guardrail: a genuinely empty `[llm]` section round-trips
    /// (serialize -> deserialize) to worker=codex / supervisor=claude —
    /// pinning both sides of the stock-floor split in a single test, via a
    /// real serialize+deserialize cycle rather than just `LlmConfig::default()`.
    ///
    /// Also confirms the `skip_serializing_if = "Option::is_none"` attribute
    /// on `harness` still holds: an unset harness must NOT be written to disk
    /// as a literal `harness = "..."` line (that would defeat the whole
    /// point of distinguishing "unset" from "explicitly claude"), matching
    /// the `model`/`reasoning_effort` precedent this field now mirrors.
    #[test]
    fn harness_empty_config_roundtrips_and_resolves_worker_codex_supervisor_claude() {
        let llm = LlmConfig::default();
        let serialized = toml::to_string(&llm).expect("serialize default LlmConfig");
        assert!(
            !serialized.contains("harness"),
            "unset top-level harness must be omitted from serialized TOML \
             (skip_serializing_if), not written as an explicit value; got:\n{serialized}"
        );

        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            llm: LlmConfig,
        }
        let wrapped = toml::to_string(&Wrapper { llm }).expect("serialize wrapped LlmConfig");
        let parsed: Wrapper = toml::from_str(&wrapped).expect("deserialize round-tripped TOML");
        let llm = parsed.llm;

        assert_eq!(
            llm.harness_for_role("worker"),
            "codex",
            "round-tripped empty config: worker must resolve to the Codex stock floor"
        );
        assert_eq!(
            llm.harness_for_role("supervisor"),
            "claude",
            "round-tripped empty config: supervisor must stay on the literal claude default"
        );
    }

    // ── cas-05e3 / cas-fbac: stock worker default ───────────────────────────
    // The worker role gets a harness + model + reasoning_effort floor when
    // nothing is configured. New installs and upgraders without an
    // `[llm.worker]` block pick this up automatically; explicit config at
    // either level still wins. Supervisor MUST stay on `None` (model/effort)
    // / literal `"claude"` (harness) so future regressions can't silently
    // switch the supervisor lane onto the worker stock.

    /// Empty config → worker resolves to the stock harness + model + effort.
    /// The whole point of cas-05e3/cas-fbac is that brand-new installs work
    /// without editing `.cas/config.toml`.
    #[test]
    fn worker_stock_default_kicks_in_when_nothing_configured() {
        let llm = LlmConfig::default();
        assert_eq!(
            llm.harness_for_role("worker"),
            STOCK_WORKER_HARNESS,
            "empty config must resolve worker harness to the stock default"
        );
        assert_eq!(
            llm.model_for_role("worker"),
            Some(STOCK_WORKER_MODEL),
            "empty config must resolve worker model to the stock default"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some(STOCK_WORKER_REASONING_EFFORT),
            "empty config must resolve worker reasoning_effort to the stock default"
        );
        // Sanity-check the constant values match the shipped routing policy.
        assert_eq!(STOCK_WORKER_HARNESS, "codex");
        assert_eq!(STOCK_WORKER_MODEL, "gpt-5.6-luna");
        assert_eq!(STOCK_WORKER_REASONING_EFFORT, "xhigh");
    }

    /// Existing-user preservation: a top-level `[llm] model = "X"` (no
    /// `[llm.worker]` block) still wins over the stock. This is the
    /// non-negotiable back-compat hinge — users who set top-level model
    /// expecting all roles to inherit must keep getting that behavior.
    /// `reasoning_effort` is None at top level so it falls through to the
    /// stock floor.
    #[test]
    fn worker_top_level_model_wins_over_stock_default() {
        let llm = LlmConfig {
            model: Some("claude-opus-5".to_string()),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.model_for_role("worker"),
            Some("claude-opus-5"),
            "top-level llm.model must still flow through to workers — \
             stock fallback only fires when both role and top-level are None"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some(STOCK_WORKER_REASONING_EFFORT),
            "top-level model set but top-level effort unset → worker effort \
             still gets the stock floor"
        );
    }

    /// Full top-level inherit: both model and effort set top-level, no role
    /// block. Worker sees both, stock never fires.
    #[test]
    fn worker_full_top_level_inherit_suppresses_stock() {
        let llm = LlmConfig {
            model: Some("custom-model".to_string()),
            reasoning_effort: Some("medium".to_string()),
            ..LlmConfig::default()
        };
        assert_eq!(llm.model_for_role("worker"), Some("custom-model"));
        assert_eq!(llm.reasoning_effort_for_role("worker"), Some("medium"));
    }

    /// Partial worker override (model only): stock still fires for the OTHER
    /// field. Setting `[llm.worker] model = "Y"` does not opt out of the
    /// stock effort floor.
    #[test]
    fn worker_partial_override_model_only_keeps_stock_effort() {
        let llm = LlmConfig {
            worker: Some(LlmRoleConfig {
                model: Some("custom-y".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(llm.model_for_role("worker"), Some("custom-y"));
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some(STOCK_WORKER_REASONING_EFFORT),
            "worker override on model must not suppress the stock effort floor"
        );
    }

    /// Partial worker override (effort only): stock model floor still fires.
    #[test]
    fn worker_partial_override_effort_only_keeps_stock_model() {
        let llm = LlmConfig {
            worker: Some(LlmRoleConfig {
                reasoning_effort: Some("low".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.model_for_role("worker"),
            Some(STOCK_WORKER_MODEL),
            "worker override on effort must not suppress the stock model floor"
        );
        assert_eq!(llm.reasoning_effort_for_role("worker"), Some("low"));
    }

    /// Supervisor stock-leak guard. `model_for_role("supervisor")` and
    /// `reasoning_effort_for_role("supervisor")` MUST return None when
    /// nothing is configured, and `harness_for_role("supervisor")` MUST stay
    /// on the literal `"claude"` default. The stock is a worker-only concept;
    /// a future change that accidentally applies it to supervisor would break
    /// the supervisor's default-Opus lane (`teams.rs:402,526`) or silently
    /// spawn the supervisor under Codex (cas-fbac).
    #[test]
    fn supervisor_does_not_receive_worker_stock_default() {
        let llm = LlmConfig::default();
        assert_eq!(
            llm.harness_for_role("supervisor"),
            "claude",
            "supervisor harness must stay \"claude\" on empty config — stock is worker-only"
        );
        assert_eq!(
            llm.model_for_role("supervisor"),
            None,
            "supervisor model must stay None on empty config — stock is worker-only"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            None,
            "supervisor effort must stay None on empty config — stock is worker-only"
        );
    }

    /// cas-499c (operator ruling): the tree-sitter symbol index is ON by default. It had never
    /// run on any install because this defaulted to false, so `code_search` was permanently a
    /// stub. Flipping it back would silently re-disable the feature for every new install.
    #[test]
    fn code_config_is_enabled_by_default() {
        assert!(CodeConfig::default().enabled);
    }

    /// A persisted config that omits `enabled` (or omits `[code]` entirely) must also resolve
    /// to on — a `#[serde(default)]` on the bool would quietly resolve to false instead.
    #[test]
    fn code_config_enabled_defaults_on_when_absent_from_toml() {
        let parsed: std::collections::HashMap<String, CodeConfig> =
            toml::from_str("[code]\n").expect("valid toml");
        assert!(
            parsed.get("code").expect("section present").enabled,
            "an empty [code] section must resolve enabled=true"
        );

        let with_other_keys: std::collections::HashMap<String, CodeConfig> =
            toml::from_str("[code]\ndebounce_ms = 750\n").expect("valid toml");
        assert!(
            with_other_keys
                .get("code")
                .expect("section present")
                .enabled
        );

        // And an explicit opt-out is still honoured.
        let opted_out: std::collections::HashMap<String, CodeConfig> =
            toml::from_str("[code]\nenabled = false\n").expect("valid toml");
        assert!(!opted_out.get("code").expect("section present").enabled);
    }
}
