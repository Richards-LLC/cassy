use std::path::PathBuf;

fn cas_binary() -> PathBuf {
    cas::test_paths::binary("cas", option_env!("CARGO_BIN_EXE_cas").map(PathBuf::from))
}

#[tokio::test(flavor = "current_thread")]
async fn internal_provisioner_main_dispatch_round_trips_success() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_dir = project.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    std::fs::write(
        cas_dir.join("config.toml"),
        "[factory]\nspawn_min_free_gib = 0\n",
    )
    .unwrap();

    // The real launcher supplies its build hash, invokes main's hidden
    // provisioner dispatch and decodes a typed WorkerSpawnResult.
    let (worker, receipt) = cas::ui::factory::launch_provisioner_for_test(
        cas_binary(),
        serde_json::json!({
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
        }),
    )
    .await
    .unwrap();
    assert_eq!(worker.worker_name, "dispatch-fixture");
    assert_eq!(
        worker.cwd.canonicalize().unwrap(),
        project.path().canonicalize().unwrap(),
    );
    assert!(worker.worktree.is_none());
    assert!(!receipt.is_empty());
}
