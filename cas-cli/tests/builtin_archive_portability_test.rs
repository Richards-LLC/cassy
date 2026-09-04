//! Archive portability guard for the builtin inspection tests.
//!
//! The tests in this list run from nextest archives, where the checkout is not
//! present. Their source inputs must therefore be embedded in the test binary
//! or supplied by Cassy's embedded builtin catalogs.

macro_rules! source {
    ($name:literal, $path:literal) => {
        ($name, include_str!($path))
    };
}

const BUILTIN_INSPECTION_SOURCES: &[(&str, &str)] = &[
    source!(
        "cas-cli/tests/agent_definition_contract_test.rs",
        "agent_definition_contract_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_doc_hygiene_test.rs",
        "builtin_doc_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_flavor_drift_test.rs",
        "builtin_flavor_drift_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_skill_description_test.rs",
        "builtin_skill_description_test.rs"
    ),
    source!(
        "cas-cli/tests/cas_image_generate_skill_test.rs",
        "cas_image_generate_skill_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_codex_skill_guardrails.rs",
        "factory_codex_skill_guardrails.rs"
    ),
    source!(
        "cas-cli/tests/mcp_action_surface_test.rs",
        "mcp_action_surface_test.rs"
    ),
    source!(
        "cas-cli/tests/skill_hygiene_test.rs",
        "skill_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/verify_before_claim_skill_test.rs",
        "verify_before_claim_skill_test.rs"
    ),
];

// Archive-mode has no producer checkout to enumerate at runtime. Keep every
// integration-test source embedded here, along with source files that contain
// unit-test fixtures, so this guard remains effective on another runner.
const FIXTURE_SOURCES: &[(&str, &str)] = &[
    source!(
        "cas-cli/src/mcp/tools/core/task/repo_context.rs",
        "../src/mcp/tools/core/task/repo_context.rs"
    ),
    source!(
        "cas-cli/src/store/known_repos.rs",
        "../src/store/known_repos.rs"
    ),
    source!(
        "cas-cli/src/worktree/discovery.rs",
        "../src/worktree/discovery.rs"
    ),
    source!(
        "cas-cli/src/ui/factory/app/render_and_ops/epic_workers.rs",
        "../src/ui/factory/app/render_and_ops/epic_workers.rs"
    ),
    source!(
        "cas-cli/tests/active_team_id_integration_test.rs",
        "active_team_id_integration_test.rs"
    ),
    source!(
        "cas-cli/tests/agent_definition_contract_test.rs",
        "agent_definition_contract_test.rs"
    ),
    source!(
        "cas-cli/tests/agents_md_sync_test.rs",
        "agents_md_sync_test.rs"
    ),
    source!(
        "cas-cli/tests/auth_integration_test.rs",
        "auth_integration_test.rs"
    ),
    source!(
        "cas-cli/tests/blame_attribution_test.rs",
        "blame_attribution_test.rs"
    ),
    source!(
        "cas-cli/tests/bridge_server_sse_test.rs",
        "bridge_server_sse_test.rs"
    ),
    source!(
        "cas-cli/tests/bridge_server_test.rs",
        "bridge_server_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_archive_portability_test.rs",
        "builtin_archive_portability_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_doc_hygiene_test.rs",
        "builtin_doc_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_flavor_drift_test.rs",
        "builtin_flavor_drift_test.rs"
    ),
    source!(
        "cas-cli/tests/builtin_skill_description_test.rs",
        "builtin_skill_description_test.rs"
    ),
    source!(
        "cas-cli/tests/cas_image_generate_helper_test.rs",
        "cas_image_generate_helper_test.rs"
    ),
    source!(
        "cas-cli/tests/cas_image_generate_skill_test.rs",
        "cas_image_generate_skill_test.rs"
    ),
    source!(
        "cas-cli/tests/claude_profile_test.rs",
        "claude_profile_test.rs"
    ),
    source!("cas-cli/tests/cli_test.rs", "cli_test.rs"),
    source!(
        "cas-cli/tests/cloud_login_scope_test.rs",
        "cloud_login_scope_test.rs"
    ),
    source!(
        "cas-cli/tests/codex_profile_test.rs",
        "codex_profile_test.rs"
    ),
    source!("cas-cli/tests/common/mod.rs", "common/mod.rs"),
    source!(
        "cas-cli/tests/component_output_test.rs",
        "component_output_test.rs"
    ),
    source!(
        "cas-cli/tests/delivery_target_cas_test.rs",
        "delivery_target_cas_test.rs"
    ),
    source!(
        "cas-cli/tests/distributed_factory_test.rs",
        "distributed_factory_test.rs"
    ),
    source!(
        "cas-cli/tests/distributed_factory_test_cases/tests.rs",
        "distributed_factory_test_cases/tests.rs"
    ),
    source!(
        "cas-cli/tests/e2e/agent_isolation.rs",
        "e2e/agent_isolation.rs"
    ),
    source!(
        "cas-cli/tests/e2e/factory_e2e/conformance.rs",
        "e2e/factory_e2e/conformance.rs"
    ),
    source!(
        "cas-cli/tests/e2e/factory_e2e/lifecycle.rs",
        "e2e/factory_e2e/lifecycle.rs"
    ),
    source!(
        "cas-cli/tests/e2e/factory_e2e/mod.rs",
        "e2e/factory_e2e/mod.rs"
    ),
    source!(
        "cas-cli/tests/e2e/factory_e2e/real_factory.rs",
        "e2e/factory_e2e/real_factory.rs"
    ),
    source!(
        "cas-cli/tests/e2e/factory_tui_headful.rs",
        "e2e/factory_tui_headful.rs"
    ),
    source!(
        "cas-cli/tests/e2e/hook_e2e/basic_sessions.rs",
        "e2e/hook_e2e/basic_sessions.rs"
    ),
    source!(
        "cas-cli/tests/e2e/hook_e2e/direct_hook.rs",
        "e2e/hook_e2e/direct_hook.rs"
    ),
    source!(
        "cas-cli/tests/e2e/hook_e2e/exit_blockers.rs",
        "e2e/hook_e2e/exit_blockers.rs"
    ),
    source!(
        "cas-cli/tests/e2e/hook_e2e/jail_core.rs",
        "e2e/hook_e2e/jail_core.rs"
    ),
    source!(
        "cas-cli/tests/e2e/hook_e2e/jail_edge_cases.rs",
        "e2e/hook_e2e/jail_edge_cases.rs"
    ),
    source!(
        "cas-cli/tests/e2e/hook_e2e/mcp_integration.rs",
        "e2e/hook_e2e/mcp_integration.rs"
    ),
    source!("cas-cli/tests/e2e/hook_e2e/mod.rs", "e2e/hook_e2e/mod.rs"),
    source!(
        "cas-cli/tests/e2e/memory_workflow.rs",
        "e2e/memory_workflow.rs"
    ),
    source!("cas-cli/tests/e2e/mod.rs", "e2e/mod.rs"),
    source!("cas-cli/tests/e2e/multi_agent.rs", "e2e/multi_agent.rs"),
    source!("cas-cli/tests/e2e/rule_workflow.rs", "e2e/rule_workflow.rs"),
    source!(
        "cas-cli/tests/e2e/task_dependencies.rs",
        "e2e/task_dependencies.rs"
    ),
    source!(
        "cas-cli/tests/e2e/task_lifecycle.rs",
        "e2e/task_lifecycle.rs"
    ),
    source!("cas-cli/tests/e2e/team_sync.rs", "e2e/team_sync.rs"),
    source!(
        "cas-cli/tests/e2e/verification_gates.rs",
        "e2e/verification_gates.rs"
    ),
    source!(
        "cas-cli/tests/e2e/worktree_lifecycle.rs",
        "e2e/worktree_lifecycle.rs"
    ),
    source!("cas-cli/tests/e2e_test.rs", "e2e_test.rs"),
    source!(
        "cas-cli/tests/factory_codex_skill_guardrails.rs",
        "factory_codex_skill_guardrails.rs"
    ),
    source!(
        "cas-cli/tests/factory_latency_test.rs",
        "factory_latency_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_mcp_ops_test.rs",
        "factory_mcp_ops_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_parity_test.rs",
        "factory_parity_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_preflight_test.rs",
        "factory_preflight_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_probe_comm_test.rs",
        "factory_probe_comm_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_server_test.rs",
        "factory_server_test.rs"
    ),
    source!(
        "cas-cli/tests/factory_server_test_cases/tests.rs",
        "factory_server_test_cases/tests.rs"
    ),
    source!(
        "cas-cli/tests/fixtures/cas_instance.rs",
        "fixtures/cas_instance.rs"
    ),
    source!(
        "cas-cli/tests/fixtures/hook_instance.rs",
        "fixtures/hook_instance.rs"
    ),
    source!(
        "cas-cli/tests/fixtures/mock_server.rs",
        "fixtures/mock_server.rs"
    ),
    source!("cas-cli/tests/fixtures/mod.rs", "fixtures/mod.rs"),
    source!(
        "cas-cli/tests/fixtures/pull_scoping/cfg_test_module_only.rs",
        "fixtures/pull_scoping/cfg_test_module_only.rs"
    ),
    source!(
        "cas-cli/tests/gh701_origin_filter_measurement_test.rs",
        "gh701_origin_filter_measurement_test.rs"
    ),
    source!(
        "cas-cli/tests/history_search_production_path_test.rs",
        "history_search_production_path_test.rs"
    ),
    source!("cas-cli/tests/hook_schema.rs", "hook_schema.rs"),
    source!(
        "cas-cli/tests/hooks_test/exit_and_summary.rs",
        "hooks_test/exit_and_summary.rs"
    ),
    source!(
        "cas-cli/tests/hooks_test/learning_rule_duplicate.rs",
        "hooks_test/learning_rule_duplicate.rs"
    ),
    source!("cas-cli/tests/hooks_test/mod.rs", "hooks_test/mod.rs"),
    source!(
        "cas-cli/tests/hooks_test/post_tool_and_flow.rs",
        "hooks_test/post_tool_and_flow.rs"
    ),
    source!(
        "cas-cli/tests/host_registry_isolation_test.rs",
        "host_registry_isolation_test.rs"
    ),
    source!(
        "cas-cli/tests/hub_clean_home_test.rs",
        "hub_clean_home_test.rs"
    ),
    source!(
        "cas-cli/tests/hub_detached_lifecycle_test.rs",
        "hub_detached_lifecycle_test.rs"
    ),
    source!(
        "cas-cli/tests/hub_launcher_path_test.rs",
        "hub_launcher_path_test.rs"
    ),
    source!(
        "cas-cli/tests/init_non_project_guard_test.rs",
        "init_non_project_guard_test.rs"
    ),
    source!(
        "cas-cli/tests/init_watchdog_budget_test.rs",
        "init_watchdog_budget_test.rs"
    ),
    source!(
        "cas-cli/tests/integrate_lifecycle_test.rs",
        "integrate_lifecycle_test.rs"
    ),
    source!(
        "cas-cli/tests/issue_intake_directive_test.rs",
        "issue_intake_directive_test.rs"
    ),
    source!("cas-cli/tests/jail_guard_test.rs", "jail_guard_test.rs"),
    source!(
        "cas-cli/tests/knowledge_distillation_test.rs",
        "knowledge_distillation_test.rs"
    ),
    source!(
        "cas-cli/tests/known_repos_binding_test.rs",
        "known_repos_binding_test.rs"
    ),
    source!("cas-cli/tests/loop_test.rs", "loop_test.rs"),
    source!(
        "cas-cli/tests/mcp_action_surface_test.rs",
        "mcp_action_surface_test.rs"
    ),
    source!("cas-cli/tests/mcp_protocol_test.rs", "mcp_protocol_test.rs"),
    source!("cas-cli/tests/mcp_proxy_test.rs", "mcp_proxy_test.rs"),
    source!("cas-cli/tests/mcp_tools_test.rs", "mcp_tools_test.rs"),
    source!(
        "cas-cli/tests/mcp_tools_test/edge_cases.rs",
        "mcp_tools_test/edge_cases.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/entity_handler_tests/mod.rs",
        "mcp_tools_test/entity_handler_tests/mod.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/field_coverage_tests.rs",
        "mcp_tools_test/field_coverage_tests.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/knowledge_tools.rs",
        "mcp_tools_test/knowledge_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/memory_autofix.rs",
        "mcp_tools_test/memory_autofix.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/memory_recent_contract.rs",
        "mcp_tools_test/memory_recent_contract.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/memory_remember_contract.rs",
        "mcp_tools_test/memory_remember_contract.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/memory_tools.rs",
        "mcp_tools_test/memory_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/rule_tools.rs",
        "mcp_tools_test/rule_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/search_tools.rs",
        "mcp_tools_test/search_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/server_protocol/mod.rs",
        "mcp_tools_test/server_protocol/mod.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/skill_tools.rs",
        "mcp_tools_test/skill_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/spec_tools.rs",
        "mcp_tools_test/spec_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/support.rs",
        "mcp_tools_test/support.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/system_tools.rs",
        "mcp_tools_test/system_tools.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/cancellation.rs",
        "mcp_tools_test/task_tools/cancellation.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/create_and_epic.rs",
        "mcp_tools_test/task_tools/create_and_epic.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/dependencies.rs",
        "mcp_tools_test/task_tools/dependencies.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/depth_e2e.rs",
        "mcp_tools_test/task_tools/depth_e2e.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/depth_light_close.rs",
        "mcp_tools_test/task_tools/depth_light_close.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/double_close.rs",
        "mcp_tools_test/task_tools/double_close.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/gate.rs",
        "mcp_tools_test/task_tools/gate.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/mod.rs",
        "mcp_tools_test/task_tools/mod.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/operations.rs",
        "mcp_tools_test/task_tools/operations.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/reopen_atomicity.rs",
        "mcp_tools_test/task_tools/reopen_atomicity.rs"
    ),
    source!(
        "cas-cli/tests/mcp_tools_test/task_tools/verification_flow.rs",
        "mcp_tools_test/task_tools/verification_flow.rs"
    ),
    source!(
        "cas-cli/tests/memory_migration_test.rs",
        "memory_migration_test.rs"
    ),
    source!("cas-cli/tests/memory_share_test.rs", "memory_share_test.rs"),
    source!("cas-cli/tests/multi_agent_test.rs", "multi_agent_test.rs"),
    source!(
        "cas-cli/tests/multi_agent_test_cases/tests.rs",
        "multi_agent_test_cases/tests.rs"
    ),
    source!(
        "cas-cli/tests/openclaw_bridge_test.rs",
        "openclaw_bridge_test.rs"
    ),
    source!(
        "cas-cli/tests/project_identity_parity_test.rs",
        "project_identity_parity_test.rs"
    ),
    source!(
        "cas-cli/tests/project_pull_archived_test.rs",
        "project_pull_archived_test.rs"
    ),
    source!("cas-cli/tests/proptest/mod.rs", "proptest/mod.rs"),
    source!(
        "cas-cli/tests/proptest/search_properties.rs",
        "proptest/search_properties.rs"
    ),
    source!("cas-cli/tests/proptest_test.rs", "proptest_test.rs"),
    source!("cas-cli/tests/provenance.rs", "provenance.rs"),
    source!("cas-cli/tests/provenance_loop.rs", "provenance_loop.rs"),
    source!(
        "cas-cli/tests/provider_shortcuts_test.rs",
        "provider_shortcuts_test.rs"
    ),
    source!(
        "cas-cli/tests/pull_no_reenqueue_test.rs",
        "pull_no_reenqueue_test.rs"
    ),
    source!(
        "cas-cli/tests/pull_scoping_regression_test.rs",
        "pull_scoping_regression_test.rs"
    ),
    source!(
        "cas-cli/tests/pull_watermark_recovery_test.rs",
        "pull_watermark_recovery_test.rs"
    ),
    source!(
        "cas-cli/tests/push_queue_scoping_test.rs",
        "push_queue_scoping_test.rs"
    ),
    source!(
        "cas-cli/tests/push_rehome_guard_test.rs",
        "push_rehome_guard_test.rs"
    ),
    source!("cas-cli/tests/push_skipped_test.rs", "push_skipped_test.rs"),
    source!(
        "cas-cli/tests/real_store_isolation_test.rs",
        "real_store_isolation_test.rs"
    ),
    source!(
        "cas-cli/tests/retrieval_eval_test.rs",
        "retrieval_eval_test.rs"
    ),
    source!(
        "cas-cli/tests/retrieval_parity_test.rs",
        "retrieval_parity_test.rs"
    ),
    source!(
        "cas-cli/tests/search_frontmatter_test.rs",
        "search_frontmatter_test.rs"
    ),
    source!(
        "cas-cli/tests/search_scoring_test.rs",
        "search_scoring_test.rs"
    ),
    source!(
        "cas-cli/tests/search_scoring_test_cases/tests.rs",
        "search_scoring_test_cases/tests.rs"
    ),
    source!(
        "cas-cli/tests/search_utf8_regression_test.rs",
        "search_utf8_regression_test.rs"
    ),
    source!(
        "cas-cli/tests/serve_parent_watchdog_test.rs",
        "serve_parent_watchdog_test.rs"
    ),
    source!(
        "cas-cli/tests/server_registry_mcp_test.rs",
        "server_registry_mcp_test.rs"
    ),
    source!(
        "cas-cli/tests/service_tools_test.rs",
        "service_tools_test.rs"
    ),
    source!(
        "cas-cli/tests/session_start_issue_triage_test.rs",
        "session_start_issue_triage_test.rs"
    ),
    source!(
        "cas-cli/tests/session_start_memory_hygiene_test.rs",
        "session_start_memory_hygiene_test.rs"
    ),
    source!("cas-cli/tests/setup_test.rs", "setup_test.rs"),
    source!(
        "cas-cli/tests/skill_hygiene_test.rs",
        "skill_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/support/builtin_catalog.rs",
        "support/builtin_catalog.rs"
    ),
    source!("cas-cli/tests/support/mod.rs", "support/mod.rs"),
    source!(
        "cas-cli/tests/task_update_verification_type_test.rs",
        "task_update_verification_type_test.rs"
    ),
    source!(
        "cas-cli/tests/task_update_work_target_close_test.rs",
        "task_update_work_target_close_test.rs"
    ),
    source!(
        "cas-cli/tests/team_backfill_test.rs",
        "team_backfill_test.rs"
    ),
    source!("cas-cli/tests/team_default_test.rs", "team_default_test.rs"),
    source!(
        "cas-cli/tests/team_memories_e2e_test.rs",
        "team_memories_e2e_test.rs"
    ),
    source!(
        "cas-cli/tests/team_pull_lww_test.rs",
        "team_pull_lww_test.rs"
    ),
    source!(
        "cas-cli/tests/team_pull_watermark_scope_test.rs",
        "team_pull_watermark_scope_test.rs"
    ),
    source!(
        "cas-cli/tests/team_pull_wiring_test.rs",
        "team_pull_wiring_test.rs"
    ),
    source!(
        "cas-cli/tests/team_registration_test.rs",
        "team_registration_test.rs"
    ),
    source!(
        "cas-cli/tests/team_scope_e2e_test.rs",
        "team_scope_e2e_test.rs"
    ),
    source!(
        "cas-cli/tests/team_set_slug_resolution_test.rs",
        "team_set_slug_resolution_test.rs"
    ),
    source!("cas-cli/tests/team_sync_test.rs", "team_sync_test.rs"),
    source!("cas-cli/tests/teams_fetch_test.rs", "teams_fetch_test.rs"),
    source!(
        "cas-cli/tests/update_all_projects_test.rs",
        "update_all_projects_test.rs"
    ),
    source!(
        "cas-cli/tests/update_sync_report_attribution_test.rs",
        "update_sync_report_attribution_test.rs"
    ),
    source!("cas-cli/tests/verification_test.rs", "verification_test.rs"),
    source!(
        "cas-cli/tests/verification_timeout_atomicity_test.rs",
        "verification_timeout_atomicity_test.rs"
    ),
    source!(
        "cas-cli/tests/verifier_handoff_cleanup_test.rs",
        "verifier_handoff_cleanup_test.rs"
    ),
    source!(
        "cas-cli/tests/verify_before_claim_skill_test.rs",
        "verify_before_claim_skill_test.rs"
    ),
    source!(
        "cas-cli/tests/viktor_distribution_test.rs",
        "viktor_distribution_test.rs"
    ),
    source!(
        "cas-cli/tests/viktor_key_setup_test.rs",
        "viktor_key_setup_test.rs"
    ),
    source!(
        "cas-cli/tests/warning_hygiene_test.rs",
        "warning_hygiene_test.rs"
    ),
    source!(
        "cas-cli/tests/worker_hold_mcp_test.rs",
        "worker_hold_mcp_test.rs"
    ),
    source!(
        "cas-cli/tests/worktree_surface_test.rs",
        "worktree_surface_test.rs"
    ),
    source!("cas-cli/tests/worktree_test.rs", "worktree_test.rs"),
];

#[test]
fn builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime() {
    let forbidden = [
        "env!(\"CARGO_MANIFEST_DIR\")",
        "cas::test_paths::workspace_root()",
    ];
    let mut violations = Vec::new();
    for (name, source) in BUILTIN_INSPECTION_SOURCES {
        for (line_number, line) in source.lines().enumerate() {
            for needle in forbidden {
                let guarded_checkout_probe =
                    needle == "cas::test_paths::workspace_root()" && source.contains("SKIP ");
                if line.contains(needle) && !guarded_checkout_probe {
                    violations.push(format!("{name}:{}: {needle}", line_number + 1));
                }
            }
        }
    }
    let fixture_constructors = [
        concat!("tempdir", "_in("),
        concat!("TempDir", "::new_in("),
    ];
    let forbidden_fixture_parents = [
        concat!("env!(\"", "CARGO_MANIFEST_DIR"),
        concat!("\"/", "tmp"),
        concat!("\"/var/", "tmp"),
    ];
    let fixture_source_is_forbidden = |source: &str| {
        fixture_constructors
            .iter()
            .any(|constructor| source.contains(constructor))
            && forbidden_fixture_parents
                .iter()
                .any(|parent| source.contains(parent))
    };
    for unsafe_fixture in [
        concat!("tempdir", "_in(Path::new(\"/", "tmp\"))"),
        concat!("TempDir", "::new_in(\"/var/", "tmp\")"),
        concat!("tempdir", "_in(\n env!(\"", "CARGO_MANIFEST_DIR", "\"))"),
    ] {
        assert!(
            fixture_source_is_forbidden(unsafe_fixture),
            "fixture source scan does not cover {unsafe_fixture}"
        );
    }
    for allowed_text in [
        "documentation says /tmp is disposable",
        "assert!(path.starts_with(\"/var/tmp\"))",
        "cas::test_paths::runtime_fixture_parent()",
    ] {
        assert!(
            !fixture_source_is_forbidden(allowed_text),
            "fixture source scan false-positive: {allowed_text}"
        );
    }
    for (name, source) in FIXTURE_SOURCES {
        let lines: Vec<_> = source.lines().collect();
        for (line_number, line) in lines.iter().enumerate() {
            if fixture_constructors
                .iter()
                .any(|constructor| line.contains(constructor))
            {
                let snippet = lines[line_number..lines.len().min(line_number + 4)].join("\n");
                if fixture_source_is_forbidden(&snippet) {
                    violations.push(format!("{name}:{}: forbidden fixture parent", line_number + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "real-project fixtures must use cas::test_paths::runtime_fixture_parent(); \
         archive tests must not resolve source files from the producer checkout:\n  {}",
        violations.join("\n  ")
    );
}
