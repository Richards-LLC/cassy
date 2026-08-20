//! Regression checks for integration-test subprocess host isolation.

const INIT_FIXTURES: &[&str] = &[
    "blame_attribution_test.rs",
    "cli_test.rs",
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
    let tests_dir = cas::test_paths::crate_root().join("tests");
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

#[test]
fn low_level_init_helper_has_no_host_registry_side_effect() {
    let source =
        std::fs::read_to_string(cas::test_paths::crate_root().join("src/store/detect.rs"))
            .expect("read store/detect.rs");

    assert!(
        !source.contains("known_repos::ensure_host_schema")
            && !source.contains("known_repos::register_repo"),
        "init_cas_dir is used directly by integration fixtures and must not resolve or write the host registry"
    );
}
