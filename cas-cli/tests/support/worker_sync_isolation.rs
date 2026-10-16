//! Re-run real CLI cases with a worker binding to a disposable host stand-in.
//! Never reproduce a write escape against the developer's actual worktree.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::test_env_guard::TestEnvGuard;

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let path = entry.path();
            (
                path.strip_prefix(root).unwrap().to_path_buf(),
                (
                    std::fs::read(path).unwrap(),
                    std::fs::metadata(path).unwrap().modified().unwrap(),
                ),
            )
        })
        .collect()
}

pub fn assert_worker_cases_are_hermetic(cases: &[&str]) {
    // One guard in this process; each case below runs in a different process.
    let env = TestEnvGuard::temp_home();
    let host = tempfile::TempDir::new().unwrap();
    let host_root = host.path().join(".cas");
    let output = Command::new(cas::test_paths::cas_binary())
        .current_dir(host.path())
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .args(["init", "--yes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stand-in init: {}", String::from_utf8_lossy(&output.stderr));
    for harness in [".codex", ".grok"] {
        std::fs::create_dir_all(host.path().join(harness)).unwrap();
    }
    // A worker-scoped sync will restore this file and leave reference-state
    // files in the stand-in. It cannot be masked by an up-to-date host install.
    std::fs::remove_file(host.path().join(".claude/skills/cas-worker/SKILL.md")).unwrap();
    let before = snapshot(host.path());
    let executable = std::env::current_exe().unwrap();
    let mut results = Vec::new();
    for case in cases {
        let output = Command::new(&executable)
            .args(["--exact", case, "--nocapture"])
            .env("HOME", env.home())
            .env("CAS_AGENT_ROLE", "worker")
            .env("CAS_AGENT_ID", "cas-76e4-disposable-worker")
            .env("CAS_FACTORY_SESSION", "cas-76e4-disposable-session")
            .env("CAS_CLONE_PATH", host.path())
            .env("CAS_ROOT", &host_root)
            .env("CAS_DIR", &host_root)
            .env("CLAUDE_PROJECT_DIR", host.path())
            .env("CODEX_HOME", host.path().join(".codex"))
            .output()
            .unwrap();
        // Check containment before test success: a failure must identify the
        // unsafe write even when the child also fails its product assertion.
        let after = snapshot(host.path());
        let changed: Vec<OsString> = before.keys().chain(after.keys())
            .filter(|path| before.get(*path) != after.get(*path))
            .map(|path| path.as_os_str().to_owned())
            .collect();
        assert!(changed.is_empty(), "{case} wrote outside its fixture into the worker worktree: {changed:?}");
        results.push((case, output));
    }
    for (case, output) in results {
        assert!(output.status.success(), "{case} failed under worker env:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"), "{case} did not execute one test");
    }
}
