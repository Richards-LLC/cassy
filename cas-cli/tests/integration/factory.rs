//! Factory integration suites, linked once.
//! Run with nextest for one-process-per-test environment/cwd isolation.

#[path = "../delivery_target_cas_test.rs"]
mod delivery_target_cas_test;
#[path = "../distributed_factory_test.rs"]
mod distributed_factory_test;
#[path = "../factory_latency_test.rs"]
mod factory_latency_test;
#[path = "../factory_mcp_ops_test.rs"]
mod factory_mcp_ops_test;
#[path = "../factory_parity_test.rs"]
mod factory_parity_test;
#[path = "../factory_preflight_test.rs"]
mod factory_preflight_test;
#[path = "../factory_probe_comm_test.rs"]
mod factory_probe_comm_test;
#[path = "../factory_server_test.rs"]
mod factory_server_test;
#[path = "../loop_test.rs"]
mod loop_test;
#[path = "../multi_agent_test.rs"]
mod multi_agent_test;
#[path = "../server_registry_mcp_test.rs"]
mod server_registry_mcp_test;
#[path = "../service_tools_test.rs"]
mod service_tools_test;
#[path = "../task_update_verification_type_test.rs"]
mod task_update_verification_type_test;
#[path = "../task_update_work_target_close_test.rs"]
mod task_update_work_target_close_test;
#[path = "../verification_test.rs"]
mod verification_test;
#[path = "../verification_timeout_atomicity_test.rs"]
mod verification_timeout_atomicity_test;
#[path = "../verifier_handoff_cleanup_test.rs"]
mod verifier_handoff_cleanup_test;
#[path = "../worker_hold_mcp_test.rs"]
mod worker_hold_mcp_test;
#[path = "../worktree_surface_test.rs"]
mod worktree_surface_test;
#[path = "../worktree_test.rs"]
mod worktree_test;
