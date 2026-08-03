//! Source-level regression checks for platform and test-crate warning hygiene.

fn assert_preceded_by(source: &str, item: &str, attribute: &str) {
    let item_offset = source
        .find(item)
        .unwrap_or_else(|| panic!("missing source item: {item}"));
    let prefix = &source[..item_offset];
    let previous_line = prefix.lines().next_back().unwrap_or_default();
    assert_eq!(
        previous_line.trim(),
        attribute,
        "{item} must be immediately preceded by {attribute}"
    );
}

#[test]
fn warning_only_symbols_are_scoped_to_the_builds_that_use_them() {
    let factory = include_str!("../src/cli/factory/mod.rs");
    assert_preceded_by(
        factory,
        "fn parse_num_threads_from_proc_stat",
        "#[cfg(target_os = \"linux\")]",
    );
    for test in [
        "fn parse_num_threads_from_proc_stat_reads_field_20_when_one",
        "fn parse_num_threads_from_proc_stat_reads_field_20_when_many",
        "fn parse_num_threads_from_proc_stat_handles_comm_with_spaces_and_parens",
        "fn parse_num_threads_from_proc_stat_rejects_malformed",
    ] {
        assert_preceded_by(factory, test, "#[cfg(target_os = \"linux\")]");
    }

    let daemon = include_str!("../src/mcp/daemon.rs");
    assert_preceded_by(
        daemon,
        "pub(crate) fn parse_starttime_from_stat",
        "#[cfg(target_os = \"linux\")]",
    );

    let daemon_tests = include_str!("../src/mcp/daemon_tests/tests.rs");
    for test in [
        "fn parse_starttime_from_stat_handles_comm_with_parens_and_spaces",
        "fn parse_starttime_from_stat_returns_none_on_malformed_input",
    ] {
        assert_preceded_by(daemon_tests, test, "#[cfg(target_os = \"linux\")]");
    }

    let wedged = include_str!("../src/cli/factory/wedged.rs");
    assert_preceded_by(
        wedged,
        "fn is_zombie_state",
        "#[cfg(target_os = \"linux\")]",
    );
    for test in [
        "fn is_zombie_state_parses_z_state_from_stat_line",
        "fn is_zombie_state_handles_comm_with_parens_and_spaces",
    ] {
        assert_preceded_by(wedged, test, "#[cfg(target_os = \"linux\")]");
    }

    let support = include_str!("support/mod.rs");
    assert!(
        support.contains("#![allow(dead_code)]"),
        "shared integration-test support needs a crate-level dead_code allowance"
    );
    assert!(
        support.contains("compiled separately for each integration-test binary"),
        "the allowance must explain the per-test-binary false positive"
    );

    let process = include_str!("../src/ui/factory/daemon/process.rs");
    assert!(
        !process.contains("use std::io::Write as _;"),
        "the test module inherits Write from daemon imports"
    );

    let factory_ui = include_str!("../src/ui/factory/mod.rs");
    assert!(
        factory_ui.contains(
            "#[cfg(test)]\npub(crate) use app::{queue_codex_worker_intro_prompt, queue_supervisor_intro_prompt};"
        ),
        "the parity-only re-export must compile only in test builds"
    );
}
