use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_fast_lane(registry: &mut ConfigRegistry) {
    const KEYWORDS: &[&str] = &["fast lane", "hotfix", "merge", "receipt", "frontend"];
    const USE_CASES: &[&str] = &["Merge a small frontend-only hotfix on its unit tests and typecheck"];
    let entries: [(&'static str, &'static str, &'static str, ConfigType, &'static str, Constraint); 9] = [
        ("fast_lane.enabled", "Fast lane enabled",
         "Opt this project into the hotfix fast lane: an eligible delivery merges on merge_checks alone and owes a post-merge receipt before close.",
         ConfigType::Bool, "false", Constraint::None),
        ("fast_lane.paths", "Fast lane paths",
         "Globs every changed path must match, e.g. apps/frontend/app/src/**,apps/frontend/locales/**.",
         ConfigType::StringList, "", Constraint::None),
        ("fast_lane.deny", "Fast lane deny list",
         "Globs no changed path may match: API routes, middleware, auth, billing, dependency and deployment config.",
         ConfigType::StringList, "", Constraint::None),
        ("fast_lane.max_files", "Fast lane file cap",
         "Most changed files an eligible delivery may have.",
         ConfigType::Int, "10", Constraint::Min(1)),
        ("fast_lane.max_lines", "Fast lane line cap",
         "Most changed lines (added plus deleted) an eligible delivery may have.",
         ConfigType::Int, "300", Constraint::Min(1)),
        ("fast_lane.merge_checks", "Fast lane merge checks",
         "Exact GitHub check-run names (Actions job names) that alone admit an eligible merge.",
         ConfigType::StringList, "", Constraint::None),
        ("fast_lane.receipt_checks", "Fast lane receipt checks",
         "Exact check-run names that must pass on the merge commit before the task closes, e.g. the hermetic e2e job on push to main.",
         ConfigType::StringList, "", Constraint::None),
        ("fast_lane.receipt_statuses", "Fast lane receipt statuses",
         "Commit-status contexts that must succeed on the merge commit, e.g. Vercel for the production build.",
         ConfigType::StringList, "", Constraint::None),
        ("fast_lane.receipt_window_mins", "Fast lane receipt window",
         "Minutes after merge the post-merge receipt is due.",
         ConfigType::Int, "30", Constraint::Min(1)),
    ];
    for (key, name, description, value_type, default, constraint) in entries {
        registry.register(ConfigMeta {
            key, section: "fast_lane", name, description, value_type, default, constraint,
            advanced: true, requires_feature: None, keywords: KEYWORDS, use_cases: USE_CASES,
        });
    }
}
