use super::{FocusEpic, focus_epic};
use std::path::Path;

fn git(repo: &Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}

fn with_fixture(check: impl FnOnce(&Path, &Path, &Path, &str)) {
    let temp = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[
        ("HOME", home.path().to_str().unwrap()),
        ("CAS_PUSH_EPIC_BRANCH", "0"),
    ]);
    let origin = temp.path().join("origin.git");
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(temp.path(), &["init", "--bare", origin.to_str().unwrap()]);
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@cas.test"]);
    git(&repo, &["config", "user.name", "Cassy Test"]);
    std::fs::write(repo.join("base"), "old").unwrap();
    git(&repo, &["add", "base"]);
    git(&repo, &["commit", "-m", "base"]);
    git(&repo, &["branch", "epic/unpublished"]);
    std::fs::write(repo.join("base"), "current").unwrap();
    git(&repo, &["commit", "-am", "advance parent"]);
    let base = git(&repo, &["rev-parse", "main"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "origin", "main"]);
    let root = crate::store::init_cas_dir(&repo).unwrap();
    std::fs::write(
        root.join("config.toml"),
        "[project]\ncanonical_id = \"publication-test\"\n",
    )
    .unwrap();
    let mut epic = cas_types::Task::new("cas-unpublished".into(), "Unpublished epic".into());
    epic.task_type = cas_types::TaskType::Epic;
    epic.branch = Some("epic/unpublished".into());
    epic.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "project:publication-test".into(),
        target_branch: "main".into(),
    });
    crate::store::open_task_store(&root)
        .unwrap()
        .add(&epic)
        .unwrap();
    let metadata = crate::ui::factory::metadata_path("publication-test");
    std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    let session = crate::ui::factory::create_metadata(
        "publication-test",
        12345,
        "supervisor",
        &[],
        None,
        repo.to_str(),
        None,
    );
    std::fs::write(metadata, serde_json::to_string(&session).unwrap()).unwrap();
    check(&repo, &origin, &root, &base);
}

fn focus(root: &Path) -> String {
    focus_epic(
        root,
        "publication-test",
        FocusEpic::Pin {
            epic_id: "cas-unpublished",
            delivery_mode: None,
        },
    )
    .unwrap()
}

#[test]
fn cas_bfa1_focus_publishes_missing_epic_and_advances_stale_local() {
    with_fixture(|repo, origin, root, base| {
        let receipt = focus(root);
        assert_eq!(
            git(&origin, &["rev-parse", "refs/heads/epic/unpublished"]),
            base,
            "focus must publish the missing remote epic at its recorded base"
        );
        assert_eq!(git(&repo, &["rev-parse", "epic/unpublished"]), base);
        assert!(
            receipt.contains("published") && receipt.contains("epic/unpublished"),
            "{receipt}"
        );
        assert!(receipt.contains("FAST-FORWARDED"), "{receipt}");
        assert!(receipt.contains("CAS_PUSH_EPIC_BRANCH=0"), "{receipt}");
        let repeated = focus(root);
        assert!(!repeated.contains("FAST-FORWARDED"), "{repeated}");
    });
}

#[test]
fn cas_bfa1_self_target_preserves_tip_and_recorded_parent_preserves_unique_work() {
    with_fixture(|repo, origin, root, _| {
        let store = crate::store::open_task_store(root).unwrap();
        let mut epic = store.get("cas-unpublished").unwrap();
        epic.deliverables
            .work_target
            .as_mut()
            .unwrap()
            .target_branch = "epic/unpublished".into();
        store.update(&epic).unwrap();
        let old = git(repo, &["rev-parse", "epic/unpublished"]);
        let receipt = focus(root);
        assert_eq!(git(repo, &["rev-parse", "epic/unpublished"]), old);
        assert_eq!(git(origin, &["rev-parse", "epic/unpublished"]), old);
        assert!(
            receipt.contains("unchanged") && !receipt.contains("FAST-FORWARDED"),
            "{receipt}"
        );
    });
    with_fixture(|repo, origin, root, _| {
        git(repo, &["checkout", "epic/unpublished"]);
        std::fs::write(repo.join("unique"), "operator work").unwrap();
        git(repo, &["add", "unique"]);
        git(repo, &["commit", "-m", "unique work"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        let receipt = focus(root);
        assert_eq!(git(repo, &["rev-parse", "epic/unpublished"]), tip);
        assert_eq!(git(origin, &["rev-parse", "epic/unpublished"]), tip);
        assert!(
            receipt.contains("unique local commit") && receipt.contains("unchanged"),
            "{receipt}"
        );
    });
}

#[test]
fn cas_bfa1_existing_remote_divergence_is_preserved() {
    with_fixture(|repo, origin, root, base| {
        git(origin, &["update-ref", "refs/heads/epic/unpublished", base]);
        git(repo, &["checkout", "epic/unpublished"]);
        std::fs::write(repo.join("unique"), "local work").unwrap();
        git(repo, &["add", "unique"]);
        git(repo, &["commit", "-m", "local work"]);
        let local = git(repo, &["rev-parse", "HEAD"]);
        let receipt = focus(root);
        assert_eq!(git(repo, &["rev-parse", "epic/unpublished"]), local);
        assert_eq!(git(origin, &["rev-parse", "epic/unpublished"]), base);
        assert!(receipt.contains("DIVERGED"), "{receipt}");
        assert_eq!(
            std::fs::read_to_string(repo.join("unique")).unwrap(),
            "local work"
        );
    });
}

#[cfg(unix)]
#[test]
fn cas_bfa1_expected_absent_publication_does_not_replace_racing_remote() {
    use std::os::unix::fs::PermissionsExt;
    with_fixture(|repo, origin, root, _| {
        let old = git(repo, &["rev-parse", "main~1"]);
        let hook = repo.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\ngit --git-dir='{}' update-ref refs/heads/epic/unpublished '{}'\n",
                origin.display(),
                old
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let receipt = focus(root);
        assert_eq!(
            git(origin, &["rev-parse", "epic/unpublished"]),
            old,
            "a concurrent creator must not be overwritten"
        );
        assert!(
            receipt.contains("UNPUBLISHED") && receipt.contains("expected-absent"),
            "{receipt}"
        );
    });
}

#[test]
fn cas_bfa1_round3_focus_refuses_deleted_local_and_remote_lane() {
    with_fixture(|repo, origin, root, _| {
        let tasks = crate::store::open_task_store(root).unwrap();
        let mut epic = tasks.get("cas-unpublished").unwrap();
        let host = crate::cloud::device::DeviceConfig::hostname().unwrap();
        epic.deliverables.epic_branch_origin = Some(cas_types::EpicBranchOrigin {
            branch: "epic/unpublished".into(),
            host: format!("\n{host}\r\u{7}"),
        });
        tasks.update(&epic).unwrap();
        git(repo, &["branch", "-D", "epic/unpublished"]);
        let receipt = focus(root);
        assert!(
            receipt.contains(&format!("epic branch missing on originating host {host}"))
                && receipt.contains("Restore epic/unpublished from a known commit or backup")
                && !receipt.contains("not published by")
                && !receipt.contains("git push")
                && !receipt.contains("spawn refused:")
                && receipt.lines().any(|line| line == "git reflog --all"),
            "source host needs recovery advice and a clean command line: {receipt}"
        );
        // pin: actual focus advice keeps the recovery command free of receipt prose.
        let diagnostic = receipt
            .split("EPIC BRANCH UNVERIFIED: ")
            .nth(1)
            .unwrap()
            .split("\nFocus recorded;")
            .next()
            .unwrap();
        assert_eq!(
            format!("{}\n", diagnostic.replace(&host, "soundwave")),
            include_str!("../../../docs/design/cli/epic-source-recovery.capture.txt")
        );
        for path in [repo, origin] {
            assert!(
                git(
                    path,
                    &[
                        "for-each-ref",
                        "--format=%(refname)",
                        "refs/heads/epic/unpublished"
                    ]
                )
                .is_empty()
            );
        }
    });
}

#[test]
fn cas_bfa1_dirty_checkout_and_unreachable_origin_are_preserved() {
    with_fixture(|repo, origin, root, _| {
        git(repo, &["checkout", "epic/unpublished"]);
        let old = git(repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("base"), "uncommitted work").unwrap();
        let receipt = focus(root);
        assert_eq!(git(repo, &["rev-parse", "HEAD"]), old);
        assert_eq!(
            std::fs::read_to_string(repo.join("base")).unwrap(),
            "uncommitted work"
        );
        assert!(
            git(
                origin,
                &[
                    "ls-remote",
                    "--heads",
                    origin.to_str().unwrap(),
                    "refs/heads/epic/unpublished"
                ]
            )
            .is_empty()
        );
        assert!(
            receipt.contains("UNVERIFIED") && receipt.contains("tracked changes"),
            "{receipt}"
        );
    });
    with_fixture(|repo, _, root, _| {
        let old = git(repo, &["rev-parse", "epic/unpublished"]);
        git(
            repo,
            &[
                "remote",
                "set-url",
                "origin",
                "/nonexistent/cas-bfa1-origin.git",
            ],
        );
        let receipt = focus(root);
        assert_eq!(git(repo, &["rev-parse", "epic/unpublished"]), old);
        assert!(
            receipt.contains("UNVERIFIED") && receipt.contains("origin presence"),
            "{receipt}"
        );
    });
}

#[test]
fn cas_bfa1_failed_base_fetch_does_not_publish_local_fallback() {
    with_fixture(|repo, origin, root, _| {
        git(origin, &["update-ref", "-d", "refs/heads/main"]);
        let old = git(repo, &["rev-parse", "epic/unpublished"]);
        let receipt = focus(root);
        assert!(
            receipt.contains("UNVERIFIED")
                && receipt.contains("base")
                && receipt.contains("fetch failed"),
            "{receipt}"
        );
        assert_eq!(git(repo, &["rev-parse", "epic/unpublished"]), old);
        assert!(
            git(
                origin,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/epic/unpublished"
                ]
            )
            .is_empty()
        );
    });
}

#[test]
fn cas_bfa1_round3_focus_refuses_cancelled_epic() {
    with_fixture(|repo, origin, root, _| {
        let tasks = crate::store::open_task_store(root).unwrap();
        let mut epic = tasks.get("cas-unpublished").unwrap();
        epic.status = cas_types::TaskStatus::Cancelled;
        tasks.update(&epic).unwrap();
        let error = focus_epic(
            root,
            "publication-test",
            FocusEpic::Pin {
                epic_id: "cas-unpublished",
                delivery_mode: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("is cancelled") && error.contains("Terminal epics cannot be pinned"),
            "{error}"
        );
        assert!(
            git(
                origin,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/epic/unpublished"
                ]
            )
            .is_empty()
        );
        assert_eq!(
            git(repo, &["rev-parse", "epic/unpublished"]),
            git(repo, &["rev-parse", "main~1"])
        );
    });
}

#[test]
fn cas_bfa1_round4_focus_no_parent_preserves_old_tag_tip() {
    for self_target in [true, false] {
        with_fixture(|repo, origin, root, _| {
            let tasks = crate::store::open_task_store(root).unwrap();
            let mut epic = tasks.get("cas-unpublished").unwrap();
            epic.deliverables.work_target = self_target.then(|| cas_types::WorkTarget {
                repo_selector: "project:publication-test".into(),
                target_branch: "epic/unpublished".into(),
            });
            tasks.update(&epic).unwrap();
            let old = git(repo, &["rev-parse", "epic/unpublished"]);
            git(repo, &["tag", "hotfix-base", &old]);
            let receipt = focus(root);
            assert_eq!(git(repo, &["rev-parse", "epic/unpublished"]), old);
            assert_eq!(git(origin, &["rev-parse", "epic/unpublished"]), old);
            assert!(
                receipt.contains("published") && !receipt.contains("FAST-FORWARDED"),
                "{receipt}"
            );
        });
    }
}
