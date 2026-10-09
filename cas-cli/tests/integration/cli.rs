//! Cli integration suites, linked once.
//! Run with nextest for one-process-per-test environment/cwd isolation.

#[path = "../artifact_publish_test.rs"]
mod artifact_publish_test;
#[path = "../bridge_server_sse_test.rs"]
mod bridge_server_sse_test;
#[path = "../bridge_server_test.rs"]
mod bridge_server_test;
#[path = "../build_script_worktree_test.rs"]
mod build_script_worktree_test;
#[path = "../claude_profile_test.rs"]
mod claude_profile_test;
#[path = "../cli_test.rs"]
mod cli_test;
#[path = "../codex_profile_test.rs"]
mod codex_profile_test;
#[path = "../gh701_origin_filter_measurement_test.rs"]
mod gh701_origin_filter_measurement_test;
#[path = "../host_registry_isolation_test.rs"]
mod host_registry_isolation_test;
#[path = "../hub_clean_home_test.rs"]
mod hub_clean_home_test;
#[path = "../hub_detached_lifecycle_test.rs"]
mod hub_detached_lifecycle_test;
#[path = "../hub_launcher_path_test.rs"]
mod hub_launcher_path_test;
#[path = "../init_non_project_guard_test.rs"]
mod init_non_project_guard_test;
#[path = "../init_store_repair_test.rs"]
mod init_store_repair_test;
#[path = "../init_watchdog_budget_test.rs"]
mod init_watchdog_budget_test;
#[path = "../integrate_lifecycle_test.rs"]
mod integrate_lifecycle_test;
#[path = "../jail_guard_test.rs"]
mod jail_guard_test;
#[path = "../worker_isolation_hook_test.rs"]
mod worker_isolation_hook_test;
#[path = "../mcp_protocol_test.rs"]
mod mcp_protocol_test;
#[path = "../mcp_proxy_test.rs"]
mod mcp_proxy_test;
#[path = "../openclaw_bridge_test.rs"]
mod openclaw_bridge_test;
#[path = "../provider_shortcuts_test.rs"]
mod provider_shortcuts_test;
#[path = "../real_store_isolation_test.rs"]
mod real_store_isolation_test;
#[path = "../release_report_test.rs"]
mod release_report_test;
#[path = "../serve_parent_watchdog_test.rs"]
mod serve_parent_watchdog_test;
#[path = "../session_start_issue_triage_test.rs"]
mod session_start_issue_triage_test;
#[path = "../session_start_memory_hygiene_test.rs"]
mod session_start_memory_hygiene_test;
#[path = "../setup_test.rs"]
mod setup_test;
#[path = "../update_all_projects_test.rs"]
mod update_all_projects_test;
#[path = "../update_sync_report_attribution_test.rs"]
mod update_sync_report_attribution_test;
#[path = "../viktor_distribution_test.rs"]
mod viktor_distribution_test;
#[path = "../viktor_key_setup_test.rs"]
mod viktor_key_setup_test;
#[path = "../violet_cli_test.rs"]
mod violet_cli_test;
#[path = "../violet_json_test.rs"]
mod violet_json_test;
