use crate::types::WorktreeStatus;
use crate::worktree::manager::worker_ops::RemoveOutcome;
use crate::worktree::manager::*;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn create_test_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().to_path_buf();

    Command::new("git")
        .args(["init"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    std::fs::write(repo_path.join("README.md"), "# Test").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    (temp, repo_path)
}

#[test]
fn test_manager_creation() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    assert!(!manager.is_enabled());
    assert!(!manager.is_in_worktree());
}

#[test]
fn test_worktree_path_calculation_for_epic() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig {
        base_path: "../{project}-worktrees".to_string(),
        ..Default::default()
    };

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    let path = manager.worktree_path_for_epic("cas-epic-1234");

    assert!(path.to_string_lossy().contains("-worktrees"));
    assert!(path.to_string_lossy().contains("cas-epic-1234"));
}

#[test]
fn test_branch_name_calculation_for_epic() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig {
        branch_prefix: "cas/".to_string(),
        ..Default::default()
    };

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    let branch = manager.branch_name_for_epic("cas-epic-1234");

    assert_eq!(branch, "cas/cas-epic-1234");
}

#[test]
fn test_create_worktree_for_epic_disabled() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig {
        enabled: false,
        ..Default::default()
    };

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    let result = manager.create_for_epic("cas-epic-1234", None);

    assert!(matches!(result, Err(WorktreeError::NotEnabled)));
}

#[test]
fn test_create_and_cleanup_worktree_for_epic() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig {
        enabled: true,
        auto_merge: false,
        cleanup_on_close: true,
        ..Default::default()
    };

    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let mut worktree = manager
        .create_for_epic("cas-epic-test-123", Some("agent-1"))
        .unwrap();

    assert!(worktree.path.exists());
    assert_eq!(worktree.status, WorktreeStatus::Active);
    assert_eq!(worktree.epic_id, Some("cas-epic-test-123".to_string()));

    manager.abandon(&mut worktree, false).unwrap();

    assert_eq!(worktree.status, WorktreeStatus::Removed);
    assert!(!worktree.path.exists());
}

#[test]
fn test_worktree_path_for_worker() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    let path = manager.worktree_path_for_worker("swift-fox");

    assert!(path.to_string_lossy().contains(".cas/worktrees"));
    assert!(path.to_string_lossy().contains("swift-fox"));
}

#[test]
fn test_branch_name_for_worker() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    let branch = manager.branch_name_for_worker("swift-fox");

    assert_eq!(branch, "factory/swift-fox");
}

#[test]
fn test_create_worker_worktree() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.create_for_worker("swift-fox").unwrap();

    assert!(worktree.path.exists());
    assert_eq!(worktree.status, WorktreeStatus::Active);
    assert!(worktree.epic_id.is_none());
    assert_eq!(worktree.branch, "factory/swift-fox");
}

#[test]
fn test_ensure_worker_worktree_creates_new() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("calm-owl").unwrap();

    assert!(worktree.path.exists());
    assert_eq!(worktree.branch, "factory/calm-owl");
}

#[test]
fn test_ensure_worker_worktree_reuses_existing() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let path1 = manager
        .ensure_worker_worktree("swift-fox")
        .unwrap()
        .path
        .clone();

    let path2 = manager
        .ensure_worker_worktree("swift-fox")
        .unwrap()
        .path
        .clone();

    assert_eq!(path1, path2);
}

#[test]
fn test_worker_cwds() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    manager.ensure_worker_worktree("swift-fox").unwrap();
    manager.ensure_worker_worktree("calm-owl").unwrap();

    let cwds = manager.worker_cwds();

    assert_eq!(cwds.len(), 2);
    assert!(cwds.contains_key("swift-fox"));
    assert!(cwds.contains_key("calm-owl"));
}

#[test]
fn test_cleanup_workers() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let path1 = manager
        .ensure_worker_worktree("swift-fox")
        .unwrap()
        .path
        .clone();
    let path2 = manager
        .ensure_worker_worktree("calm-owl")
        .unwrap()
        .path
        .clone();

    assert!(path1.exists());
    assert!(path2.exists());

    let report = manager.cleanup_workers(false).unwrap();

    assert_eq!(report.cleaned.len(), 2);
    assert!(report.dirty_deferred.is_empty());
    assert!(!path1.exists());
    assert!(!path2.exists());
    assert!(manager.worker_cwds().is_empty());
}

#[test]
fn test_remove_single_worker() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let path1 = manager
        .ensure_worker_worktree("swift-fox")
        .unwrap()
        .path
        .clone();
    manager.ensure_worker_worktree("calm-owl").unwrap();

    manager.remove_worker("swift-fox", false).unwrap();

    assert!(!path1.exists());
    assert_eq!(manager.worker_cwds().len(), 1);
    assert!(manager.worker_cwds().contains_key("calm-owl"));
}

#[test]
fn test_slugify_title() {
    assert_eq!(slugify_title("Add User Auth"), "add-user-auth");
    assert_eq!(slugify_title("CAS v1"), "cas-v1");
    assert_eq!(slugify_title("Fix bug #123"), "fix-bug-123");
    assert_eq!(slugify_title("  Multiple   Spaces  "), "multiple-spaces");
    assert_eq!(
        slugify_title("Special!@#$%^&*()Characters"),
        "special-characters"
    );
    let long_title = "A".repeat(100);
    assert_eq!(slugify_title(&long_title).len(), 50);
}

#[test]
fn test_create_epic_branch() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let branch = manager
        .create_epic_branch("Add User Authentication")
        .unwrap();

    assert_eq!(branch, "epic/add-user-authentication");
    assert!(manager.git.branch_exists(&branch).unwrap());
}

#[test]
fn test_create_epic_branch_uses_trunk_not_current_head() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let trunk = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    let trunk = String::from_utf8_lossy(&trunk.stdout).trim().to_string();
    let trunk_sha = Command::new("git")
        .args(["rev-parse", &trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    let trunk_sha = String::from_utf8_lossy(&trunk_sha.stdout)
        .trim()
        .to_string();

    Command::new("git")
        .args(["checkout", "-q", "-b", "feature/supervisor-head"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    std::fs::write(repo_path.join("feature.txt"), "feature-only").unwrap();
    Command::new("git")
        .args(["add", "feature.txt"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "feature-only"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();
    let branch = manager.create_epic_branch("Base Regression").unwrap();
    let epic_sha = Command::new("git")
        .args(["rev-parse", &branch])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    let epic_sha = String::from_utf8_lossy(&epic_sha.stdout).trim().to_string();

    assert_eq!(
        epic_sha, trunk_sha,
        "epic branch must be created from trunk {trunk}, not current feature HEAD"
    );
}

#[test]
fn test_create_epic_branch_idempotent() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let branch1 = manager.create_epic_branch("Test Epic").unwrap();
    let branch2 = manager.create_epic_branch("Test Epic").unwrap();

    assert_eq!(branch1, branch2);
    assert_eq!(branch1, "epic/test-epic");
}

#[test]
fn test_merge_workers_to_epic() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Test Merge").unwrap();

    let worktree = manager.create_for_worker("merge-worker").unwrap();

    std::fs::write(worktree.path.join("worker-file.txt"), "worker content").unwrap();
    Command::new("git")
        .args(["add", "worker-file.txt"])
        .current_dir(&worktree.path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Worker commit"])
        .current_dir(&worktree.path)
        .output()
        .unwrap();

    let results = manager.merge_workers_to_epic(&epic_branch).unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].1, "Merge should succeed");

    manager.git.checkout(&epic_branch).unwrap();
    assert!(repo_path.join("worker-file.txt").exists());
}

/// cas-369f: mid-session merge with cleanup=false leaves worktree + branch.
#[test]
fn merge_and_cleanup_preserve_leaves_worktree_and_branch() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    config.cleanup_on_close = true; // config would clean — caller opts out
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Preserve Merge").unwrap();
    let mut worktree = manager.create_for_worker("preserve-worker").unwrap();
    let wt_path = worktree.path.clone();
    let worker_branch = worktree.branch.clone();

    std::fs::write(wt_path.join("mid-epic.txt"), "still working").unwrap();
    Command::new("git")
        .args(["add", "mid-epic.txt"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "mid-epic work"])
        .current_dir(&wt_path)
        .output()
        .unwrap();

    worktree.parent_branch = epic_branch.clone();
    let commit = manager
        .merge_and_cleanup(&mut worktree, false, false)
        .expect("merge preserve");
    assert!(commit.is_some());

    assert!(
        wt_path.exists(),
        "worktree path must remain when cleanup=false (mid-session)"
    );
    assert!(
        manager.git.branch_exists(&worker_branch).unwrap(),
        "factory branch must remain when cleanup=false"
    );
    manager.git.checkout(&epic_branch).unwrap();
    assert!(
        repo_path.join("mid-epic.txt").exists(),
        "merge content must land on parent"
    );
}

/// cas-369f: cleanup=true still consumes the worktree after merge.
#[test]
fn merge_and_cleanup_true_removes_worktree() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Cleanup Merge").unwrap();
    let mut worktree = manager.create_for_worker("cleanup-merge-worker").unwrap();
    let wt_path = worktree.path.clone();
    let worker_branch = worktree.branch.clone();

    std::fs::write(wt_path.join("done.txt"), "lane done").unwrap();
    Command::new("git")
        .args(["add", "done.txt"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "done"])
        .current_dir(&wt_path)
        .output()
        .unwrap();

    worktree.parent_branch = epic_branch.clone();
    manager
        .merge_and_cleanup(&mut worktree, false, true)
        .expect("merge cleanup");

    assert!(
        !wt_path.exists(),
        "worktree must be removed when cleanup=true"
    );
    assert!(
        !manager.git.branch_exists(&worker_branch).unwrap(),
        "branch must be deleted when cleanup=true"
    );
}

/// cas-369f: force=true on dirty tree merges without implying cleanup.
#[test]
fn merge_force_dirty_does_not_remove_when_cleanup_false() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Force Dirty").unwrap();
    let mut worktree = manager.create_for_worker("force-dirty-worker").unwrap();
    let wt_path = worktree.path.clone();

    std::fs::write(wt_path.join("committed.txt"), "c").unwrap();
    Command::new("git")
        .args(["add", "committed.txt"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "committed"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    // Genuine uncommitted work: a tracked file modified after commit.
    // cas-006c: untracked-only no longer blocks a force-free merge, so this
    // must be a tracked modification to still exercise the force-required
    // path this test is about.
    std::fs::write(wt_path.join("committed.txt"), "modified after commit").unwrap();

    worktree.parent_branch = epic_branch;
    // Without force, dirty fails
    assert!(manager
        .merge_and_cleanup(&mut worktree, false, false)
        .is_err());
    // force=true merges dirty; cleanup=false keeps path
    manager
        .merge_and_cleanup(&mut worktree, true, false)
        .expect("force dirty merge preserve");
    assert!(wt_path.exists(), "force must not imply cleanup");
}

#[test]
fn test_cleanup_worker_branches_after_merge() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Cleanup Test").unwrap();

    let worktree = manager.create_for_worker("cleanup-worker").unwrap();
    let worker_branch = worktree.branch.clone();

    std::fs::write(worktree.path.join("cleanup-file.txt"), "cleanup content").unwrap();
    Command::new("git")
        .args(["add", "cleanup-file.txt"])
        .current_dir(&worktree.path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Cleanup commit"])
        .current_dir(&worktree.path)
        .output()
        .unwrap();

    manager.merge_workers_to_epic(&epic_branch).unwrap();

    assert!(
        manager.git.branch_exists(&worker_branch).unwrap(),
        "Worker branch should exist"
    );

    assert!(
        manager
            .is_branch_merged(&worker_branch, &epic_branch)
            .unwrap(),
        "Worker branch should be merged into epic branch"
    );

    manager.remove_worker("cleanup-worker", true).unwrap();

    assert!(
        !manager.git.branch_exists(&worker_branch).unwrap(),
        "Worker branch should be deleted by remove_worker"
    );
}

#[test]
fn test_is_branch_merged() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let current = manager.git.current_branch().unwrap();
    Command::new("git")
        .args(["checkout", "-b", "test-merged"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    std::fs::write(repo_path.join("merged-file.txt"), "merged").unwrap();
    Command::new("git")
        .args(["add", "merged-file.txt"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Merged commit"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["checkout", &current])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["merge", "test-merged", "--no-ff", "-m", "Merge test-merged"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    assert!(manager.is_branch_merged("test-merged", &current).unwrap());

    Command::new("git")
        .args(["checkout", "-b", "test-unmerged"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    std::fs::write(repo_path.join("unmerged-file.txt"), "unmerged").unwrap();
    Command::new("git")
        .args(["add", "unmerged-file.txt"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Unmerged commit"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["checkout", &current])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    assert!(!manager.is_branch_merged("test-unmerged", &current).unwrap());
}

#[test]
fn test_attempt_remove_worker_clean_removes_tree_and_branch() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let path = manager
        .ensure_worker_worktree("clean-wolf")
        .unwrap()
        .path
        .clone();
    let branch = manager.branch_name_for_worker("clean-wolf");

    assert!(path.exists());
    assert!(manager.git.branch_exists(&branch).unwrap());

    let outcome = manager.attempt_remove_worker("clean-wolf").unwrap();

    assert_eq!(outcome, RemoveOutcome::Removed);
    assert!(!path.exists());
    assert!(!manager.git.branch_exists(&branch).unwrap());
    assert!(manager.get_worker("clean-wolf").is_none());
}

#[test]
fn test_attempt_remove_worker_dirty_modified_defers() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("dirty-hawk").unwrap();
    let path = worktree.path.clone();
    let branch = worktree.branch.clone();

    // Modify the tracked README to create an uncommitted change
    std::fs::write(path.join("README.md"), "# Modified").unwrap();

    let outcome = manager.attempt_remove_worker("dirty-hawk").unwrap();

    match outcome {
        RemoveOutcome::DirtyDeferred(warning) => {
            assert_eq!(warning.worker_name, "dirty-hawk");
            assert_eq!(warning.path, path);
            assert!(warning.file_count >= 1);
        }
        other => panic!("expected DirtyDeferred, got {other:?}"),
    }

    assert!(path.exists(), "dirty tree must be preserved");
    assert!(manager.git.branch_exists(&branch).unwrap());
    assert!(
        manager.get_worker("dirty-hawk").is_some(),
        "manager must keep tracking the dirty worker so a reaper can pick it up"
    );
}

#[test]
fn test_attempt_remove_worker_untracked_files_defer() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("spike-lynx").unwrap();
    let path = worktree.path.clone();

    // Untracked file only
    std::fs::write(path.join("scratch.txt"), "draft").unwrap();

    let outcome = manager.attempt_remove_worker("spike-lynx").unwrap();

    assert!(
        matches!(outcome, RemoveOutcome::DirtyDeferred(_)),
        "untracked-only worktrees must be treated as dirty"
    );
    assert!(path.exists());
}

#[test]
fn test_attempt_remove_worker_not_tracked() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let outcome = manager.attempt_remove_worker("never-spawned").unwrap();
    assert_eq!(outcome, RemoveOutcome::NotTracked);
}

#[test]
fn test_cleanup_workers_non_force_reports_dirty_deferred() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();

    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let clean_path = manager
        .ensure_worker_worktree("tidy-cat")
        .unwrap()
        .path
        .clone();
    let dirty_path = manager
        .ensure_worker_worktree("messy-dog")
        .unwrap()
        .path
        .clone();

    std::fs::write(dirty_path.join("wip.txt"), "in progress").unwrap();

    let report = manager.cleanup_workers(false).unwrap();

    assert_eq!(report.cleaned, vec!["tidy-cat".to_string()]);
    assert_eq!(report.dirty_deferred.len(), 1);
    assert_eq!(report.dirty_deferred[0].worker_name, "messy-dog");
    assert_eq!(report.dirty_deferred[0].path, dirty_path);
    assert!(report.dirty_deferred[0].file_count >= 1);

    assert!(!clean_path.exists());
    assert!(dirty_path.exists(), "dirty tree must survive non-force cleanup");
}

// --- cas-b082: epic-branch base resolution (fetch-before-branch + config) --

/// Write `.cas/config.toml` with `[factory] epic_base_branch = "<branch>"`
/// under `repo_root`, creating the `.cas` dir if needed.
fn write_epic_base_branch_config(repo_root: &std::path::Path, branch: &str) {
    let cas_dir = repo_root.join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    std::fs::write(
        cas_dir.join("config.toml"),
        format!("[factory]\nepic_base_branch = \"{branch}\"\n"),
    )
    .unwrap();
}

/// Bare "origin" repo plus a local clone tracking it — a real git remote
/// setup (unlike `create_test_repo`'s local-only repo), so fetch-before-branch
/// behavior can be exercised. Returns (tempdir, origin bare path, local clone path).
fn create_repo_with_origin() -> (TempDir, PathBuf, PathBuf) {
    let temp = TempDir::new().unwrap();
    let origin_path = temp.path().join("origin.git");
    let local_path = temp.path().join("local");

    Command::new("git")
        .args(["init", "--bare", "-b", "main"])
        .arg(&origin_path)
        .output()
        .unwrap();

    Command::new("git")
        .args([
            "clone",
            origin_path.to_str().unwrap(),
            local_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    for (key, value) in [("user.email", "test@test.com"), ("user.name", "Test")] {
        Command::new("git")
            .args(["config", key, value])
            .current_dir(&local_path)
            .output()
            .unwrap();
    }

    std::fs::write(local_path.join("README.md"), "# Test").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&local_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&local_path)
        .output()
        .unwrap();

    (temp, origin_path, local_path)
}

/// Push `count` extra commits to `origin_path`'s `main` from a fresh clone,
/// simulating upstream moving ahead while `local_path` never fetches —
/// the exact BUG-epic-branch-stale-local-base-2026-07-08 scenario.
fn advance_origin_main(temp: &TempDir, origin_path: &std::path::Path, count: usize) {
    let advancer_path = temp.path().join("advancer");
    Command::new("git")
        .args([
            "clone",
            origin_path.to_str().unwrap(),
            advancer_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    for (key, value) in [("user.email", "test@test.com"), ("user.name", "Test")] {
        Command::new("git")
            .args(["config", key, value])
            .current_dir(&advancer_path)
            .output()
            .unwrap();
    }
    for i in 0..count {
        std::fs::write(advancer_path.join(format!("upstream-{i}.txt")), "x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&advancer_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", &format!("upstream commit {i}")])
            .current_dir(&advancer_path)
            .output()
            .unwrap();
    }
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&advancer_path)
        .output()
        .unwrap();
}

#[test]
fn test_create_epic_branch_fetches_and_uses_remote_tip_when_local_base_stale() {
    let (temp, origin_path, local_path) = create_repo_with_origin();
    // origin/main moves 3 commits ahead; local_path's tracking ref is stale
    // because it never fetches before create_epic_branch runs.
    advance_origin_main(&temp, &origin_path, 3);

    let config = WorktreeConfig::default();
    let manager = WorktreeManager::new(&local_path, config).unwrap();

    let stale_local_main_sha = manager.git().ref_sha("main").unwrap();
    let branch = manager
        .create_epic_branch("Stale Base Regression")
        .unwrap();
    let epic_sha = manager.git().ref_sha(&branch).unwrap();

    assert_ne!(
        epic_sha, stale_local_main_sha,
        "epic branch must not be cut from the stale local base — it must include \
         the 3 commits origin/main gained after clone"
    );

    // The 3 upstream-only commits must be reachable from the epic branch.
    let epic_has_upstream_files = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", &branch])
        .current_dir(&local_path)
        .output()
        .unwrap();
    let epic_tree = String::from_utf8_lossy(&epic_has_upstream_files.stdout);
    for i in 0..3 {
        assert!(
            epic_tree.contains(&format!("upstream-{i}.txt")),
            "epic branch tree must contain upstream-only file {i} fetched from origin/main"
        );
    }
}

#[test]
fn test_create_epic_branch_honors_configured_epic_base_branch() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    // Capture the manager's own view of the trunk (whatever this system's
    // git default-branch happens to be — "main" or "master") before adding
    // a divergent branch, so the test doesn't hardcode either name.
    let detected_trunk = WorktreeManager::new(&repo_path, config)
        .unwrap()
        .git()
        .detect_default_branch();

    // Create a "staging" branch one commit ahead of the detected default
    // branch, and point [factory] epic_base_branch at it.
    Command::new("git")
        .args(["checkout", "-q", "-b", "staging"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    std::fs::write(repo_path.join("staging-only.txt"), "staging").unwrap();
    Command::new("git")
        .args(["add", "staging-only.txt"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "staging-only commit"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["checkout", "-q", &detected_trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    write_epic_base_branch_config(&repo_path, "staging");

    let config = WorktreeConfig::default();
    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let staging_sha = manager.git().ref_sha("staging").unwrap();
    let branch = manager.create_epic_branch("Configured Base").unwrap();
    let epic_sha = manager.git().ref_sha(&branch).unwrap();

    assert_eq!(
        epic_sha, staging_sha,
        "epic branch must be cut from the configured epic_base_branch (staging), \
         not the repo-detected default branch ({detected_trunk})"
    );
}

// --- cas-006c: merge/removal dirty-check classification --------------------

/// AC1: a worktree whose only dirty entry is the CAS-generated
/// `.husky/_/` artifact merges WITHOUT force: true.
#[test]
fn merge_and_cleanup_husky_artifact_only_merges_without_force() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Husky Noise").unwrap();
    let mut worktree = manager.create_for_worker("husky-worker").unwrap();
    let wt_path = worktree.path.clone();

    // Exactly the false-positive from the bug report: an untracked
    // `.husky/_/` directory the worker startup hook creates itself.
    std::fs::create_dir_all(wt_path.join(".husky/_")).unwrap();
    std::fs::write(wt_path.join(".husky/_/husky.sh"), "# shim").unwrap();

    worktree.parent_branch = epic_branch;
    let result = manager.merge_and_cleanup(&mut worktree, false, false);

    assert!(
        result.is_ok(),
        "husky artifact alone must not require force: {result:?}"
    );
}

/// AC2: a worktree with modified tracked files still blocks without force,
/// and the error names the offending path with its status.
#[test]
fn merge_and_cleanup_modified_tracked_file_blocks_and_names_path() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Real Wip").unwrap();
    let mut worktree = manager.create_for_worker("wip-worker").unwrap();
    let wt_path = worktree.path.clone();

    // Real, uncommitted, tracked modification.
    std::fs::write(wt_path.join("README.md"), "# real uncommitted work").unwrap();

    worktree.parent_branch = epic_branch;
    let err = manager
        .merge_and_cleanup(&mut worktree, false, false)
        .expect_err("modified tracked file must still block without force");

    let message = err.to_string();
    assert!(
        message.contains("README.md"),
        "error must name the offending path: {message}"
    );
}

/// AC1/AC2 via `remove_worker` (worker_ops.rs:263 — the other named call
/// site in the bug report).
#[test]
fn remove_worker_husky_artifact_only_removes_without_force() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("husky-remove").unwrap();
    let path = worktree.path.clone();

    std::fs::create_dir_all(path.join(".husky/_")).unwrap();
    std::fs::write(path.join(".husky/_/husky.sh"), "# shim").unwrap();

    manager
        .remove_worker("husky-remove", false)
        .expect("husky artifact alone must not require force");

    assert!(!path.exists());
}

#[test]
fn remove_worker_modified_tracked_file_blocks_and_names_path() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("wip-remove").unwrap();
    let path = worktree.path.clone();

    std::fs::write(path.join("README.md"), "# real uncommitted work").unwrap();

    let err = manager
        .remove_worker("wip-remove", false)
        .expect_err("modified tracked file must still block without force");

    let message = err.to_string();
    assert!(
        message.contains("README.md"),
        "error must name the offending path: {message}"
    );
    assert!(path.exists(), "blocked removal must leave the worktree intact");
}

#[test]
fn test_create_epic_branch_without_config_still_defaults_to_detected_trunk() {
    // No .cas/config.toml at all — epic_base_branch must default to None,
    // falling back to detect_default_branch() exactly as before cas-b082.
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let detected_trunk = manager.git().detect_default_branch();
    let trunk_sha = manager.git().ref_sha(&detected_trunk).unwrap();
    let branch = manager.create_epic_branch("Default Base").unwrap();
    let epic_sha = manager.git().ref_sha(&branch).unwrap();

    assert_eq!(epic_sha, trunk_sha);
}
