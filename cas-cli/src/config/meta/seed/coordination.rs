use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_coordination_lease_telemetry_and_missing(registry: &mut ConfigRegistry) {
    // FACTORY SECTION
    // ============================================================
    for (key, name, description) in [
        ("factory.supervisor_only_mcp", "Supervisor-only MCP Servers", "Exact MCP server names excluded from worker project configuration, inherited Claude MCP scopes, native Codex servers and proxy connections. Supervisors retain them; empty keeps existing policy."),
        ("factory.supervisor_only_env", "Supervisor-only Environment", "Environment variable names removed from every worker, even when a project proxy grants them. Machine credential bootstrap cannot restore them. Supervisors retain them; empty keeps existing credential protections."),
        ("factory.worker_credential_env", "Worker Credential Environment", "Explicit environment names granted to every worker harness from the operator environment or existing credential-file/profile resolution. Missing names warn without refusing spawn; supervisor_only_env wins conflicts. Factory identity variables cannot be granted."),
    ] {
        registry.register(ConfigMeta {
            key, section: "factory", name, description,
            value_type: ConfigType::StringList, default: "", constraint: Constraint::None,
            advanced: false, requires_feature: None,
            keywords: &["factory", "worker", "supervisor", "mcp", "credentials", "environment"],
            use_cases: &["Keep deployment and production database access on the supervisor"],
        });
    }
    registry.register(ConfigMeta {
        key: "factory.artifacts_root",
        section: "factory",
        name: "Durable Task Artifacts Root",
        description: "Real-disk parent for project-scoped durable proof. Workers write in their worktree, this root/<project-key>/<task-id>, or a harness scratchpad. Task briefs provide the exact directory; legacy flat evidence remains readable. Bare /tmp and stray home files are blocked.",
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
        key: "factory.message_max_chars",
        section: "factory",
        name: "Agent Message Character Cap",
        description: "Reject ordinary coordination message bodies longer than this many characters. Store detailed evidence under [factory] artifacts_root/<project-key>/<task-id>/ and send its path with a short summary.",
        value_type: ConfigType::Int,
        default: "1200",
        constraint: Constraint::Range(1, 1_000_000),
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "message", "coordination", "cap", "characters", "traffic"],
        use_cases: &["Keep ordinary worker-to-worker traffic compact", "Raise the cap only when a project has a specific short-message need"],
    });

    registry.register(ConfigMeta {
        key: "factory.message_max_chars_escalation",
        section: "factory",
        name: "Escalation Message Character Cap",
        description: "Reject blocker and merge-request coordination message bodies longer than this many characters. These escalation types receive structured CAS envelopes and have a larger default budget.",
        value_type: ConfigType::Int,
        default: "2500",
        constraint: Constraint::Range(1, 1_000_000),
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "message", "blocker", "merge", "escalation", "cap"],
        use_cases: &["Allow concise blocker evidence and merge receipts", "Keep escalations bounded while preserving their structured context"],
    });

    registry.register(ConfigMeta {
        key: "factory.note_max_chars",
        section: "factory",
        name: "Task Note Character Cap",
        description: "Reject appended task notes longer than this many characters. Store detailed evidence under [factory] artifacts_root/<project-key>/<task-id>/ and send the path with a short summary.",
        value_type: ConfigType::Int,
        default: "1500",
        constraint: Constraint::Range(1, 1_000_000),
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "task", "notes", "cap", "characters", "traffic"],
        use_cases: &["Keep task timelines readable", "Use supervisor review overrides for durable discovery or decision evidence"],
    });

    registry.register(ConfigMeta {
        key: "factory.max_concurrent_builders",
        section: "factory",
        name: "Maximum Concurrent Builders",
        description: "Builder cap for worker compile checks and targeted tests, enforced with OS slot locks. spawn_workers also refuses when a request would exceed this cap or one-minute load exceeds CPU count; force=true overrides only the spawn-time soft guard.",
        value_type: ConfigType::Int,
        default: "4",
        constraint: Constraint::Range(1, 256),
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "cargo", "build", "concurrency", "load", "workers"],
        use_cases: &[
            "Keep a shared 32-core host at four concurrent builders",
            "Raise the cap only when the host has spare CPU capacity",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.spawn_min_free_gib",
        section: "factory",
        name: "Worker Spawn Minimum Free Space (GiB)",
        description: "Refuse worker spawns below this available-space floor before checkout or reuse, whether target seeding is enabled or not. Failure names spawn_disk_floor. 0 disables the floor.",
        value_type: ConfigType::Int,
        default: "25",
        constraint: Constraint::Range(0, 65536),
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "cargo", "seed", "disk", "space", "workers"],
        use_cases: &["Fail a low-disk spawn before creating or reusing its worktree"],
    });

    registry.register(ConfigMeta {
        key: "factory.prompt_retention_days",
        section: "factory",
        name: "Prompt Queue Retention (days)",
        description: "Days a terminal prompt-queue row (delivered, acknowledged, suppressed or abandoned) is kept before it is deleted with its delivery receipts, by the maintenance sweep and by the canonical daemon every 15 minutes in transactions of at most 1,000 rows. Pending rows and rows that carry a relay episode key are never deleted by retention, except a supervisor-queue outbox key whose notification is already delivered or gone. gc_cleanup force=true uses the same window. 0 disables the sweep.",
        value_type: ConfigType::Int,
        default: "7",
        constraint: Constraint::Range(0, 3650),
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "prompt", "queue", "retention", "gc", "cleanup", "messages"],
        use_cases: &[
            "Keep a week of message forensics while bounding queue growth",
            "Set 0 to keep every terminal prompt row",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.event_telemetry_retention_days",
        section: "factory",
        name: "Telemetry Event Retention (days)",
        description: "Days high-volume telemetry events (supervisor_injected, supervisor_notified, agent_heartbeat, worker_file_edited, worker_subagent_spawned, worker_subagent_completed) stay in the events table. The canonical daemon deletes older rows every 15 minutes in transactions of at most 1,000 rows, whether or not the project is idle. Lifecycle events such as tasks, commits and verification are never pruned by this window. 0 disables the sweep.",
        value_type: ConfigType::Int,
        default: "14",
        constraint: Constraint::Range(0, 3650),
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "events", "telemetry", "retention", "prune", "database", "size"],
        use_cases: &[
            "Bound cas.db growth from supervisor injection telemetry",
            "Set 0 to keep every telemetry event",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.prompt_transcript_retention_days",
        section: "factory",
        name: "Prompt Transcript Retention (days)",
        description: "Days a captured prompt keeps its session transcript (prompts.messages_json). The canonical daemon clears older transcripts every 15 minutes in transactions of at most 1,000 rows, whether or not the project is idle. The prompt row, its text and its provenance keys (id, session, agent, task, content hash) are kept, so blame and attribution still resolve; no reader consumes the transcript. 0 disables the trim.",
        value_type: ConfigType::Int,
        default: "7",
        constraint: Constraint::Range(0, 3650),
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "prompt", "transcript", "retention", "messages", "database", "size"],
        use_cases: &[
            "Bound cas.db growth from per-session prompt transcripts",
            "Set 0 to keep every transcript",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.supervisor_queue_retention_days",
        section: "factory",
        name: "Supervisor Queue Retention (days)",
        description: "Days a finished supervisor-queue notification is kept: an outbox notification whose prompt was delivered, or a pulled notification that was processed. The canonical daemon deletes older finished rows every 15 minutes in transactions of at most 1,000 rows. Pending and undelivered notifications are never deleted, nor are keys that can recur for the same subject (worker-attention:, integration:). 0 disables the sweep.",
        value_type: ConfigType::Int,
        default: "14",
        constraint: Constraint::Range(0, 3650),
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "supervisor", "queue", "notification", "retention", "database", "size"],
        use_cases: &[
            "Bound cas.db growth from delivered lifecycle notifications",
            "Set 0 to keep every supervisor notification",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.worker_build_jobs",
        section: "factory",
        name: "Worker Cargo Build Jobs",
        description: "Per-worker CARGO_BUILD_JOBS cap. The default auto value is max(2, available CPUs / 4); factory.cargo_build_jobs remains accepted as a compatibility alias. Set a numeric value to override the computed cap.",
        value_type: ConfigType::String,
        default: "auto",
        constraint: Constraint::NotEmpty,
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "worker", "cargo", "build", "jobs", "throttle"],
        use_cases: &[
            "Keep each worker at a bounded Cargo parallelism",
            "Set a numeric cap when the fleet size differs from the default four workers",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.merge_sweep",
        section: "factory",
        name: "Post-Merge Workspace Sweep",
        description: "Run one bounded cargo nextest --workspace --no-fail-fast sweep on each merged epic tip. Disable only when the factory host cannot absorb the additional validation load.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "merge", "sweep", "nextest", "workspace", "epic"],
        use_cases: &["Catch merged-tree regressions before the next merge", "Disable on a deliberately constrained host"],
    });

    registry.register(ConfigMeta {
        key: "factory.merge_sweep_command",
        section: "factory",
        name: "Post-Merge Sweep Command",
        description: "Command the post-merge sweep runs (via sh -c in the merged-tip worktree) instead of the detected cargo nextest or package test script. Empty keeps detection. Its environment comes from the [factory.merge_sweep_env] table in config.toml, whose values are never logged.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "merge", "sweep", "command", "test", "env"],
        use_cases: &["Run the suite the way CI runs it", "Give database-backed suites their setup step"],
    });

    registry.register(ConfigMeta {
        key: "factory.release_gate_home_dir",
        section: "factory",
        name: "Assembly Proof Scratch Base",
        description: "Absolute scratch base for the assembly proof's plain clone, on the checkout filesystem outside system temporary roots, TMPDIR and every .cas ancestor (for example /home/cas-release-gate/base). The daemon passes it as CAS_RELEASE_GATE_HOME_DIR. Empty reports NOT CONFIGURED without running or attributing a failed suite.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "release", "assembly", "scratch", "clone", "integration"],
        use_cases: &["Configure automatic assembly sweeps without daemon environment variables"],
    });

    registry.register(ConfigMeta {
        key: "factory.epic_base_branch",
        section: "factory",
        name: "Epic Base Branch",
        description: "Integration branch epics and workers are cut from, and the work target a task without one defaults to. Empty uses the repository's detected default branch (origin/HEAD, then init.defaultBranch).",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["factory", "epic", "base", "branch", "trunk", "staging", "target", "integration"],
        use_cases: &[
            "Cut epics and workers from staging in a staging-first repository",
            "Leave empty to use the repository's default branch",
        ],
    });

    registry.register(ConfigMeta {
        key: "factory.merge_sweep_cwd",
        section: "factory",
        name: "Post-Merge Sweep Working Directory",
        description: "Directory relative to the merged-tip checkout for the sweep command or detected runner. Empty uses automatic runner discovery.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "merge", "sweep", "cwd", "directory", "test"],
        use_cases: &["Run a web app suite from web/", "Select one runner in a multi-app repository"],
    });

    registry.register(ConfigMeta {
        key: "factory.merge_sweep_timeout_secs",
        section: "factory",
        name: "Post-Merge Sweep Timeout",
        description: "Maximum wall-clock seconds allowed for one post-merge cargo nextest workspace sweep before it is terminated and reported.",
        value_type: ConfigType::Int,
        default: "1800",
        constraint: Constraint::Range(1, 86_400),
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "merge", "sweep", "timeout", "nextest"],
        use_cases: &["Bound validation on a shared host", "Allow a larger workspace more time to finish"],
    });

    registry.register(ConfigMeta {
        key: "factory.merge_sweep_quiet_secs",
        section: "factory",
        name: "Post-Merge Sweep Quiet Period",
        description: "Seconds with no further epic merge before the rolling integration sweep starts. A burst of merges runs one sweep of the newest tip; each merge still cancels a stale running sweep. 0 starts immediately.",
        value_type: ConfigType::Int,
        default: "120",
        constraint: Constraint::Range(0, 3_600),
        advanced: true,
        requires_feature: None,
        keywords: &["factory", "merge", "sweep", "debounce", "quiet", "proof"],
        use_cases: &["Leave QA browser runs host memory during merge bursts", "Start every sweep immediately with 0"],
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
            description: "Agent coordination mode. 'local' for standalone operation, 'cloud' for multi-device sync via Cassy Cloud.",
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
            description: "URL of the Cassy Cloud server for cloud coordination mode. Only used when coordination.mode is 'cloud'.",
            value_type: ConfigType::String,
            default: "",
            constraint: Constraint::None,
            advanced: true,
            requires_feature: None,
            keywords: &["cloud", "url", "server", "endpoint", "api"],
            use_cases: &[
                "Set to your Cassy Cloud instance URL",
                "Leave empty to use default Cassy Cloud",
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
            description: "Enable anonymous usage telemetry to help improve Cassy. Opt-in via CAS_TELEMETRY=1 or this setting. No personal or code data is collected.",
            value_type: ConfigType::Bool,
            default: "false",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["telemetry", "analytics", "usage", "metrics", "anonymous"],
            use_cases: &[
                "Disable for complete privacy",
                "Enable to help improve Cassy with anonymous usage data",
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
        description: "Show notification when Cassy Cloud authentication succeeds.",
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
