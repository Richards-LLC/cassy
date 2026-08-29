use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_daemon(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "daemon.archive_retention_days",
        section: "daemon",
        name: "Trace Archive Retention",
        description: "Days to retain compressed event and recording archives. Zero keeps archives forever.",
        value_type: ConfigType::Int,
        default: "0",
        constraint: Constraint::Min(0),
        advanced: true,
        requires_feature: None,
        keywords: &["daemon", "archive", "retention", "events", "recordings", "storage"],
        use_cases: &[
            "Keep the default zero for append-only archives",
            "Set a positive value to reclaim old archive files",
        ],
    });
}
