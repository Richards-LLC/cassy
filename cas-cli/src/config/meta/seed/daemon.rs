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

    registry.register(ConfigMeta {
        key: "daemon.relevance_sampling_enabled",
        section: "daemon",
        name: "Injected Relevance Sampling",
        description: "Schedule the weekly bounded relevance pass over recent injected retrieval results. This setting does not configure a receiving-agent or model judge; without one, precision remains honestly unavailable.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["daemon", "evaluation", "retrieval", "relevance", "judge", "sampling"],
        use_cases: &["Collect rolling relevance labels for injected memory packets"],
    });

    registry.register(ConfigMeta {
        key: "daemon.relevance_sampling_interval_secs",
        section: "daemon",
        name: "Relevance Sampling Cadence",
        description: "Minimum interval between injected-relevance sampling passes, in seconds. The default is one week.",
        value_type: ConfigType::Int,
        default: "604800",
        constraint: Constraint::Min(1),
        advanced: true,
        requires_feature: None,
        keywords: &["daemon", "evaluation", "retrieval", "relevance", "cadence"],
        use_cases: &["Run the judge more often in a high-volume evaluation environment"],
    });

    registry.register(ConfigMeta {
        key: "daemon.relevance_sampling_sample_size",
        section: "daemon",
        name: "Relevance Sampling Size",
        description: "Maximum injected result rows offered to the judge per pass. The default is 20.",
        value_type: ConfigType::Int,
        default: "20",
        constraint: Constraint::Min(1),
        advanced: true,
        requires_feature: None,
        keywords: &["daemon", "evaluation", "retrieval", "relevance", "sample", "size"],
        use_cases: &["Bound judge work while still measuring a representative rolling sample"],
    });
}
