use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_daemon(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "daemon.archive_max_bytes",
        section: "daemon",
        name: "Trace Archive Size Cap",
        description: "Maximum total bytes for compressed event and recording archives. Oldest files are evicted first; the default is 1 GiB. Set a positive value; zero is rejected.",
        value_type: ConfigType::Int,
        default: "1073741824",
        constraint: Constraint::Min(1),
        advanced: true,
        requires_feature: None,
        keywords: &["daemon", "archive", "size", "cap", "events", "recordings", "storage"],
        use_cases: &[
            "Bound trace archive disk usage with a finite compressed-byte cap",
            "Raise the cap when Wiki-Maintainer sampling needs a longer history",
        ],
    });

    registry.register(ConfigMeta {
        key: "daemon.archive_retention_days",
        section: "daemon",
        name: "Legacy Trace Archive Age",
        description: "Legacy compatibility setting; archive retention is now bounded by daemon.archive_max_bytes and oldest files are evicted first.",
        value_type: ConfigType::Int,
        default: "0",
        constraint: Constraint::Min(0),
        advanced: true,
        requires_feature: None,
        keywords: &["daemon", "archive", "retention", "events", "recordings", "storage"],
        use_cases: &[
            "Retain this key while migrating older config files",
        ],
    });
}
