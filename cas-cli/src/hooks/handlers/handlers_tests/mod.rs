mod agent_worktree_block;
mod ask_user_question_remind;
mod basic;
mod factory_auto_approve;
mod factory_inbox_surfacing;
mod formatter_scope_guard;
mod message_display;
mod neon_sql_guard;
mod permission_request_factory;
mod preferences_context;
mod reload_skills;
mod reviews;
mod ripple_path_scope;
mod send_message_autoroute;
mod session_title;
mod slack_transport;
mod stop_hook_active;
mod supervisor_reminder;
mod tmpfs_guardrail;
mod unscoped_test_guard;

use crate::test_support::TestEnvGuard;

/// Reuse the caller's guard so role and harness overrides share one lock and
/// are restored together, including if a handler assertion unwinds.
fn set_role_env(env: &mut TestEnvGuard, role: Option<&str>) {
    match role {
        Some(role) => env.set("CAS_AGENT_ROLE", role),
        None => env.remove("CAS_AGENT_ROLE"),
    }
}

fn set_supervisor_cli_env(env: &mut TestEnvGuard, cli: Option<&str>) {
    match cli {
        Some(cli) => env.set("CAS_FACTORY_SUPERVISOR_CLI", cli),
        None => env.remove("CAS_FACTORY_SUPERVISOR_CLI"),
    }
}
