//! Cloud integration suites, linked once.
//! Run with nextest for one-process-per-test environment/cwd isolation.

#[path = "../active_team_id_integration_test.rs"]
mod active_team_id_integration_test;
#[path = "../auth_integration_test.rs"]
mod auth_integration_test;
#[path = "../blame_attribution_test.rs"]
mod blame_attribution_test;
#[path = "../cloud_login_scope_test.rs"]
mod cloud_login_scope_test;
#[path = "../history_search_production_path_test.rs"]
mod history_search_production_path_test;
#[path = "../knowledge_distillation_test.rs"]
mod knowledge_distillation_test;
#[path = "../known_repos_binding_test.rs"]
mod known_repos_binding_test;
#[path = "../memory_migration_test.rs"]
mod memory_migration_test;
#[path = "../memory_share_test.rs"]
mod memory_share_test;
#[path = "../project_pull_archived_test.rs"]
mod project_pull_archived_test;
#[path = "../provenance.rs"]
mod provenance;
#[path = "../provenance_loop.rs"]
mod provenance_loop;
#[path = "../pull_authorship_test.rs"]
mod pull_authorship_test;
#[path = "../pull_no_reenqueue_test.rs"]
mod pull_no_reenqueue_test;
#[path = "../pull_scoping_regression_test.rs"]
mod pull_scoping_regression_test;
#[path = "../pull_watermark_recovery_test.rs"]
mod pull_watermark_recovery_test;
#[path = "../push_queue_scoping_test.rs"]
mod push_queue_scoping_test;
#[path = "../push_rehome_guard_test.rs"]
mod push_rehome_guard_test;
#[path = "../push_skipped_test.rs"]
mod push_skipped_test;
#[path = "../retrieval_eval_test.rs"]
mod retrieval_eval_test;
#[path = "../retrieval_parity_test.rs"]
mod retrieval_parity_test;
#[path = "../search_frontmatter_test.rs"]
mod search_frontmatter_test;
#[path = "../search_scoring_test.rs"]
mod search_scoring_test;
#[path = "../search_utf8_regression_test.rs"]
mod search_utf8_regression_test;
#[path = "../team_backfill_test.rs"]
mod team_backfill_test;
#[path = "../team_default_test.rs"]
mod team_default_test;
#[path = "../team_memories_e2e_test.rs"]
mod team_memories_e2e_test;
#[path = "../team_pull_lww_test.rs"]
mod team_pull_lww_test;
#[path = "../team_pull_watermark_scope_test.rs"]
mod team_pull_watermark_scope_test;
#[path = "../team_pull_wiring_test.rs"]
mod team_pull_wiring_test;
#[path = "../team_registration_test.rs"]
mod team_registration_test;
#[path = "../team_scope_e2e_test.rs"]
mod team_scope_e2e_test;
#[path = "../team_set_slug_resolution_test.rs"]
mod team_set_slug_resolution_test;
#[path = "../team_sync_test.rs"]
mod team_sync_test;
#[path = "../teams_fetch_test.rs"]
mod teams_fetch_test;
