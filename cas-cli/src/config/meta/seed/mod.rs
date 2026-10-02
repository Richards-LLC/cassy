use crate::config::meta::registry::ConfigRegistry;

mod coordination;
mod daemon;
mod history;
mod hooks_and_code;
mod issues;
mod jev;
mod llm;
mod memory;
mod notifications;
mod qa;
mod release;
mod sections;
mod skill_validation;
mod skills;
mod slack;

pub(crate) fn populate_registry(registry: &mut ConfigRegistry) {
    sections::add_section_descriptions(registry);
    hooks_and_code::register_hooks_and_code(registry);
    daemon::register_daemon(registry);
    history::register_history(registry);
    issues::register_issues(registry);
    notifications::register_notifications(registry);
    qa::register_qa(registry);
    coordination::register_coordination_lease_telemetry_and_missing(registry);
    llm::register_llm(registry);
    jev::register_jev(registry);
    memory::register_memory(registry);
    release::register_release(registry);
    skill_validation::register_skill_validation(registry);
    skills::register_skills(registry);
    slack::register_slack(registry);
}
