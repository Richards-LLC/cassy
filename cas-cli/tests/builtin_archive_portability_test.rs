//! Archive portability guard for the builtin inspection tests.
//!
//! The tests in this list run from nextest archives, where the checkout is not
//! present. Their source inputs must therefore be embedded in the test binary
//! or supplied by Cassy's embedded builtin catalogs.

const TEST_SOURCES: &[(&str, &str)] = &[
    (
        "agent_definition_contract_test.rs",
        include_str!("agent_definition_contract_test.rs"),
    ),
    (
        "builtin_doc_hygiene_test.rs",
        include_str!("builtin_doc_hygiene_test.rs"),
    ),
    (
        "builtin_flavor_drift_test.rs",
        include_str!("builtin_flavor_drift_test.rs"),
    ),
    (
        "builtin_skill_description_test.rs",
        include_str!("builtin_skill_description_test.rs"),
    ),
    (
        "cas_image_generate_skill_test.rs",
        include_str!("cas_image_generate_skill_test.rs"),
    ),
    (
        "factory_codex_skill_guardrails.rs",
        include_str!("factory_codex_skill_guardrails.rs"),
    ),
    (
        "mcp_action_surface_test.rs",
        include_str!("mcp_action_surface_test.rs"),
    ),
    (
        "skill_hygiene_test.rs",
        include_str!("skill_hygiene_test.rs"),
    ),
    (
        "verify_before_claim_skill_test.rs",
        include_str!("verify_before_claim_skill_test.rs"),
    ),
];

#[test]
fn builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime() {
    let forbidden = [
        "env!(\"CARGO_MANIFEST_DIR\")",
        "cas::test_paths::workspace_root()",
    ];
    let mut violations = Vec::new();
    for (name, source) in TEST_SOURCES {
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
    assert!(
        violations.is_empty(),
        "builtin archive tests must not resolve source files from the checkout:\n  {}",
        violations.join("\n  ")
    );
}
