use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_coordination_lease_telemetry_and_missing(registry: &mut ConfigRegistry) {
    // FACTORY SECTION
    // ============================================================
    registry.register(ConfigMeta {
        key: "factory.artifacts_root",
        section: "factory",
        name: "Durable Task Artifacts Root",
        description: "Real-disk root for per-task durable proof. Workers may write only in their worktree, this root/<task-id>, or a harness scratchpad; bare /tmp and stray home files are blocked.",
        value_type: ConfigType::String,
        default: "~/.cas/artifacts",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "artifacts", "proof", "durable", "workspace", "tmpfs"],
        use_cases: &[
            "Set to a durable volume such as /mnt/datacube/agent-scratch",
            "Leave unset to use ~/.cas/artifacts",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.ai_enrichment.enabled",
        section: "factory",
        name: "AI Enrichment",
        description: "DEFAULT OFF. Enabling this sends redacted terminal transcript excerpts to a third-party API from a machine that may hold secrets. Configure a local OpenAI-compatible endpoint when transcripts must not leave the machine or tailnet.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["commander", "summary", "session", "privacy", "transcript", "AI"],
        use_cases: &["Enable concise session titles and phase cards", "Keep disabled when terminal excerpts must never reach a provider"],
    });
    registry.register(ConfigMeta {
        key: "factory.ai_enrichment.endpoint",
        section: "factory",
        name: "Session Summary Provider Endpoint",
        description: "OpenAI Responses-compatible endpoint used for opt-in session summaries. Point this at a local provider to keep redacted transcript excerpts on the machine or tailnet.",
        value_type: ConfigType::String,
        default: "https://api.openai.com/v1/responses",
        constraint: Constraint::NotEmpty,
        advanced: true,
        requires_feature: None,
        keywords: &["summary", "provider", "endpoint", "local model", "privacy"],
        use_cases: &["Use the OpenAI Responses API", "Use an OpenAI-compatible local model server"],
    });
    registry.register(ConfigMeta {
        key: "factory.ai_enrichment.provider",
        section: "factory",
        name: "AI Enrichment Provider",
        description: "Provider protocol for the shared AI enrichment worker. Use openai or openai-compatible.",
        value_type: ConfigType::String,
        default: "openai",
        constraint: Constraint::OneOf(vec!["openai".to_string(), "openai-compatible".to_string()]),
        advanced: true,
        requires_feature: None,
        keywords: &["AI", "provider", "local model"],
        use_cases: &["Use OpenAI", "Use an OpenAI-compatible local endpoint"],
    });
    registry.register(ConfigMeta {
        key: "factory.ai_enrichment.api_key_env",
        section: "factory",
        name: "AI Enrichment API Key Environment Variable",
        description: "Name of the environment variable containing the provider credential. The credential is used only as an Authorization header and is never placed in model input.",
        value_type: ConfigType::String,
        default: "OPENAI_API_KEY",
        constraint: Constraint::NotEmpty,
        advanced: true,
        requires_feature: None,
        keywords: &["AI", "API key", "environment", "credential"],
        use_cases: &["Use OPENAI_API_KEY", "Use a local provider without setting the variable"],
    });
    registry.register(ConfigMeta {
        key: "factory.ai_enrichment.model",
        section: "factory",
        name: "Session Summary Model",
        description: "Low-cost model used for session-card summaries.",
        value_type: ConfigType::String,
        default: "gpt-5.6-luna",
        constraint: Constraint::NotEmpty,
        advanced: true,
        requires_feature: None,
        keywords: &["summary", "model", "luna"],
        use_cases: &["Pin the guide-recommended gpt-5.6-luna model"],
    });
    registry.register(ConfigMeta {
        key: "factory.ai_enrichment.effort",
        section: "factory",
        name: "AI Enrichment Reasoning Effort",
        description: "Reasoning effort for low-latency enrichment. The shared worker requires low effort.",
        value_type: ConfigType::String,
        default: "low",
        constraint: Constraint::OneOf(vec!["low".to_string()]),
        advanced: true,
        requires_feature: None,
        keywords: &["AI", "effort", "latency", "cost"],
        use_cases: &["Keep low for fast, inexpensive summarization"],
    });

    // COORDINATION SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "coordination.mode",
            section: "coordination",
            name: "Coordination Mode",
            description: "Agent coordination mode. 'local' for standalone operation, 'cloud' for multi-device sync via CAS Cloud.",
            value_type: ConfigType::String,
            default: "local",
            constraint: Constraint::OneOf(vec!["local".to_string(), "cloud".to_string()]),
            advanced: false,
            requires_feature: None,
            keywords: &["coordination", "mode", "local", "cloud", "sync", "multi-device"],
            use_cases: &[
                "Use 'local' for single-machine development",
                "Use 'cloud' for team collaboration or multi-device sync",
            ],
        });

    registry.register(ConfigMeta {
            key: "coordination.cloud_url",
            section: "coordination",
            name: "Cloud URL",
            description: "URL of the CAS Cloud server for cloud coordination mode. Only used when coordination.mode is 'cloud'.",
            value_type: ConfigType::String,
            default: "",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["cloud", "url", "server", "endpoint", "api"],
            use_cases: &[
                "Set to your CAS Cloud instance URL",
                "Leave empty to use default CAS Cloud",
            ],
        });

    // ============================================================
    // LEASE SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "lease.default_duration_mins",
            section: "lease",
            name: "Default Duration",
            description: "Default task lease duration in minutes. Tasks are automatically released if the lease expires without renewal.",
            value_type: ConfigType::Int,
            default: "30",
            constraint: Constraint::Range(1, 480),
            advanced: false,
            requires_feature: None,
            keywords: &["lease", "duration", "timeout", "task", "minutes"],
            use_cases: &[
                "Increase for long-running tasks",
                "Decrease for faster task turnover in multi-agent scenarios",
            ],
        });

    registry.register(ConfigMeta {
            key: "lease.max_duration_mins",
            section: "lease",
            name: "Max Duration",
            description: "Maximum allowed task lease duration in minutes. Prevents tasks from being locked indefinitely.",
            value_type: ConfigType::Int,
            default: "240",
            constraint: Constraint::Range(30, 1440),
            advanced: true,
            requires_feature: None,
            keywords: &["lease", "maximum", "limit", "cap", "duration"],
            use_cases: &[
                "Increase for very long tasks that need extended ownership",
                "Decrease to ensure faster task recycling",
            ],
        });

    registry.register(ConfigMeta {
        key: "lease.heartbeat_interval_secs",
        section: "lease",
        name: "Heartbeat Interval",
        description: "How often agents send heartbeats to renew their task leases, in seconds.",
        value_type: ConfigType::Int,
        default: "300",
        constraint: Constraint::Range(30, 900),
        advanced: true,
        requires_feature: None,
        keywords: &["heartbeat", "interval", "renewal", "keepalive", "ping"],
        use_cases: &[
            "Decrease for more responsive lease management",
            "Increase to reduce overhead in stable environments",
        ],
    });

    registry.register(ConfigMeta {
            key: "lease.expiry_grace_secs",
            section: "lease",
            name: "Expiry Grace Period",
            description: "Grace period in seconds after a lease expires before the task is released. Allows for network delays.",
            value_type: ConfigType::Int,
            default: "120",
            constraint: Constraint::Range(30, 600),
            advanced: true,
            requires_feature: None,
            keywords: &["grace", "expiry", "buffer", "delay", "tolerance"],
            use_cases: &[
                "Increase for unreliable network conditions",
                "Decrease for faster task recycling on failures",
            ],
        });

    // ============================================================
    // TELEMETRY SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "telemetry.enabled",
            section: "telemetry",
            name: "Enable Telemetry",
            description: "Enable anonymous usage telemetry to help improve CAS. Opt-in via CAS_TELEMETRY=1 or this setting. No personal or code data is collected.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["telemetry", "analytics", "usage", "metrics", "anonymous"],
            use_cases: &[
                "Disable for complete privacy",
                "Enable to help improve CAS with anonymous usage data",
            ],
        });

    // ============================================================
    // MISSING FROM EXISTING SECTIONS
    // ============================================================

    // tasks.block_exit_on_open
    registry.register(ConfigMeta {
            key: "tasks.block_exit_on_open",
            section: "tasks",
            name: "Block Exit on Open Tasks",
            description: "Prevent session exit when there are open tasks assigned to the agent. Ensures tasks are completed or reassigned before stopping.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["block", "exit", "open", "tasks", "prevent", "stop"],
            use_cases: &[
                "Disable to allow stopping with unfinished tasks",
                "Enable to ensure all tasks are handled before exit",
            ],
        });

    // notifications.on_permission_prompt
    registry.register(ConfigMeta {
        key: "notifications.on_permission_prompt",
        section: "notifications",
        name: "On Permission Prompt",
        description: "Show notification when Claude Code requests a permission prompt.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &[
            "permission",
            "prompt",
            "notification",
            "approval",
            "request",
        ],
        use_cases: &[
            "Enable to be alerted when Claude needs approval",
            "Disable if permission prompts are too frequent",
        ],
    });

    // notifications.on_idle_prompt
    registry.register(ConfigMeta {
        key: "notifications.on_idle_prompt",
        section: "notifications",
        name: "On Idle Prompt",
        description: "Show notification when Claude Code becomes idle awaiting input.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["idle", "prompt", "notification", "waiting", "input"],
        use_cases: &[
            "Enable to be alerted when Claude is waiting for you",
            "Disable to reduce notification noise",
        ],
    });

    // notifications.on_auth_success
    registry.register(ConfigMeta {
        key: "notifications.on_auth_success",
        section: "notifications",
        name: "On Auth Success",
        description: "Show notification when CAS Cloud authentication succeeds.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["auth", "authentication", "login", "success", "cloud"],
        use_cases: &[
            "Enable to confirm cloud login",
            "Disable if auth notifications are unnecessary",
        ],
    });

    // notifications.webhook_url
    registry.register(ConfigMeta {
            key: "notifications.webhook_url",
            section: "notifications",
            name: "Webhook URL",
            description: "Optional webhook URL for sending notifications to external services (Slack, Discord, etc.).",
            value_type: ConfigType::String,
            default: "",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["webhook", "url", "slack", "discord", "external", "integration"],
            use_cases: &[
                "Set to Slack webhook URL for team notifications",
                "Set to Discord webhook for personal alerts",
                "Leave empty to disable external notifications",
            ],
        });
}
