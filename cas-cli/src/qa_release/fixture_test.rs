// Included in qa_release::tests to keep the adapter seam's regression together.
#[test]
fn fixture_cleanup() {
    use super::fixtures::{self, FixtureSuite, SetupError};
    use crate::qa_registry::FixtureRecipe;
    use std::os::unix::fs::PermissionsExt;

    let config = ReleaseConfig {
        adapter: "./fixture-adapter".into(),
        timeout_secs: 5,
        env: vec![
            "FAKE_FIXTURE_LOG".into(),
            "FAKE_SETUP_FAIL".into(),
            "FAKE_TEARDOWN_FAIL".into(),
        ],
        ..ReleaseConfig::default()
    };
    let recipe = FixtureRecipe {
        name: "past_due".into(),
        envs: vec!["staging".into()],
        provides: vec!["account_id".into()],
        ttl_mins: 1,
    };
    if let Some(root) = std::env::var_os("CAS_QA_FIXTURE_CRASH_ROOT") {
        let root = std::path::PathBuf::from(root);
        let mut suite = FixtureSuite::new(&root, &root.join(".cas"), &config, "staging", "abcdef0");
        let receipt = suite.setup(&recipe).unwrap();
        // Durable entry evidence for this intentionally non-returning child.
        std::fs::write(
            root.join("crash-receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        std::process::exit(17); // skips Drop, as a process crash would
    }
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let cas_root = crate::store::init_cas_dir(root).unwrap();
    let adapter = root.join("fixture-adapter");
    std::fs::write(
        &adapter,
        include_str!("../../tests/fixtures/qa-release/fixture-adapter"),
    )
    .unwrap();
    std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
    let log = root.join("calls.jsonl");
    env.set("FAKE_FIXTURE_LOG", log.to_str().unwrap());
    let suite = |target: &str| FixtureSuite::new(root, &cas_root, &config, target, "abcdef0");

    assert!(matches!(
        suite("prod").setup(&recipe),
        Err(SetupError::WrongEnvironment { .. })
    ));
    assert!(!log.exists(), "wrong env must not invoke the adapter");

    env.set("FAKE_SETUP_FAIL", "1");
    let error = suite("staging").setup(&recipe).unwrap_err();
    assert!(matches!(error, SetupError::NotExercised(_)));
    assert!(
        error
            .to_string()
            .starts_with("NOT_EXERCISED{fixture_setup}")
    );
    assert!(
        cas_store::fixture_cleanups(&cas_root, None)
            .unwrap()
            .is_empty()
    );
    env.set("FAKE_SETUP_FAIL", "0");

    // A suite's successful finish tears down and removes its journal row.
    let mut normal = suite("staging");
    let receipt = normal.setup(&recipe).unwrap();
    assert_eq!(receipt.outputs["account_id"], "account-123");
    let row = cas_store::fixture_cleanups(&cas_root, None)
        .unwrap()
        .remove(0);
    assert_eq!(row.id, receipt.cleanup_id);
    assert_eq!(row.env, "staging");
    let report = normal.finish();
    assert_eq!(report.cleaned, 1);
    assert!(report.orphan_debt.is_empty());
    assert!(
        cas_store::fixture_cleanups(&cas_root, None)
            .unwrap()
            .is_empty()
    );

    // Early return/unwind uses the same teardown path.
    let mut aborted = suite("staging");
    aborted.setup(&recipe).unwrap();
    drop(aborted);
    assert!(
        cas_store::fixture_cleanups(&cas_root, None)
            .unwrap()
            .is_empty()
    );

    // Teardown failure leaves durable debt, separately from any verdict.
    let mut orphan = suite("staging");
    orphan.setup(&recipe).unwrap();
    env.set("FAKE_TEARDOWN_FAIL", "1");
    let report = orphan.finish();
    assert_eq!(report.orphan_debt.len(), 1);
    let debt = cas_store::fixture_cleanups(&cas_root, None).unwrap();
    assert!(
        debt[0]
            .orphan_error
            .as_deref()
            .unwrap()
            .contains("cleanup unavailable")
    );
    env.set("FAKE_TEARDOWN_FAIL", "0");

    // A separate process exits without destructors after setup. Sweep reads
    // only the persisted journal; unexpired rows stay untouched until due.
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "qa_release::tests::fixture_cleanup",
            "--nocapture",
        ])
        .env("CAS_QA_FIXTURE_CRASH_ROOT", root)
        .output()
        .unwrap();
    assert_eq!(
        child.status.code(),
        Some(17),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let crash_receipt: fixtures::FixtureReceipt =
        serde_json::from_slice(&std::fs::read(root.join("crash-receipt.json")).unwrap()).unwrap();
    assert!(
        cas_store::fixture_cleanups(&cas_root, None)
            .unwrap()
            .iter()
            .any(|row| row.id == crash_receipt.cleanup_id)
    );
    assert_eq!(
        fixtures::sweep(root, &cas_root, &config).unwrap().cleaned,
        0
    );
    let later = chrono::Utc::now().timestamp() + 61;
    let report = fixtures::sweep_at(root, &cas_root, &config, later).unwrap();
    assert_eq!(report.cleaned, 2);
    assert!(report.orphan_debt.is_empty());
    assert!(
        cas_store::fixture_cleanups(&cas_root, None)
            .unwrap()
            .is_empty()
    );
    let calls: Vec<serde_json::Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(calls.iter().all(|c| c["env"] == "staging"));
    assert!(
        calls
            .iter()
            .filter(|c| c["verb"] == "fixture-teardown")
            .all(|c| c["args"]["handle"] == "fake-persona")
    );
}
