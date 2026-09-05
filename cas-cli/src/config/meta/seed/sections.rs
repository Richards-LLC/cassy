use crate::config::meta::registry::ConfigRegistry;

pub(super) fn add_section_descriptions(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("sync", "Rule synchronization to .claude/rules/");
    registry
        .section_descriptions
        .insert("cloud", "Cloud sync configuration");
    registry
        .section_descriptions
        .insert("hooks", "Claude Code hook behavior");
    registry
        .section_descriptions
        .insert("hooks.stop", "Stop hook behavior");
    registry
        .section_descriptions
        .insert("hooks.plan_mode", "Plan mode specific settings");
    registry
        .section_descriptions
        .insert("tasks", "Task management settings");
    registry.section_descriptions.insert(
        "issues",
        "GitHub repository routing for Cassy-system bug reports",
    );
    registry.section_descriptions.insert(
        "issues.components",
        "Issue repositories for each Cassy component",
    );
    registry
        .section_descriptions
        .insert("dev", "Development and tracing options");
    registry.section_descriptions.insert(
        "release",
        "Release-note routing policy, including approved Claude accounts",
    );
    registry
        .section_descriptions
        .insert("daemon", "Background maintenance and trace archives");
    registry
        .section_descriptions
        .insert("memory", "Memory lifecycle and automatic learning settings");
    registry.section_descriptions.insert(
        "memory.decay",
        "Curated-memory decay floors and access promotion",
    );
    registry
        .section_descriptions
        .insert("code", "Background code indexing for semantic search");
    registry
        .section_descriptions
        .insert("notifications", "TUI notification settings");
    registry
        .section_descriptions
        .insert("notifications.tasks", "Task notification events");
    registry
        .section_descriptions
        .insert("notifications.entries", "Entry/memory notification events");
    registry
        .section_descriptions
        .insert("notifications.rules", "Rule notification events");
    registry
        .section_descriptions
        .insert("notifications.skills", "Skill notification events");
    registry
        .section_descriptions
        .insert("coordination", "Agent coordination mode settings");
    registry
        .section_descriptions
        .insert("lease", "Task lease management settings");
    registry
        .section_descriptions
        .insert("telemetry", "Telemetry and analytics settings");
    registry.section_descriptions.insert(
        "llm",
        "LLM harness and model configuration for factory agents",
    );
    registry.section_descriptions.insert(
        "staging",
        "Durable staging paths, agent scratch roots, and tmpfs write guardrails",
    );
    registry.section_descriptions.insert(
        "factory",
        "Factory worker lifecycle, durable artifacts, and workspace guardrails",
    );
    registry.section_descriptions.insert(
        "skill_validation",
        "Sandbox policy for skill validation scripts",
    );
    registry.section_descriptions.insert(
        "skills",
        "Optional stack-specific builtin skills for this project",
    );
}
