//! Contracts integration suites, linked once.
//! Run with nextest for one-process-per-test environment/cwd isolation.

#[path = "../agent_definition_contract_test.rs"]
mod agent_definition_contract_test;
#[path = "../agents_md_sync_test.rs"]
mod agents_md_sync_test;
#[path = "../builtin_doc_hygiene_test.rs"]
mod builtin_doc_hygiene_test;
#[path = "../builtin_skill_description_test.rs"]
mod builtin_skill_description_test;
#[path = "../cas_image_generate_helper_test.rs"]
mod cas_image_generate_helper_test;
#[path = "../cas_image_generate_skill_test.rs"]
mod cas_image_generate_skill_test;
#[path = "../cas_technical_drawing_skill_test.rs"]
mod cas_technical_drawing_skill_test;
#[path = "../credential_debug_guard_test.rs"]
mod credential_debug_guard_test;
#[path = "../factory_codex_skill_guardrails.rs"]
mod factory_codex_skill_guardrails;
#[path = "../hook_schema.rs"]
mod hook_schema;
#[path = "../hooks_test/main.rs"]
mod hooks_test;
#[path = "../issue_intake_directive_test.rs"]
mod issue_intake_directive_test;
#[path = "../mcp_action_surface_test.rs"]
mod mcp_action_surface_test;
#[path = "../project_identity_parity_test.rs"]
mod project_identity_parity_test;
#[path = "../skill_hygiene_test.rs"]
mod skill_hygiene_test;
#[path = "../verify_before_claim_skill_test.rs"]
mod verify_before_claim_skill_test;
#[path = "../warning_hygiene_test.rs"]
mod warning_hygiene_test;
