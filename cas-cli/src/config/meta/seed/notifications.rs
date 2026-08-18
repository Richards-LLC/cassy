use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_notifications(registry: &mut ConfigRegistry) {
    // NOTIFICATIONS SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "notifications.enabled",
            section: "notifications",
            name: "Enable Notifications",
            description: "Enable TUI notifications for Cassy events like task creation, completion, and memory additions.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["notifications", "alerts", "tui", "events", "popup"],
            use_cases: &[
                "Disable for distraction-free mode",
                "Enable to stay informed of Cassy activity",
            ],
        });

    registry.register(ConfigMeta {
        key: "notifications.sound_enabled",
        section: "notifications",
        name: "Sound Enabled",
        description: "Play terminal bell sound when notifications appear.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["sound", "bell", "audio", "alert", "beep"],
        use_cases: &["Disable for silent operation", "Enable for audio alerts"],
    });

    registry.register(ConfigMeta {
        key: "notifications.display_duration_secs",
        section: "notifications",
        name: "Display Duration",
        description: "How long to display notifications in seconds before auto-dismiss.",
        value_type: ConfigType::Int,
        default: "5",
        constraint: Constraint::Range(1, 60),
        advanced: false,
        requires_feature: None,
        keywords: &["duration", "timeout", "dismiss", "seconds", "display"],
        use_cases: &[
            "Increase to read notifications longer",
            "Decrease for less intrusive notifications",
        ],
    });

    registry.register(ConfigMeta {
        key: "notifications.max_visible",
        section: "notifications",
        name: "Max Visible",
        description: "Maximum number of notifications to display at once.",
        value_type: ConfigType::Int,
        default: "3",
        constraint: Constraint::Range(1, 10),
        advanced: false,
        requires_feature: None,
        keywords: &["limit", "visible", "stack", "queue", "count"],
        use_cases: &[
            "Increase to see more notifications at once",
            "Decrease to reduce screen clutter",
        ],
    });

    // ============================================================
    // NOTIFICATIONS.TASKS SECTION
    // ============================================================
    registry.register(ConfigMeta {
        key: "notifications.tasks.on_created",
        section: "notifications.tasks",
        name: "Task Created",
        description: "Show notification when a task is created.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["task", "created", "new", "notification"],
        use_cases: &["Disable if task creation is too noisy"],
    });

    registry.register(ConfigMeta {
        key: "notifications.tasks.on_started",
        section: "notifications.tasks",
        name: "Task Started",
        description: "Show notification when a task is started.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["task", "started", "begin", "in_progress"],
        use_cases: &["Disable if task starts are too frequent"],
    });

    registry.register(ConfigMeta {
        key: "notifications.tasks.on_closed",
        section: "notifications.tasks",
        name: "Task Closed",
        description: "Show notification when a task is closed/completed.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["task", "closed", "completed", "done", "finished"],
        use_cases: &["Enable to celebrate task completions"],
    });

    registry.register(ConfigMeta {
        key: "notifications.tasks.on_updated",
        section: "notifications.tasks",
        name: "Task Updated",
        description:
            "Show notification when a task is updated. Disabled by default as it can be noisy.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["task", "updated", "modified", "changed"],
        use_cases: &["Enable to track all task changes"],
    });

    // ============================================================
    // NOTIFICATIONS.ENTRIES SECTION
    // ============================================================
    registry.register(ConfigMeta {
        key: "notifications.entries.on_added",
        section: "notifications.entries",
        name: "Entry Added",
        description: "Show notification when a memory entry is added.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["entry", "memory", "added", "new", "remember"],
        use_cases: &["Enable to track memory additions"],
    });

    registry.register(ConfigMeta {
        key: "notifications.entries.on_updated",
        section: "notifications.entries",
        name: "Entry Updated",
        description: "Show notification when a memory entry is updated. Disabled by default.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["entry", "memory", "updated", "modified"],
        use_cases: &["Enable to track memory modifications"],
    });

    registry.register(ConfigMeta {
        key: "notifications.entries.on_deleted",
        section: "notifications.entries",
        name: "Entry Deleted",
        description: "Show notification when a memory entry is deleted.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["entry", "memory", "deleted", "removed"],
        use_cases: &["Enable to track memory deletions"],
    });

    // ============================================================
    // NOTIFICATIONS.RULES SECTION
    // ============================================================
    registry.register(ConfigMeta {
        key: "notifications.rules.on_created",
        section: "notifications.rules",
        name: "Rule Created",
        description: "Show notification when a rule is created.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["rule", "created", "new", "draft"],
        use_cases: &["Enable to track new rules"],
    });

    registry.register(ConfigMeta {
        key: "notifications.rules.on_promoted",
        section: "notifications.rules",
        name: "Rule Promoted",
        description: "Show notification when a rule is promoted to Proven status.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["rule", "promoted", "proven", "validated"],
        use_cases: &["Enable to celebrate rule promotions"],
    });

    registry.register(ConfigMeta {
        key: "notifications.rules.on_demoted",
        section: "notifications.rules",
        name: "Rule Demoted",
        description: "Show notification when a rule is demoted. Disabled by default.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["rule", "demoted", "stale", "harmful"],
        use_cases: &["Enable to track rule demotions"],
    });

    // ============================================================
    // NOTIFICATIONS.SKILLS SECTION
    // ============================================================
    registry.register(ConfigMeta {
        key: "notifications.skills.on_created",
        section: "notifications.skills",
        name: "Skill Created",
        description: "Show notification when a skill is created.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["skill", "created", "new"],
        use_cases: &["Enable to track new skills"],
    });

    registry.register(ConfigMeta {
        key: "notifications.skills.on_enabled",
        section: "notifications.skills",
        name: "Skill Enabled",
        description: "Show notification when a skill is enabled.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["skill", "enabled", "activated"],
        use_cases: &["Enable to track skill activations"],
    });

    registry.register(ConfigMeta {
        key: "notifications.skills.on_disabled",
        section: "notifications.skills",
        name: "Skill Disabled",
        description: "Show notification when a skill is disabled. Disabled by default.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["skill", "disabled", "deactivated"],
        use_cases: &["Enable to track skill deactivations"],
    });

    // ============================================================
}
