//! Verification contracts through the current MCP surface and live config CLI.

use assert_cmd::Command;
use cas::store::init_cas_dir;
use predicates::prelude::*;

use crate::test_env_guard::TestEnvGuard;

#[path = "verification_test_cases/current.rs"]
mod current;

/// The config CLI remains supported; keep its enable/disable journey active.
#[test]
fn test_verification_config_toggle() {
    let mut env = TestEnvGuard::temp_home();
    let project = env.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let root = init_cas_dir(&project).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let command = || {
        let mut command = Command::new(cas::test_paths::cas_binary());
        command.current_dir(&project);
        command
    };
    for expected in ["true", "false"] {
        command()
            .args(["config", "set", "verification.enabled", expected])
            .assert()
            .success();
        command()
            .args(["config", "get", "verification.enabled"])
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
}
