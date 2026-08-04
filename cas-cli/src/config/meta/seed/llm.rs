use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_llm(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "llm.harness",
        section: "llm",
        name: "Harness",
        description: "Which CLI harness to use for spawning agents. 'claude' uses Claude Code CLI, 'codex' uses Codex CLI. If not set, supervisor/unknown roles default to 'claude'; workers default to the stock floor 'codex' (see llm.worker.harness).",
        value_type: ConfigType::String,
        default: "(default)",
        constraint: Constraint::OneOf(vec!["claude".to_string(), "codex".to_string()]),
        advanced: false,
        requires_feature: None,
        keywords: &["llm", "harness", "cli", "claude", "codex", "agent", "binary"],
        use_cases: &[
            "Set to 'claude' for Claude Code agents",
            "Set to 'codex' for OpenAI Codex agents",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.model",
        section: "llm",
        name: "Model",
        description: "Default model to use within the harness. Examples: 'claude-sonnet-4-5-20250929', 'gpt-5.3-codex'. If not set, the harness uses its built-in default.",
        value_type: ConfigType::String,
        default: "(default)",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["llm", "model", "claude", "gpt", "sonnet", "opus", "codex"],
        use_cases: &[
            "Set a specific model for all agents",
            "Override the harness default model",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.reasoning_effort",
        section: "llm",
        name: "Reasoning Effort",
        description: "Reasoning effort level. Supported by some models to control thinking depth. Options: 'low', 'medium', 'high'.",
        value_type: ConfigType::String,
        default: "(default)",
        constraint: Constraint::OneOf(vec!["low".to_string(), "medium".to_string(), "high".to_string(), "xhigh".to_string(), "max".to_string()]),
        advanced: true,
        requires_feature: None,
        keywords: &["llm", "reasoning", "effort", "thinking", "depth"],
        use_cases: &[
            "Set to 'high' for complex tasks requiring deep reasoning",
            "Set to 'low' for simple tasks to save cost/time",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.supervisor.harness",
        section: "llm",
        name: "Supervisor Harness",
        description: "Override harness for supervisor agents. Falls back to llm.harness if not set.",
        value_type: ConfigType::String,
        default: "(inherit)",
        constraint: Constraint::OneOf(vec!["claude".to_string(), "codex".to_string()]),
        advanced: false,
        requires_feature: None,
        keywords: &["llm", "supervisor", "harness", "cli", "override"],
        use_cases: &[
            "Use a different CLI for the supervisor than workers",
            "Run supervisor on Claude while workers use Codex",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.supervisor.model",
        section: "llm",
        name: "Supervisor Model",
        description: "Override model for supervisor agents. Falls back to llm.model if not set.",
        value_type: ConfigType::String,
        default: "(inherit)",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["llm", "supervisor", "model", "override"],
        use_cases: &[
            "Use a more capable model for the supervisor",
            "Use Opus for supervisor, Sonnet for workers",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.supervisor.reasoning_effort",
        section: "llm",
        name: "Supervisor Reasoning Effort",
        description: "Override reasoning effort for supervisor agents. Falls back to llm.reasoning_effort if not set.",
        value_type: ConfigType::String,
        default: "(inherit)",
        constraint: Constraint::OneOf(vec!["low".to_string(), "medium".to_string(), "high".to_string(), "xhigh".to_string(), "max".to_string()]),
        advanced: true,
        requires_feature: None,
        keywords: &["llm", "supervisor", "reasoning", "effort", "override"],
        use_cases: &[
            "Set higher reasoning for supervisor planning tasks",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.worker.harness",
        section: "llm",
        name: "Worker Harness",
        description: "Override harness for worker agents. Fallback chain: [llm.worker.harness] → [llm.harness] → stock worker default (codex).",
        value_type: ConfigType::String,
        default: "(inherit)",
        constraint: Constraint::OneOf(vec!["claude".to_string(), "codex".to_string()]),
        advanced: false,
        requires_feature: None,
        keywords: &["llm", "worker", "harness", "cli", "override"],
        use_cases: &["Use a different CLI for workers than the supervisor"],
    });

    // cas-05e3: `default:` stays as `(inherit)` (the literal value of the
    // serialised config when no override is set) so `cas config diff` does
    // not flag every fresh install as "modified". The new stock-default
    // behaviour is surfaced via `description:` instead — `cas config
    // describe llm.worker.model` shows the full fallback chain.
    registry.register(ConfigMeta {
        key: "llm.worker.model",
        section: "llm",
        name: "Worker Model",
        description: "Override model for worker agents. Fallback chain: [llm.worker.model] → [llm.model] → stock worker default (gpt-5.6-terra).",
        value_type: ConfigType::String,
        default: "(inherit)",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["llm", "worker", "model", "override"],
        use_cases: &[
            "Use a cost-effective model for workers",
            "Use a custom model for workers while preserving the supervisor model",
        ],
    });

    registry.register(ConfigMeta {
        key: "llm.worker.reasoning_effort",
        section: "llm",
        name: "Worker Reasoning Effort",
        description: "Override reasoning effort for worker agents. Fallback chain: [llm.worker.reasoning_effort] → [llm.reasoning_effort] → stock worker default (medium).",
        value_type: ConfigType::String,
        default: "(inherit)",
        constraint: Constraint::OneOf(vec!["low".to_string(), "medium".to_string(), "high".to_string(), "xhigh".to_string(), "max".to_string()]),
        advanced: true,
        requires_feature: None,
        keywords: &["llm", "worker", "reasoning", "effort", "override"],
        use_cases: &[
            "Set lower reasoning for routine worker tasks",
        ],
    });
}
