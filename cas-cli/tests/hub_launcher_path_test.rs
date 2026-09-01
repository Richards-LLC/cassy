#![cfg(unix)]

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn cas_command(home: &Path, path: &Path) -> Command {
    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

#[test]
fn detached_hub_launcher_starts_with_an_empty_path() {
    let home = TempDir::new().unwrap();
    let empty_path = TempDir::new().unwrap();

    let start = cas_command(home.path(), empty_path.path())
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start hub with empty PATH");
    assert!(
        start.status.success(),
        "hub start failed with empty PATH: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    let record: Value = serde_json::from_slice(&start.stdout).expect("start output is JSON");
    assert!(record["pid"].as_u64().is_some(), "start output has pid: {record}");

    let stop = cas_command(home.path(), empty_path.path())
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop hub with empty PATH");
    assert!(
        stop.status.success(),
        "hub stop failed with empty PATH: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}
