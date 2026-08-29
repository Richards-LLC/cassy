use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_hooks_and_code(registry: &mut ConfigRegistry) {
    // ============================================================
    // SYNC SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "sync.enabled",
            section: "sync",
            name: "Enable Sync",
            description: "Whether to automatically sync proven rules to .claude/rules/. When enabled, rules that reach 'Proven' status are copied to the Claude Code rules directory for automatic inclusion.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["rules", "synchronization", "claude", "proven", "automatic"],
            use_cases: &[
                "Disable to manually manage rule files",
                "Enable to auto-publish validated rules to Claude Code",
            ],
        });

    registry.register(ConfigMeta {
            key: "sync.target",
            section: "sync",
            name: "Target Directory",
            description: "Target directory for synced rules, relative to project root. Default is .claude/rules/cas which keeps Cassy rules separate from manually created rules.",
            value_type: ConfigType::String,
            default: ".claude/rules/cas",
            constraint: Constraint::NotEmpty,
            advanced: false,
            requires_feature: None,
            keywords: &["directory", "path", "rules", "location", "folder"],
            use_cases: &[
                "Change to organize rules in a custom directory structure",
                "Set to .claude/rules/ to mix with manual rules",
            ],
        });

    registry.register(ConfigMeta {
            key: "sync.min_helpful",
            section: "sync",
            name: "Minimum Helpful Votes",
            description: "Minimum number of 'helpful' votes required before a rule is synced. Higher values ensure only well-validated rules are synced.",
            value_type: ConfigType::Int,
            default: "1",
            constraint: Constraint::Min(0),
            advanced: false,
            requires_feature: None,
            keywords: &["votes", "threshold", "validation", "quality", "approval"],
            use_cases: &[
                "Increase to require more validation before syncing",
                "Set to 0 to sync all proven rules immediately",
            ],
        });

    registry.register(ConfigMeta {
            key: "sync.promotion_threshold",
            section: "sync",
            name: "Rule Promotion Threshold",
            description: "Minimum number of outcome-evidence events required before an unproven rule becomes Proven. Existing Proven rules are grandfathered and are not mass-demoted when this changes. The minimum accepted value is 2 so one self-reported call can never promote a rule; retrieval evidence also requires outcomes from at least two distinct privacy-preserving sessions.",
            value_type: ConfigType::Int,
            default: "2",
            constraint: Constraint::Min(2),
            advanced: false,
            requires_feature: None,
            keywords: &["rules", "promotion", "threshold", "evidence", "validation", "quality"],
            use_cases: &[
                "Raise to require more independent outcome evidence before promotion",
                "Keep the default of 2 to prevent one-call promotions",
            ],
        });

    registry.register(ConfigMeta {
            key: "sync.promotion_evidence",
            section: "sync",
            name: "Rule Promotion Evidence",
            description: "Comma-separated evidence sources that may satisfy the promotion threshold: helpful uses explicit rule feedback; retrieval uses per-rule retrieval outcome aggregates and requires useful, uncorrected evidence from at least two distinct privacy-preserving sessions. Any configured source may satisfy the threshold.",
            value_type: ConfigType::StringList,
            default: "helpful",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["rules", "promotion", "evidence", "retrieval", "feedback", "validation"],
            use_cases: &[
                "Use helpful for explicit rule feedback",
                "Use retrieval to require measured retrieval outcomes",
                "List both sources when either configured signal may qualify",
            ],
        });

    registry.register(ConfigMeta {
            key: "sync.demotion_threshold",
            section: "sync",
            name: "Rule Demotion Threshold",
            description: "Minimum number of harmful or corrected outcome events required before a Proven rule becomes Stale. Retrieval evidence also requires outcomes from at least two distinct privacy-preserving sessions. Existing Proven rules are grandfathered until new negative evidence reaches this threshold.",
            value_type: ConfigType::Int,
            default: "2",
            constraint: Constraint::Min(2),
            advanced: false,
            requires_feature: None,
            keywords: &["rules", "demotion", "threshold", "evidence", "harmful", "corrected"],
            use_cases: &[
                "Raise to require more negative evidence before demotion",
                "Keep the default of 2 to avoid one self-reported negative event removing a rule",
            ],
        });

    // ============================================================
    // CLOUD SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "cloud.auto_sync",
            section: "cloud",
            name: "Auto Sync",
            description: "Automatically sync changes to Cassy Cloud when logged in. Sync happens in the background at the configured interval.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["cloud", "sync", "backup", "remote", "automatic", "background"],
            use_cases: &[
                "Disable for offline-only usage",
                "Enable to sync across multiple devices",
            ],
        });

    registry.register(ConfigMeta {
            key: "cloud.interval_secs",
            section: "cloud",
            name: "Sync Interval",
            description: "How often to sync with Cassy Cloud, in seconds. Lower values provide faster sync but more network usage.",
            value_type: ConfigType::Int,
            default: "60",
            constraint: Constraint::Range(10, 3600),
            advanced: false,
            requires_feature: None,
            keywords: &["frequency", "interval", "polling", "seconds", "timing"],
            use_cases: &[
                "Decrease for faster cross-device sync",
                "Increase to reduce network usage and battery drain",
            ],
        });

    registry.register(ConfigMeta {
            key: "cloud.queue_pending_warning",
            section: "cloud",
            name: "Queue Pending Warning",
            description: "Warn in factory preflight when this many local cloud changes are still waiting to push.",
            value_type: ConfigType::Int,
            default: "200",
            constraint: Constraint::Range(1, 1000000),
            advanced: false,
            requires_feature: None,
            keywords: &["cloud", "sync", "queue", "warning", "health"],
            use_cases: &["Lower to detect sync wedges sooner", "Raise for intentionally large offline batches"],
        });

    registry.register(ConfigMeta {
            key: "cloud.queue_oldest_warning_secs",
            section: "cloud",
            name: "Queue Age Warning",
            description: "Warn in factory preflight when the oldest local cloud change has waited this long to push.",
            value_type: ConfigType::Int,
            default: "21600",
            constraint: Constraint::Range(60, 604800),
            advanced: false,
            requires_feature: None,
            keywords: &["cloud", "sync", "queue", "age", "warning", "health"],
            use_cases: &["Detect a stalled sync daemon", "Tune for a planned offline period"],
        });

    registry.register(ConfigMeta {
            key: "cloud.pull_on_start",
            section: "cloud",
            name: "Pull on Start",
            description: "Pull latest changes from Cassy Cloud when the MCP server starts. Ensures you have the latest data from other devices.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["startup", "fetch", "download", "initial", "refresh"],
            use_cases: &[
                "Disable if you want faster startup",
                "Enable to always have latest data when starting",
            ],
        });

    registry.register(ConfigMeta {
        key: "cloud.max_retries",
        section: "cloud",
        name: "Max Retries",
        description:
            "Maximum number of retry attempts for failed sync operations before giving up.",
        value_type: ConfigType::Int,
        default: "5",
        constraint: Constraint::Range(1, 20),
        advanced: true,
        requires_feature: None,
        keywords: &["retry", "failure", "network", "resilience", "attempts"],
        use_cases: &[
            "Increase for unreliable network connections",
            "Decrease to fail faster on network issues",
        ],
    });

    // ============================================================
    // HOOKS SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "hooks.capture_enabled",
            section: "hooks",
            name: "Enable Capture",
            description: "Capture observations from Claude Code tool calls. When enabled, Write/Edit/Bash operations are recorded for later processing.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["capture", "observation", "recording", "tools", "tracking"],
            use_cases: &[
                "Disable to reduce storage usage",
                "Enable to build learning from Claude's actions",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.capture_tools",
            section: "hooks",
            name: "Capture Tools",
            description: "Comma-separated list of tool names to capture observations from. Default captures file operations and shell commands.",
            value_type: ConfigType::StringList,
            default: "Write,Edit,Bash",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["tools", "write", "edit", "bash", "filter", "list"],
            use_cases: &[
                "Add Read to capture file reads",
                "Remove Bash to skip shell command capture",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.inject_context",
            section: "hooks",
            name: "Inject Context",
            description: "Inject relevant Cassy context at session start. Provides Claude with helpful memories, tasks, and rules.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["context", "injection", "session", "startup", "memories", "tasks"],
            use_cases: &[
                "Disable for completely fresh sessions",
                "Enable to give Claude project context automatically",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.context_limit",
            section: "hooks",
            name: "Context Entry Limit",
            description: "Maximum number of memory entries to include in context injection. Higher values provide more context but use more tokens.",
            value_type: ConfigType::Int,
            default: "5",
            constraint: Constraint::Range(1, 50),
            advanced: false,
            requires_feature: None,
            keywords: &["limit", "entries", "memories", "count", "maximum"],
            use_cases: &[
                "Increase for complex projects needing more context",
                "Decrease to keep sessions lean and fast",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.generate_summaries",
            section: "hooks",
            name: "Generate Summaries",
            description: "Generate AI summaries at session end. Requires the 'ai-extraction' feature to be enabled.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: Some("ai-extraction"),
            keywords: &["summary", "ai", "extraction", "session", "end"],
            use_cases: &[
                "Enable to auto-generate session summaries",
                "Disable to save API costs",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.token_budget",
            section: "hooks",
            name: "Token Budget",
            description: "Maximum tokens to use for context injection. Set to 0 for unlimited. Higher values allow more context but increase prompt size.",
            value_type: ConfigType::Int,
            default: "4000",
            constraint: Constraint::Min(0),
            advanced: false,
            requires_feature: None,
            keywords: &["tokens", "budget", "limit", "context", "size", "prompt"],
            use_cases: &[
                "Set lower (2000) for faster, leaner sessions",
                "Set higher (8000) for complex projects",
                "Set to 0 for unlimited context",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.ai_context",
            section: "hooks",
            name: "AI Context",
            description: "Use AI to prioritize context items instead of heuristics. Requires claude CLI to be available.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: Some("ai-extraction"),
            keywords: &["ai", "prioritization", "smart", "ranking", "relevance"],
            use_cases: &[
                "Enable for smarter context selection",
                "Disable to use faster heuristic-based selection",
            ],
        });

    registry.register(ConfigMeta {
        key: "hooks.ai_model",
        section: "hooks",
        name: "AI Model",
        description:
            "Model to use for AI context prioritization. Haiku is recommended for speed and cost.",
        value_type: ConfigType::String,
        default: "claude-haiku-4-5",
        constraint: Constraint::NotEmpty,
        advanced: true,
        requires_feature: Some("ai-extraction"),
        keywords: &["model", "haiku", "sonnet", "opus", "claude"],
        use_cases: &[
            "Use haiku for fast, cheap prioritization",
            "Use sonnet for better quality at higher cost",
        ],
    });

    registry.register(ConfigMeta {
            key: "hooks.ai_fallback",
            section: "hooks",
            name: "AI Fallback",
            description: "Fall back to non-AI context selection if AI prioritization fails. Recommended to keep enabled for reliability.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: Some("ai-extraction"),
            keywords: &["ai", "fallback", "graceful", "degradation", "reliability"],
            use_cases: &[
                "Enable for reliable context injection even when AI is unavailable",
                "Disable to fail fast if AI prioritization is critical",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.minimal_start",
            section: "hooks",
            name: "Minimal Start",
            description: "Start sessions with minimal context (only blocked tasks and pinned memories). Follows context engineering best practice of starting lean.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["minimal", "lean", "startup", "fast", "light"],
            use_cases: &[
                "Enable for faster session starts",
                "Enable when you want Claude to ask for context as needed",
            ],
        });

    // ============================================================
    // STAGING SECTION
    // ============================================================
    registry.register(ConfigMeta {
        key: "staging.staging_dir",
        section: "staging",
        name: "Durable Staging Directory",
        description: "Approved durable directory for large generated artifacts. Alias: staging.large_artifact_dir. When configured, tmpfs/ramfs write guardrail warnings tell agents to restate this location before continuing large writes.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["staging", "durable", "tmpfs", "ramfs", "large", "artifacts", "directory"],
        use_cases: &[
            "Point agents at a disk-backed mount for generated media, archives, and build outputs",
            "Avoid filling /tmp or other memory-backed filesystems during long sessions",
        ],
    });

    registry.register(ConfigMeta {
        key: "staging.scratch_root",
        section: "staging",
        name: "Agent Scratch Root",
        description: "Configured root for ephemeral agent writes. The PreToolUse workspace guard permits file and directory creation below this root, while writes outside the worktree, durable artifacts root, system temp directory, and explicit harness exceptions are denied.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["staging", "scratch", "workspace", "guardrail", "pretooluse", "agent", "temporary"],
        use_cases: &[
            "Keep agent-generated logs and temporary fixtures under one disk-backed root",
            "Enforce a host-specific scratch location without moving source or durable proof",
        ],
    });

    registry.register(ConfigMeta {
        key: "staging.tmpfs_warning_threshold_bytes",
        section: "staging",
        name: "tmpfs Warning Threshold",
        description: "Warn from the PostToolUse hook when cumulative session writes or usage growth on a tmpfs/ramfs-backed mount crosses this byte threshold. The hook only warns; it never denies tool execution.",
        value_type: ConfigType::Int,
        default: "1073741824",
        constraint: Constraint::Min(1),
        advanced: false,
        requires_feature: None,
        keywords: &["tmpfs", "ramfs", "threshold", "warning", "bytes", "memory", "posttooluse"],
        use_cases: &[
            "Lower on memory-constrained hosts to surface risk earlier",
            "Raise for hosts with intentionally large memory-backed scratch mounts",
        ],
    });

    // ============================================================
    // HOOKS.STOP SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "hooks.stop.learning_review_enabled",
            section: "hooks.stop",
            name: "Learning Review Enabled",
            description: "Block stop to review unreviewed learnings. When enabled and unreviewed learnings exceed the threshold, the stop hook blocks and prompts to spawn a learning-reviewer subagent.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["learning", "review", "stop", "block", "memories", "subagent"],
            use_cases: &[
                "Enable to ensure learnings are reviewed periodically",
                "Enable to promote valuable learnings to rules or skills",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.stop.learning_review_threshold",
            section: "hooks.stop",
            name: "Learning Review Threshold",
            description: "Number of unreviewed learnings required to trigger review. Only applies when learning_review_enabled is true.",
            value_type: ConfigType::Int,
            default: "5",
            constraint: Constraint::Range(1, 100),
            advanced: false,
            requires_feature: None,
            keywords: &["learning", "review", "threshold", "count", "trigger"],
            use_cases: &[
                "Increase to allow more learnings to accumulate before review",
                "Decrease to trigger reviews more frequently",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.stop.rule_review_enabled",
            section: "hooks.stop",
            name: "Rule Review Enabled",
            description: "Block stop to review draft rules. When enabled and draft rules exceed the threshold, spawns a rule-reviewer subagent.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["rule", "review", "stop", "block", "draft", "subagent"],
            use_cases: &[
                "Enable to ensure draft rules are promoted or archived",
                "Enable to maintain rule quality over time",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.stop.rule_review_threshold",
            section: "hooks.stop",
            name: "Rule Review Threshold",
            description: "Number of draft rules required to trigger review. Only applies when rule_review_enabled is true.",
            value_type: ConfigType::Int,
            default: "5",
            constraint: Constraint::Range(1, 50),
            advanced: false,
            requires_feature: None,
            keywords: &["rule", "review", "threshold", "draft", "count"],
            use_cases: &[
                "Increase to allow more draft rules before review",
                "Decrease to trigger reviews more frequently",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.stop.duplicate_detection_enabled",
            section: "hooks.stop",
            name: "Duplicate Detection Enabled",
            description: "Block stop to run duplicate detection. When enabled and entries exceed threshold, spawns a duplicate-detector subagent.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["duplicate", "detection", "cleanup", "memory", "consolidate"],
            use_cases: &[
                "Enable to keep memories clean and deduplicated",
                "Enable to reduce context bloat from duplicate entries",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.stop.duplicate_detection_threshold",
            section: "hooks.stop",
            name: "Duplicate Detection Threshold",
            description: "Minimum entries before triggering duplicate detection. Only applies when duplicate_detection_enabled is true.",
            value_type: ConfigType::Int,
            default: "20",
            constraint: Constraint::Range(5, 200),
            advanced: false,
            requires_feature: None,
            keywords: &["duplicate", "threshold", "entries", "count", "cleanup"],
            use_cases: &[
                "Increase to run detection less frequently",
                "Decrease to keep memory cleaner",
            ],
        });

    // ============================================================
    // HOOKS.PLAN_MODE SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "hooks.plan_mode.enabled",
            section: "hooks.plan_mode",
            name: "Enable Plan Mode",
            description: "Use plan-aware context when Claude is in planning mode. Provides more comprehensive task and memory information.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["plan", "planning", "mode", "context", "comprehensive"],
            use_cases: &[
                "Disable to use same context for plan and execution",
                "Enable for richer context during planning phases",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.plan_mode.token_budget",
            section: "hooks.plan_mode",
            name: "Plan Token Budget",
            description: "Token budget for plan mode context. Typically higher than execution mode to allow for comprehensive planning information.",
            value_type: ConfigType::Int,
            default: "8000",
            constraint: Constraint::Min(0),
            advanced: false,
            requires_feature: None,
            keywords: &["tokens", "budget", "planning", "limit", "size"],
            use_cases: &[
                "Increase for complex multi-task planning",
                "Decrease if plan mode context is too verbose",
            ],
        });

    registry.register(ConfigMeta {
        key: "hooks.plan_mode.task_limit",
        section: "hooks.plan_mode",
        name: "Task Limit",
        description: "Maximum number of tasks to show in plan mode context.",
        value_type: ConfigType::Int,
        default: "15",
        constraint: Constraint::Range(1, 100),
        advanced: false,
        requires_feature: None,
        keywords: &["tasks", "limit", "count", "maximum", "planning"],
        use_cases: &[
            "Increase for projects with many interdependent tasks",
            "Decrease to focus on fewer high-priority tasks",
        ],
    });

    registry.register(ConfigMeta {
            key: "hooks.plan_mode.show_dependencies",
            section: "hooks.plan_mode",
            name: "Show Dependencies",
            description: "Include task dependency trees in plan mode context. Helps understand task relationships.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["dependencies", "tree", "relationships", "blocked", "blocking"],
            use_cases: &[
                "Disable if dependency info is too noisy",
                "Enable to help Claude understand task ordering",
            ],
        });

    registry.register(ConfigMeta {
            key: "hooks.plan_mode.include_closed",
            section: "hooks.plan_mode",
            name: "Include Closed Tasks",
            description: "Include recently closed tasks in plan mode for reference. Useful for understanding recent work.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["closed", "completed", "history", "recent", "done"],
            use_cases: &[
                "Enable to provide context about recent completions",
                "Enable when continuing work from previous sessions",
            ],
        });

    registry.register(ConfigMeta {
        key: "hooks.plan_mode.semantic_search",
        section: "hooks.plan_mode",
        name: "Semantic Search",
        description: "Search for related memories using semantic similarity in plan mode.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["semantic", "search", "embeddings", "similarity", "related"],
        use_cases: &[
            "Disable if you prefer keyword-based search only",
            "Enable for finding conceptually related memories",
        ],
    });

    // ============================================================
    // TASKS SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "tasks.commit_nudge_on_close",
            section: "tasks",
            name: "Commit Nudge",
            description: "Prompt to commit changes when closing a task. Encourages atomic commits tied to task completion.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["commit", "git", "nudge", "prompt", "close", "atomic"],
            use_cases: &[
                "Enable to encourage atomic commits per task",
                "Enable for better git history tracking",
            ],
        });

    // ============================================================
    // DEV SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "dev.dev_mode",
            section: "dev",
            name: "Dev Mode",
            description: "Enable development mode with enhanced tracing and diagnostics. Useful for debugging Cassy behavior.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["debug", "development", "tracing", "diagnostics", "logging"],
            use_cases: &[
                "Enable when debugging Cassy issues",
                "Enable when developing Cassy features",
            ],
        });

    registry.register(ConfigMeta {
        key: "dev.trace_commands",
        section: "dev",
        name: "Trace Commands",
        description: "Trace CLI command executions in dev mode.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["trace", "commands", "cli", "logging", "debug"],
        use_cases: &[
            "Disable to reduce trace noise",
            "Enable to debug CLI command flow",
        ],
    });

    registry.register(ConfigMeta {
        key: "dev.trace_store_ops",
        section: "dev",
        name: "Trace Store Operations",
        description: "Trace store operations (add, update, delete, get) in dev mode.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["trace", "store", "database", "operations", "sqlite"],
        use_cases: &[
            "Enable to debug database operations",
            "Disable if store traces are too verbose",
        ],
    });

    registry.register(ConfigMeta {
        key: "dev.trace_claude_api",
        section: "dev",
        name: "Trace Claude API",
        description: "Trace Claude API calls with full prompts and responses in dev mode.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["trace", "api", "claude", "prompts", "responses"],
        use_cases: &[
            "Enable to debug AI interactions",
            "Disable to reduce trace size (prompts can be large)",
        ],
    });

    registry.register(ConfigMeta {
        key: "dev.trace_hooks",
        section: "dev",
        name: "Trace Hooks",
        description: "Trace hook events in dev mode.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["trace", "hooks", "events", "callbacks", "lifecycle"],
        use_cases: &[
            "Enable to debug hook execution",
            "Disable if hook traces are too noisy",
        ],
    });

    registry.register(ConfigMeta {
        key: "dev.trace_retention_days",
        section: "dev",
        name: "Trace Retention",
        description: "Days to retain traces before automatic cleanup.",
        value_type: ConfigType::Int,
        default: "7",
        constraint: Constraint::Range(1, 365),
        advanced: true,
        requires_feature: None,
        keywords: &["retention", "cleanup", "days", "storage", "purge"],
        use_cases: &[
            "Increase if you need longer trace history",
            "Decrease to save disk space",
        ],
    });

    // ============================================================
    // CODE SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "code.enabled",
            section: "code",
            name: "Enable Code Indexing",
            description: "Enable background code indexing for semantic code search. Indexes source files and extracts symbols (functions, classes, etc.) for search.",
            value_type: ConfigType::Bool,
            // Keep metadata in lockstep with `CodeConfig::default()`: code indexing is
            // enabled for a fresh install, while an explicit false remains an override.
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["code", "indexing", "search", "symbols", "functions", "background"],
            use_cases: &[
                "Enable for semantic code search via mcp__cas__search code_search",
                "Disable on low-resource machines to save CPU",
            ],
        });

    registry.register(ConfigMeta {
            key: "code.watch_paths",
            section: "code",
            name: "Watch Paths",
            description: "Directories to watch for code changes (relative to project root). Default watches common source directories.",
            value_type: ConfigType::StringList,
            default: "src,lib,crates",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["paths", "directories", "source", "watch", "scan"],
            use_cases: &[
                "Add custom source directories like 'app' or 'packages'",
                "Remove unused paths to reduce indexing scope",
            ],
        });

    registry.register(ConfigMeta {
            key: "code.exclude_patterns",
            section: "code",
            name: "Exclude Patterns",
            description: "Glob patterns for directories/files to exclude from indexing. Excludes build artifacts and dependencies by default.",
            value_type: ConfigType::StringList,
            default: "target/**,node_modules/**,.git/**,dist/**,build/**,_build/**,deps/**,vendor/**",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["exclude", "ignore", "patterns", "glob", "filter"],
            use_cases: &[
                "Add project-specific build directories",
                "Exclude generated code directories",
            ],
        });

    registry.register(ConfigMeta {
            key: "code.extensions",
            section: "code",
            name: "File Extensions",
            description: "File extensions to index (without leading dot). Common programming languages included by default.",
            value_type: ConfigType::StringList,
            default: "rs,ts,tsx,js,jsx,py,go,ex,exs,rb,java,kt,swift",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["extensions", "languages", "files", "types", "filetypes"],
            use_cases: &[
                "Add project-specific extensions like 'vue' or 'svelte'",
                "Remove unused language extensions to reduce index size",
            ],
        });

    registry.register(ConfigMeta {
            key: "code.index_interval_secs",
            section: "code",
            name: "Index Interval",
            description: "How often to run full code indexing in seconds. Lower values keep index fresher but use more CPU.",
            value_type: ConfigType::Int,
            default: "60",
            constraint: Constraint::Range(10, 3600),
            advanced: true,
            requires_feature: None,
            keywords: &["interval", "frequency", "indexing", "refresh", "timing"],
            use_cases: &[
                "Decrease for active development with frequent file changes",
                "Increase for stable codebases to reduce CPU usage",
            ],
        });

    registry.register(ConfigMeta {
            key: "code.debounce_ms",
            section: "code",
            name: "Debounce Time",
            description: "Debounce time for file watcher events in milliseconds. Batches rapid file changes together.",
            value_type: ConfigType::Int,
            default: "500",
            constraint: Constraint::Range(100, 5000),
            advanced: true,
            requires_feature: None,
            keywords: &["debounce", "delay", "batch", "watcher", "events"],
            use_cases: &[
                "Increase if file saves trigger too many re-indexes",
                "Decrease for faster response to file changes",
            ],
        });

    // ============================================================
}
