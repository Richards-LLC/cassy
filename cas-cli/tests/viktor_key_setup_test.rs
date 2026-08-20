//! User-scoped Viktor credential setup contract.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_cmd(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env_remove("VIKTOR_API_KEY")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

#[test]
fn missing_viktor_key_gives_the_one_step_setup_command() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .args(["viktor"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cas viktor key <operator-issued-key>"));
}
