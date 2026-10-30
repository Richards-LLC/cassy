use crate::config::meta::registry::ConfigRegistry;

mod accounts;
mod coordination;
mod daemon;
mod history;
mod hub;
mod hooks_and_code;
mod issues;
mod llm;
mod memory;
mod notifications;
mod qa;
mod qa_capture;
mod release;
mod secrets;
mod sections;
mod skill_validation;
mod skills;
mod peers;
mod slack;
mod viktor;

pub(crate) fn populate_registry(registry: &mut ConfigRegistry) {
    sections::add_section_descriptions(registry);
    accounts::register_accounts(registry);
    hooks_and_code::register_hooks_and_code(registry);
    daemon::register_daemon(registry);
    history::register_history(registry);
    issues::register_issues(registry);
    notifications::register_notifications(registry);
    qa::register_qa(registry);
    qa_capture::register_capture(registry);
    coordination::register_coordination_lease_telemetry_and_missing(registry);
    llm::register_llm(registry);
    memory::register_memory(registry);
    release::register_release(registry);
    skill_validation::register_skill_validation(registry);
    skills::register_skills(registry);
    peers::register_peers(registry);
    slack::register_slack(registry);
    hub::register_hub(registry);
    secrets::register_secrets(registry);
    viktor::register_viktor(registry);
}
