use crate::config::meta::registry::ConfigRegistry;

mod coordination;
mod daemon;
mod hooks_and_code;
mod issues;
mod llm;
mod notifications;
mod sections;

pub(crate) fn populate_registry(registry: &mut ConfigRegistry) {
    sections::add_section_descriptions(registry);
    hooks_and_code::register_hooks_and_code(registry);
    daemon::register_daemon(registry);
    issues::register_issues(registry);
    notifications::register_notifications(registry);
    coordination::register_coordination_lease_telemetry_and_missing(registry);
    llm::register_llm(registry);
}
