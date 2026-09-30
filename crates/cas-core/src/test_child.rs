//! Shared libtest re-execution evidence. Included only by test targets.

pub fn assert_started(stdout: &str, name: &str) {
    assert!(
        stdout.lines().any(|line| line == "running 1 test")
            && (stdout.contains(&format!("test {name} ..."))
                || stdout.contains(&format!("test {name} - should panic ..."))),
        "child did not execute exactly {name}:\n{stdout}"
    );
}

pub fn assert_passed(stdout: &str, name: &str) {
    assert_started(stdout, name);
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
        "child did not complete exactly one passing test ({name}):\n{stdout}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_child_summary_passes() {
        assert_passed(
            "running 1 test\ntest child::probe ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 2 filtered out;",
            "child::probe",
        );
    }

    #[test]
    fn zero_wrong_name_skipped_or_eleven_do_not_prove_a_child() {
        for stdout in [
            "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored;",
            "running 1 test\ntest other::probe ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;",
            "running 1 test\ntest child::probe ... ignored\ntest result: ok. 0 passed; 0 failed; 1 ignored;",
            "running 11 tests\ntest child::probe ... ok\ntest result: ok. 11 passed; 0 failed; 0 ignored;",
        ] {
            assert!(std::panic::catch_unwind(|| assert_passed(stdout, "child::probe")).is_err());
        }
    }

    #[test]
    fn real_zero_match_reexecution_is_rejected() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "__cas_zero_execution_fixture_not_a_test__"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            std::panic::catch_unwind(|| assert_passed(
                &stdout,
                "__cas_zero_execution_fixture_not_a_test__"
            ))
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| assert_started(
                &stdout,
                "__cas_zero_execution_fixture_not_a_test__"
            ))
            .is_err()
        );
    }

    #[test]
    fn early_exit_requires_startup_evidence() {
        assert_started("running 1 test\ntest child::probe ... ", "child::probe");
        assert!(
            std::panic::catch_unwind(|| assert_started("running 0 tests", "child::probe")).is_err()
        );
    }
}
