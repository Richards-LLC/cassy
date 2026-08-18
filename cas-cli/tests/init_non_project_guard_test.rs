//! `cas init` must not scaffold a project into the home directory (cas-2962,
//! Ben's field report #8b: init in `$HOME` created CLAUDE.md, .gitignore,
//! .mcp.json and scripts/ loose in the home directory with no warning).
//!
//! The classification itself is unit-tested in `cli::init::non_project_guard_tests`;
//! this drives the real binary to prove the guard is wired in, that nothing is
//! written when it fires, and that automation can still opt in.
//!
//! The "allowed" direction needs no test of its own here: every other
//! integration test in this suite runs `cas init` in a temp directory, so a
//! false positive would fail the suite loudly.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cas_init_in(home: &Path, cwd: &Path, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    cmd.current_dir(cwd)
        .env("HOME", home)
        .env_remove("CAS_ROOT")
        .arg("init")
        .arg("-y")
        .args(extra);
    cmd.output().expect("run cas init")
}

#[test]
fn init_in_home_refuses_and_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let output = cas_init_in(&home, &home, &[]);

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "init in $HOME must fail, not silently scaffold; stderr: {stderr}"
    );
    assert!(
        stderr.contains("home directory"),
        "the refusal must say what the directory is; stderr: {stderr}"
    );
    assert!(
        stderr.contains("--allow-non-project"),
        "the refusal must name the escape hatch; stderr: {stderr}"
    );

    let leftovers: Vec<String> = std::fs::read_dir(&home)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        leftovers.is_empty(),
        "nothing may be written to $HOME when the guard fires, found: {leftovers:?}"
    );
}

#[test]
fn allow_non_project_lets_automation_through() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let output = cas_init_in(&home, &home, &["--allow-non-project"]);

    assert!(
        output.status.success(),
        "--allow-non-project must bypass the guard; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        home.join(".cas").exists(),
        "the bypass must actually initialize"
    );
}
