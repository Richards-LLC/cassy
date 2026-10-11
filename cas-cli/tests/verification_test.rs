//! Verification contracts through the current MCP surface and live config CLI.

use assert_cmd::Command;
use cas::store::init_cas_dir;
use predicates::prelude::*;

use crate::test_env_guard::TestEnvGuard;

#[path = "verification_test_cases/current.rs"]
mod current;

/// The config CLI remains supported. `verification.enabled` is operator-only
/// (cas-0d4f0): the operator path (here, the library as a test fixture) sets
/// it, `cas config get` reads it, and a non-interactive agent `cas config set`
/// is refused without changing it.
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
        command.env("HOME", env.home());
        command.current_dir(&project);
        command
    };
    for expected in ["true", "false"] {
        let mut config = cas::config::Config::load(&root).unwrap();
        config.set("verification.enabled", expected).unwrap();
        config.save(&root).unwrap();
        command()
            .args(["config", "get", "verification.enabled"])
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
    command()
        .env("CLAUDECODE", "1")
        .args(["config", "set", "verification.enabled", "true"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("operator"));
    command()
        .args(["config", "get", "verification.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}
