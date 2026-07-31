//! Regression checks for integration-test subprocess host isolation.

use std::path::Path;

const INIT_FIXTURES: &[&str] = &[
    "blame_attribution_test.rs",
    "component_output_test.rs",
    "fixtures/cas_instance.rs",
    "hooks_test/mod.rs",
    "jail_guard_test.rs",
    "loop_test.rs",
    "search_scoring_test.rs",
    "verification_test.rs",
    "verifier_handoff_cleanup_test.rs",
];

#[test]
fn every_init_fixture_overrides_home_for_spawned_cas_children() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut missing = Vec::new();

    for relative in INIT_FIXTURES {
        let source = std::fs::read_to_string(tests_dir.join(relative))
            .unwrap_or_else(|error| panic!("read {relative}: {error}"));
        if !source.contains("env(\"HOME\"") {
            missing.push(*relative);
        }
    }

    assert!(
        missing.is_empty(),
        "cas init fixtures must override HOME before spawning the production cas binary; missing: {missing:?}"
    );
}
