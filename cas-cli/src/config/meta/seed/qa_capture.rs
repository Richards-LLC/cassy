use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_capture(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "qa.capture.base_url", section: "qa.capture", name: "Capture: Base URL",
        description: "HTTP(S) capture target without embedded credentials, query or fragment; it or serve enables capture.",
        value_type: ConfigType::String, default: "", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.serve", section: "qa.capture", name: "Capture: Serve Command",
        description: "Command serving a checkout with a {port} placeholder; it or base_url enables capture.",
        value_type: ConfigType::String, default: "", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.ready_path",
        section: "qa.capture",
        name: "Capture: Ready Path",
        description: "Local readiness path for the capture server.",
        value_type: ConfigType::String,
        default: "/",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.shots", section: "qa.capture", name: "Capture: Shot Catalog",
        description: "Project-relative capture catalog; absent catalog uses affected journeys final screens.",
        value_type: ConfigType::String, default: "docs/qa/shots.json", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.credentials_env", section: "qa.capture", name: "Capture: Credentials Environment",
        description: "Environment-variable NAME pointing to an existing readable QA credentials file at capture time; never a secret or resolved path.",
        value_type: ConfigType::String, default: "", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.login_path",
        section: "qa.capture",
        name: "Capture: Login Path",
        description: "Local login path; empty captures public surfaces without login.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.success_url", section: "qa.capture", name: "Capture: Login Success URL",
        description: "Signed-in URL regular expression anchored with ^; required by capture when login is enabled.",
        value_type: ConfigType::String, default: "", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.email_sel",
        section: "qa.capture",
        name: "Capture: Email Selector",
        description: "Login email input CSS selector.",
        value_type: ConfigType::String,
        default: "input[type=email]",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.password_sel",
        section: "qa.capture",
        name: "Capture: Password Selector",
        description: "Login password input CSS selector.",
        value_type: ConfigType::String,
        default: "input[type=password]",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.submit_sel",
        section: "qa.capture",
        name: "Capture: Submit Selector",
        description: "Login submit CSS selector.",
        value_type: ConfigType::String,
        default: "button[type=submit]",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.mask",
        section: "qa.capture",
        name: "Capture: PII Masks",
        description: "Global PII CSS selectors, comma-separated, added to each shot mask.",
        value_type: ConfigType::StringList,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.viewports", section: "qa.capture", name: "Capture: Viewports",
        description: "Capture viewports, comma-separated: 1280x800 light by default; 390x844 only for shots requesting mobile.",
        value_type: ConfigType::StringList, default: "1280x800", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.max_shots",
        section: "qa.capture",
        name: "Capture: Shot Limit",
        description: "Maximum shots per delivery.",
        value_type: ConfigType::Int,
        default: "6",
        constraint: Constraint::Range(1, 6),
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.timeout_secs", section: "qa.capture", name: "Capture: Capture Time Limit",
        description: "Seconds for the base build and both captures; timeout yields unavailable with a reason.",
        value_type: ConfigType::Int, default: "240", constraint: Constraint::Range(1, 240),
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
    registry.register(ConfigMeta {
        key: "qa.capture.max_html_mb", section: "qa.capture", name: "Capture: Capture HTML Ceiling",
        description: "Capture-side HTML size ceiling in MB; report embedding budgets belong to finalize.",
        value_type: ConfigType::Int, default: "8", constraint: Constraint::Range(1, 8),
        advanced: false, requires_feature: None,
        keywords: &["qa", "capture", "pairs", "deliverable"],
        use_cases: &["Configure project before/after evidence and bounded report assets"],
    });
}
