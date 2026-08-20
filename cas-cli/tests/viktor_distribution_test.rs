//! Distribution contract for the managed Viktor gateway.

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
fn clean_init_and_sync_distribute_viktor_skill_and_status_never_prints_a_key() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    assert!(
        temp.path()
            .join(".claude/skills/cas-viktor/SKILL.md")
            .is_file(),
        "missing Claude Viktor skill after cas init"
    );

    // `cas init` configures only detected harnesses. Once a project opts into
    // Codex/Grok by creating their project directories, `cas update --sync`
    // refreshes each enabled mirror through the ordinary downstream path.
    std::fs::create_dir_all(temp.path().join(".codex")).unwrap();
    std::fs::create_dir_all(temp.path().join(".grok")).unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["update", "--sync"])
        .assert()
        .success();

    for path in [
        ".claude/skills/cas-viktor/SKILL.md",
        ".claude/skills/cas-viktor/references/gateway.md",
        ".codex/skills/cas-viktor/SKILL.md",
        ".codex/skills/cas-viktor/references/gateway.md",
        ".grok/skills/cas-viktor/SKILL.md",
        ".grok/skills/cas-viktor/references/gateway.md",
    ] {
        assert!(
            temp.path().join(path).is_file(),
            "missing {path} after cas init"
        );
    }

    let sentinel = "viktor-test-secret-must-not-appear";
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "viktor"])
        .env("VIKTOR_API_KEY", sentinel)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"credential_env\": \"VIKTOR_API_KEY\"",
        ))
        .stdout(predicate::str::contains("\"credential_present\": true"))
        .stdout(predicate::str::contains(sentinel).not());
}
