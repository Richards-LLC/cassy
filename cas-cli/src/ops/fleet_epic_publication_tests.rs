use super::{FocusEpic, focus_epic};
use std::path::Path;

fn git(repo: &Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(result.status.success(), "{args:?}: {}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}

#[test]
fn cas_bfa1_focus_publishes_missing_epic_and_advances_stale_local() {
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
    git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&repo, &["push", "origin", "main"]);
    let root = crate::store::init_cas_dir(&repo).unwrap();
    let mut epic = cas_types::Task::new("cas-unpublished".into(), "Unpublished epic".into());
    epic.task_type = cas_types::TaskType::Epic;
    epic.branch = Some("epic/unpublished".into());
    epic.deliverables.work_target = crate::mcp::tools::core::task::repo_context::declare_work_target(
        &root, Some(repo.to_str().unwrap()), Some("main"),
    ).unwrap();
    crate::store::open_task_store(&root).unwrap().add(&epic).unwrap();
    let receipt = focus_epic(&root, "publication-test", FocusEpic::Pin {
        epic_id: &epic.id, delivery_mode: None,
    }).unwrap();
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/epic/unpublished"]), base,
        "focus must publish the missing remote epic at its recorded base"
    );
    assert_eq!(git(&repo, &["rev-parse", "epic/unpublished"]), base);
    assert!(receipt.contains("published") && receipt.contains("epic/unpublished"), "{receipt}");
    assert!(receipt.contains("FAST-FORWARDED"), "{receipt}");
    let repeated = focus_epic(&root, "publication-test", FocusEpic::Pin {
        epic_id: &epic.id, delivery_mode: None,
    }).unwrap();
    assert!(!repeated.contains("FAST-FORWARDED"), "{repeated}");
}
