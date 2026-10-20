use std::path::PathBuf;
use std::process::Command;

fn cas_binary() -> PathBuf {
    cas::test_paths::binary("cas", option_env!("CARGO_BIN_EXE_cas").map(PathBuf::from))
}

#[test]
fn internal_provisioner_main_dispatch_round_trips_success() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_dir = project.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    std::fs::write(
        cas_dir.join("config.toml"),
        "[factory]\nspawn_min_free_gib = 0\n",
    )
    .unwrap();

    let executable = cas_binary();
    let build_hash = Command::new(&executable)
        .arg("--internal-factory-build-id")
        .output()
        .unwrap();
    assert!(build_hash.status.success());
    let build_hash = String::from_utf8(build_hash.stdout)
        .unwrap()
        .trim()
        .to_owned();
    let input_dir = tempfile::tempdir().unwrap();
    let input = input_dir.path().join("input.json");
    let output = input_dir.path().join("output.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&serde_json::json!({
            "build_hash": build_hash,
            "context": {
                "worker_name": "dispatch-fixture",
                "spawn_type": "named",
                "isolate": false,
                "task_id": null,
                "project_path": project.path(),
                "cas_dir": cas_dir,
                "worktree_repo_root": null,
                "worktree_root": null,
                "epic_branch": null,
                "current_epic_id": null,
                "factory_session": null
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let dispatched = Command::new(executable)
        .arg("--internal-factory-provisioner")
        .arg(&input)
        .arg(&output)
        .current_dir(project.path())
        .output()
        .unwrap();
    assert!(
        dispatched.status.success(),
        "{}",
        String::from_utf8_lossy(&dispatched.stderr)
    );
    let outcome: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let worker = outcome
        .get("Ok")
        .and_then(|outcome| outcome.get("result"))
        .expect("provisioner should return a worker result");
    assert_eq!(worker["worker_name"], "dispatch-fixture");
    assert_eq!(worker["cwd"], project.path().to_string_lossy().as_ref());
    assert!(worker["worktree"].is_null());
    assert!(!outcome["Ok"]["receipt"].as_str().unwrap().is_empty());
}
