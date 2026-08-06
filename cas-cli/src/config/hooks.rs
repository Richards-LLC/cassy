use serde::{Deserialize, Serialize};

/// Per-hook configuration for SessionStart
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_session_start_timeout")]
    pub timeout: u32,

    /// Token budget for context injection (0 = unlimited)
    #[serde(default = "default_token_budget")]
    pub token_budget: usize,

    /// Minimal start mode - only inject blocked tasks and pinned memories
    #[serde(default)]
    pub minimal: bool,
}

fn default_session_start_timeout() -> u32 {
    5000
}

impl Default for SessionStartHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_session_start_timeout(),
            token_budget: default_token_budget(),
            minimal: false,
        }
    }
}

/// Skip configuration for PostToolUse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolUseSkipConfig {
    /// Commands to skip capturing (e.g., "ls", "cd", "pwd")
    #[serde(default = "default_skip_commands")]
    pub commands: Vec<String>,

    /// Command prefixes to skip (e.g., "cas ", "cas-")
    #[serde(default = "default_skip_prefixes")]
    pub prefixes: Vec<String>,

    /// Read-only git commands to skip
    #[serde(default = "default_skip_git_readonly")]
    pub git_readonly: Vec<String>,

    /// File extensions to skip for Read observations
    #[serde(default = "default_skip_read_extensions")]
    pub read_extensions: Vec<String>,
}

fn default_skip_commands() -> Vec<String> {
    vec![
        "ls".into(),
        "cd".into(),
        "pwd".into(),
        "echo".into(),
        "cat".into(),
        "head".into(),
        "tail".into(),
        "less".into(),
        "more".into(),
    ]
}

fn default_skip_prefixes() -> Vec<String> {
    vec!["cas ".into(), "cas-".into()]
}

fn default_skip_git_readonly() -> Vec<String> {
    vec![
        "git status".into(),
        "git log".into(),
        "git diff".into(),
        "git show".into(),
        "git branch".into(),
    ]
}

fn default_skip_read_extensions() -> Vec<String> {
    vec![
        ".md".into(),
        ".txt".into(),
        ".json".into(),
        ".yaml".into(),
        ".yml".into(),
        ".toml".into(),
    ]
}

impl Default for PostToolUseSkipConfig {
    fn default() -> Self {
        Self {
            commands: default_skip_commands(),
            prefixes: default_skip_prefixes(),
            git_readonly: default_skip_git_readonly(),
            read_extensions: default_skip_read_extensions(),
        }
    }
}

/// Per-hook configuration for PostToolUse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolUseHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_post_tool_use_timeout")]
    pub timeout: u32,

    /// Tool matcher for Claude Code (pipe-separated)
    #[serde(default = "default_post_tool_use_matcher")]
    pub matcher: Vec<String>,

    /// Tools to capture observations from
    #[serde(default = "default_capture_tools")]
    pub capture_tools: Vec<String>,

    /// Maximum observations to buffer before synthesis
    #[serde(default = "default_max_observations")]
    pub max_observations: usize,

    /// Number of recent observations to keep
    #[serde(default = "default_keep_recent")]
    pub keep_recent: usize,

    /// Skip configuration
    #[serde(default)]
    pub skip: PostToolUseSkipConfig,
}

fn default_post_tool_use_timeout() -> u32 {
    3000
}

fn default_post_tool_use_matcher() -> Vec<String> {
    vec!["Write".into(), "Edit".into(), "Bash".into()]
}

fn default_max_observations() -> usize {
    50
}

fn default_keep_recent() -> usize {
    5
}

impl Default for PostToolUseHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_post_tool_use_timeout(),
            matcher: default_post_tool_use_matcher(),
            capture_tools: default_capture_tools(),
            max_observations: default_max_observations(),
            keep_recent: default_keep_recent(),
            skip: PostToolUseSkipConfig::default(),
        }
    }
}

/// Protection configuration for PreToolUse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolUseProtectionConfig {
    /// Whether file protection is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Protected file patterns (e.g., ".env", ".env.*")
    #[serde(default = "default_protected_files")]
    pub files: Vec<String>,

    /// Credential file patterns
    #[serde(default = "default_credential_patterns")]
    pub patterns: Vec<String>,
}

fn default_protected_files() -> Vec<String> {
    vec![".env".into()]
}

fn default_credential_patterns() -> Vec<String> {
    vec![
        "credentials.json".into(),
        "secrets.yaml".into(),
        "secrets.json".into(),
        ".pem".into(),
        ".key".into(),
        "id_rsa".into(),
        "id_ed25519".into(),
    ]
}

impl Default for PreToolUseProtectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            files: default_protected_files(),
            patterns: default_credential_patterns(),
        }
    }
}

/// Per-hook configuration for PreToolUse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolUseHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_pre_tool_use_timeout")]
    pub timeout: u32,

    /// Tool matcher for Claude Code (pipe-separated)
    #[serde(default = "default_pre_tool_use_matcher")]
    pub matcher: Vec<String>,

    /// File protection configuration
    #[serde(default)]
    pub protection: PreToolUseProtectionConfig,
}

fn default_pre_tool_use_timeout() -> u32 {
    2000
}

fn default_pre_tool_use_matcher() -> Vec<String> {
    vec![
        "Read".into(),
        "Glob".into(),
        "Grep".into(),
        "Write".into(),
        "Edit".into(),
        "Bash".into(),
        "WebFetch".into(),
        "WebSearch".into(),
        "Task".into(),            // For verification jail unjailing
        "SendMessage".into(),     // Blocked in factory mode → use coordination message
        "AskUserQuestion".into(), // Blocked in factory mode → ask in plain text / coordination
        "Skill".into(), // cas-bcfb: cas-code-review ownership gate (GH #125) — harness-native path
        "Workflow".into(), // cas-bcfb: same gate for direct cas-code-review workflow invocation
    ]
}

impl Default for PreToolUseHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_pre_tool_use_timeout(),
            matcher: default_pre_tool_use_matcher(),
            protection: PreToolUseProtectionConfig::default(),
        }
    }
}

/// Per-hook configuration for Stop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_stop_timeout")]
    pub timeout: u32,

    /// Block exit when agent has open tasks
    #[serde(default = "default_true")]
    pub block_on_open_tasks: bool,

    /// Generate AI summary at session end
    #[serde(default)]
    pub generate_summary: bool,

    /// Enable learning review at session end (blocks stop if unreviewed learnings exceed threshold)
    #[serde(default)]
    pub learning_review_enabled: bool,

    /// Number of unreviewed learnings required to trigger review (default: 5)
    #[serde(default = "default_learning_review_threshold")]
    pub learning_review_threshold: usize,

    /// Enable rule review at session end (blocks stop if draft rules exceed threshold)
    #[serde(default)]
    pub rule_review_enabled: bool,

    /// Number of draft rules required to trigger review (default: 5)
    #[serde(default = "default_rule_review_threshold")]
    pub rule_review_threshold: usize,

    /// Enable duplicate detection at session end
    #[serde(default)]
    pub duplicate_detection_enabled: bool,

    /// Minimum entries before triggering duplicate detection (default: 20)
    #[serde(default = "default_duplicate_detection_threshold")]
    pub duplicate_detection_threshold: usize,

    /// Enable signals analysis at session end (analyzes friction patterns)
    #[serde(default)]
    pub signals_analysis_enabled: bool,

    /// Number of friction events required to trigger analysis (default: 10)
    #[serde(default = "default_signals_analysis_threshold")]
    pub signals_analysis_threshold: usize,
}

fn default_stop_timeout() -> u32 {
    10000
}

fn default_learning_review_threshold() -> usize {
    5
}

fn default_rule_review_threshold() -> usize {
    5
}

fn default_duplicate_detection_threshold() -> usize {
    20
}

fn default_signals_analysis_threshold() -> usize {
    10
}

impl Default for StopHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_stop_timeout(),
            block_on_open_tasks: true,
            generate_summary: false,
            learning_review_enabled: false,
            learning_review_threshold: default_learning_review_threshold(),
            rule_review_enabled: false,
            rule_review_threshold: default_rule_review_threshold(),
            duplicate_detection_enabled: false,
            duplicate_detection_threshold: default_duplicate_detection_threshold(),
            signals_analysis_enabled: false,
            signals_analysis_threshold: default_signals_analysis_threshold(),
        }
    }
}

/// Per-hook configuration for UserPromptSubmit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPromptSubmitHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_user_prompt_submit_timeout")]
    pub timeout: u32,
}

fn default_user_prompt_submit_timeout() -> u32 {
    3000
}

impl Default for UserPromptSubmitHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_user_prompt_submit_timeout(),
        }
    }
}

/// Per-hook configuration for PermissionRequest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequestHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_permission_request_timeout")]
    pub timeout: u32,
}

fn default_permission_request_timeout() -> u32 {
    2000
}

impl Default for PermissionRequestHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_permission_request_timeout(),
        }
    }
}

/// Per-hook configuration for PreCompact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreCompactHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_pre_compact_timeout")]
    pub timeout: u32,
}

fn default_pre_compact_timeout() -> u32 {
    3000
}

impl Default for PreCompactHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_pre_compact_timeout(),
        }
    }
}

/// Per-hook configuration for Notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationHookConfig {
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout in milliseconds
    #[serde(default = "default_notification_timeout")]
    pub timeout: u32,

    /// Event matcher
    #[serde(default = "default_notification_matcher")]
    pub matcher: Vec<String>,
}

fn default_notification_timeout() -> u32 {
    1000
}

fn default_notification_matcher() -> Vec<String> {
    vec![
        "permission_prompt".into(),
        "idle_prompt".into(),
        "auth_success".into(),
    ]
}

impl Default for NotificationHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: default_notification_timeout(),
            matcher: default_notification_matcher(),
        }
    }
}

/// Hook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// MessageDisplay guard — sanitize Ink-crash trigger shapes and redact
    /// secrets from assistant text before it reaches the terminal renderer
    /// (CC 2.1.152+ MessageDisplay hook, cas-b39a).
    ///
    /// **Default: false** — the handler is a pure passthrough when this flag
    /// is absent or false, preserving existing behavior byte-for-byte.
    /// Set to `true` in `[hooks]` of `.cas/config.toml` to opt in.
    #[serde(default)]
    pub message_display_guard: bool,
    /// Whether capture is enabled (legacy, use post_tool_use.enabled)
    #[serde(default = "default_true")]
    pub capture_enabled: bool,

    /// Tools to capture (legacy, use post_tool_use.capture_tools)
    #[serde(default = "default_capture_tools")]
    pub capture_tools: Vec<String>,

    /// Whether to inject context at session start (legacy, use session_start.enabled)
    #[serde(default = "default_true")]
    pub inject_context: bool,

    /// Maximum entries to include in context
    #[serde(default = "default_context_limit")]
    pub context_limit: usize,

    /// Whether to generate AI summaries at session end (legacy, use stop.generate_summary)
    #[serde(default)]
    pub generate_summaries: bool,

    /// Token budget for context injection (legacy, use session_start.token_budget)
    #[serde(default = "default_token_budget")]
    pub token_budget: usize,

    /// Use AI to prioritize context items (requires claude CLI)
    #[serde(default)]
    pub ai_context: bool,

    /// Model to use for AI context prioritization
    #[serde(default = "default_ai_model")]
    pub ai_model: String,

    /// Fall back to non-AI context if AI prioritization fails
    #[serde(default = "default_ai_fallback")]
    pub ai_fallback: bool,

    /// Plan mode specific configuration
    #[serde(default)]
    pub plan_mode: PlanModeConfig,

    /// Minimal start mode (legacy, use session_start.minimal)
    #[serde(default)]
    pub minimal_start: bool,

    // === Per-hook configurations ===
    /// SessionStart hook configuration
    #[serde(default)]
    pub session_start: SessionStartHookConfig,

    /// PostToolUse hook configuration
    #[serde(default)]
    pub post_tool_use: PostToolUseHookConfig,

    /// PreToolUse hook configuration
    #[serde(default)]
    pub pre_tool_use: PreToolUseHookConfig,

    /// Stop hook configuration
    #[serde(default)]
    pub stop: StopHookConfig,

    /// UserPromptSubmit hook configuration
    #[serde(default)]
    pub user_prompt_submit: UserPromptSubmitHookConfig,

    /// PermissionRequest hook configuration
    #[serde(default)]
    pub permission_request: PermissionRequestHookConfig,

    /// PreCompact hook configuration
    #[serde(default)]
    pub pre_compact: PreCompactHookConfig,

    /// Notification hook configuration
    #[serde(default)]
    pub notification: NotificationHookConfig,
}

/// Plan mode specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeConfig {
    /// Whether to use plan-aware context in plan mode
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Token budget for plan mode (typically higher than execution)
    #[serde(default = "default_plan_token_budget")]
    pub token_budget: usize,

    /// Maximum tasks to show in plan context
    #[serde(default = "default_plan_task_limit")]
    pub task_limit: usize,

    /// Include dependency trees in plan context
    #[serde(default = "default_true")]
    pub show_dependencies: bool,

    /// Include closed tasks for reference
    #[serde(default)]
    pub include_closed: bool,

    /// Search for related memories semantically
    #[serde(default = "default_true")]
    pub semantic_search: bool,
}

fn default_plan_token_budget() -> usize {
    8000 // Higher budget for planning
}

fn default_plan_task_limit() -> usize {
    15
}

impl Default for PlanModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            token_budget: default_plan_token_budget(),
            task_limit: default_plan_task_limit(),
            show_dependencies: true,
            include_closed: false,
            semantic_search: true,
        }
    }
}

fn default_ai_model() -> String {
    "claude-haiku-4-5".to_string()
}

fn default_ai_fallback() -> bool {
    true // Always fall back to non-AI context if AI fails
}

fn default_capture_tools() -> Vec<String> {
    vec!["Write".to_string(), "Edit".to_string(), "Bash".to_string()]
}

fn default_context_limit() -> usize {
    5
}

fn default_token_budget() -> usize {
    4000 // Default ~4k tokens for context injection
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            // Legacy fields (kept for backward compatibility)
            capture_enabled: true,
            capture_tools: default_capture_tools(),
            inject_context: true,
            context_limit: 5,
            generate_summaries: false,
            token_budget: default_token_budget(),
            ai_context: false,
            ai_model: default_ai_model(),
            ai_fallback: default_ai_fallback(),
            plan_mode: PlanModeConfig::default(),
            minimal_start: false,
            // Per-hook configurations
            session_start: SessionStartHookConfig::default(),
            post_tool_use: PostToolUseHookConfig::default(),
            pre_tool_use: PreToolUseHookConfig::default(),
            stop: StopHookConfig::default(),
            user_prompt_submit: UserPromptSubmitHookConfig::default(),
            permission_request: PermissionRequestHookConfig::default(),
            pre_compact: PreCompactHookConfig::default(),
            notification: NotificationHookConfig::default(),
            // MessageDisplay guard — off by default (opt-in)
            message_display_guard: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pre_tool_use_matcher_covers_factory_intercepts() {
        let matcher = default_pre_tool_use_matcher();
        // `Skill`/`Workflow` (cas-bcfb, GH #125): the cas-code-review
        // ownership gate is unreachable unless PreToolUse fires for the
        // harness-native tools that enter the pipeline.
        for tool in ["SendMessage", "AskUserQuestion", "Skill", "Workflow"] {
            assert!(
                matcher.iter().any(|entry| entry == tool),
                "PreToolUse matcher must include factory intercept tool {tool}: {matcher:?}"
            );
        }
    }
}
