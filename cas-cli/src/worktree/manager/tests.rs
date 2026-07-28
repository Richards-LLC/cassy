use crate::types::WorktreeStatus;
use crate::worktree::git::GitError;
use crate::worktree::manager::worker_ops::RemoveOutcome;
use crate::worktree::manager::*;
use std::path::{Path, PathBuf};
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
    commit_tracked_husky_dir(&repo_path);
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

/// Same as above but with `cleanup=true` (the worktree directory is
/// actually deleted) — proves `.husky/_/` is genuinely *excluded*, not just
/// let through by cleanup=false's warn-only leniency for untracked paths.
#[test]
fn merge_and_cleanup_husky_artifact_only_merges_and_removes_without_force() {
    let (_temp, repo_path) = create_test_repo();
    commit_tracked_husky_dir(&repo_path);
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Husky Noise Cleanup").unwrap();
    let mut worktree = manager.create_for_worker("husky-cleanup-worker").unwrap();
    let wt_path = worktree.path.clone();

    std::fs::create_dir_all(wt_path.join(".husky/_")).unwrap();
    std::fs::write(wt_path.join(".husky/_/husky.sh"), "# shim").unwrap();

    worktree.parent_branch = epic_branch;
    let result = manager.merge_and_cleanup(&mut worktree, false, true);

    assert!(
        result.is_ok(),
        "husky artifact alone must not require force, even when cleanup removes the tree: {result:?}"
    );
    assert!(!wt_path.exists(), "cleanup=true must still remove the worktree");
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

/// Commit a tracked `.husky/pre-commit` placeholder before a worktree is
/// created off this repo, so `.husky/` itself is already known to git —
/// matching real repos where husky's tracked hook scripts are committed.
/// Only the `_` runner subdir is left untracked afterward, which is why git
/// reports it individually (`?? .husky/_/`) instead of collapsing the whole
/// `.husky/` directory into one untracked entry.
fn commit_tracked_husky_dir(repo_path: &std::path::Path) {
    std::fs::create_dir_all(repo_path.join(".husky")).unwrap();
    std::fs::write(repo_path.join(".husky/pre-commit"), "#!/bin/sh\n").unwrap();
    Command::new("git")
        .args(["add", ".husky/pre-commit"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add husky pre-commit hook"])
        .current_dir(repo_path)
        .output()
        .unwrap();
}

/// AC1/AC2 via `remove_worker` (worker_ops.rs:263 — the other named call
/// site in the bug report).
#[test]
fn remove_worker_husky_artifact_only_removes_without_force() {
    let (_temp, repo_path) = create_test_repo();
    commit_tracked_husky_dir(&repo_path);
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

/// Supervisor review regression (cas-006c): removal DESTROYS anything git
/// never tracked — there is no blob, no index entry, no reflog for an
/// untracked file once its containing worktree directory is deleted. The
/// first cut of this fix made untracked-only dirt warning-only everywhere,
/// including on the removal path, which would have silently destroyed a
/// worker's uncommitted-but-not-yet-`git add`-ed file. `remove_worker` must
/// still refuse on a bare untracked non-CAS file, and the file must survive
/// the refusal.
#[test]
fn remove_worker_untracked_non_cas_file_blocks_and_file_survives() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("untracked-remove").unwrap();
    let path = worktree.path.clone();
    let new_file = path.join("new_module.rs");

    // Real, uncommitted, never-`git add`-ed work — exists ONLY in this
    // worktree directory.
    std::fs::write(&new_file, "pub fn not_yet_committed() {}").unwrap();

    let err = manager
        .remove_worker("untracked-remove", false)
        .expect_err("an untracked non-CAS file must still block removal — it would be destroyed");

    assert!(
        err.to_string().contains("new_module.rs"),
        "error must name the offending untracked path: {err}"
    );
    assert!(path.exists(), "worktree must survive the refusal");
    assert!(
        new_file.exists(),
        "the untracked file itself must survive — it cannot be recovered once removal destroys it"
    );
}

/// Same regression, exercised via `attempt_remove_worker` (the graceful
/// shutdown path `finalize_worker_worktree` actually calls in production —
/// see ui/factory/app/render_and_ops/epic_workers.rs).
#[test]
fn attempt_remove_worker_untracked_non_cas_file_defers_and_file_survives() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("untracked-defer").unwrap();
    let path = worktree.path.clone();
    let new_file = path.join("new_module.rs");
    std::fs::write(&new_file, "pub fn not_yet_committed() {}").unwrap();

    let outcome = manager.attempt_remove_worker("untracked-defer").unwrap();

    assert!(
        matches!(outcome, RemoveOutcome::DirtyDeferred(_)),
        "untracked-only worktrees must still defer, not silently remove: {outcome:?}"
    );
    assert!(path.exists());
    assert!(new_file.exists(), "the untracked file must survive");
}

/// Same regression via `merge_and_cleanup(cleanup=true)` — the merge
/// succeeds (the branch itself has nothing to lose), but the worktree
/// directory removal must still refuse while an untracked non-CAS file
/// sits in it.
#[test]
fn merge_and_cleanup_untracked_non_cas_file_blocks_when_cleanup_true() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Untracked Cleanup").unwrap();
    let mut worktree = manager.create_for_worker("untracked-cleanup-worker").unwrap();
    let wt_path = worktree.path.clone();
    let new_file = wt_path.join("new_module.rs");
    std::fs::write(&new_file, "pub fn not_yet_committed() {}").unwrap();

    worktree.parent_branch = epic_branch;
    let err = manager
        .merge_and_cleanup(&mut worktree, false, true)
        .expect_err("cleanup=true must block on an untracked file it would destroy");

    assert!(
        err.to_string().contains("new_module.rs"),
        "error must name the offending untracked path: {err}"
    );
    assert!(wt_path.exists(), "worktree must survive the refusal");
    assert!(new_file.exists(), "the untracked file must survive");
}

/// Contrast case: the SAME untracked file must NOT block when the worktree
/// is preserved (cleanup=false) — nothing is destroyed by a merge that
/// leaves the directory in place, which is the intentional cas-006c
/// behavior this fix must not regress.
#[test]
fn merge_and_cleanup_untracked_non_cas_file_warns_not_blocks_when_cleanup_false() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let epic_branch = manager.create_epic_branch("Untracked Preserve").unwrap();
    let mut worktree = manager.create_for_worker("untracked-preserve-worker").unwrap();
    let wt_path = worktree.path.clone();
    let new_file = wt_path.join("new_module.rs");
    std::fs::write(&new_file, "pub fn not_yet_committed() {}").unwrap();

    worktree.parent_branch = epic_branch;
    manager
        .merge_and_cleanup(&mut worktree, false, false)
        .expect("cleanup=false must not block on untracked-only dirt — nothing is destroyed");

    assert!(wt_path.exists());
    assert!(new_file.exists());
}

/// Same regression via `abandon`, which — like remove_worker — always
/// deletes the worktree directory.
#[test]
fn abandon_untracked_non_cas_file_blocks_and_file_survives() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig {
        enabled: true,
        ..Default::default()
    };
    let manager = WorktreeManager::new(&repo_path, config).unwrap();

    let mut worktree = manager
        .create_for_epic("cas-epic-untracked-abandon", None)
        .unwrap();
    let wt_path = worktree.path.clone();
    let new_file = wt_path.join("new_module.rs");
    std::fs::write(&new_file, "pub fn not_yet_committed() {}").unwrap();

    let err = manager
        .abandon(&mut worktree, false)
        .expect_err("abandon must block on an untracked file it would destroy");

    assert!(
        err.to_string().contains("new_module.rs"),
        "error must name the offending untracked path: {err}"
    );
    assert!(wt_path.exists(), "worktree must survive the refusal");
    assert!(new_file.exists(), "the untracked file must survive");
}

// --- cas-df97: worktree removal must not destroy live external symlink targets ---

/// AC1/AC2 end-to-end: a real external symlink (outside the worktree,
/// resolved via a scoped `$HOME`) pointing into the worktree blocks
/// `remove_worker`, is surfaced through the existing `WorktreeError` type
/// (not a parallel mechanism), and the worktree — and the symlink's live
/// target — both survive the refusal.
#[test]
fn remove_worker_external_symlink_blocks_and_survives() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("symlink-remove").unwrap();
    let path = worktree.path.clone();
    let real_file = path.join("dotfile-target.txt");
    std::fs::write(&real_file, "live config").unwrap();

    crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
        let link = home.join(".dotfile-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        let err = manager
            .remove_worker("symlink-remove", false)
            .expect_err("a live external symlink into the worktree must block removal");

        assert!(
            matches!(err, WorktreeError::ExternalSymlinksDetected(_)),
            "must surface through the existing WorktreeError type: {err:?}"
        );
        assert!(
            err.to_string().contains(&link.display().to_string()),
            "error must name the offending link: {err}"
        );
        assert!(link.exists(), "the external symlink itself must still resolve");
    });

    assert!(path.exists(), "worktree must survive the refusal");
    assert!(real_file.exists(), "the symlink's live target must survive");
}

/// Same guard via `attempt_remove_worker` — the actual production path
/// `finalize_worker_worktree` calls on graceful worker shutdown (see
/// ui/factory/app/render_and_ops/epic_workers.rs), surfaced through
/// `RemoveOutcome` per AC2.
#[test]
fn attempt_remove_worker_external_symlink_blocks_and_survives() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("symlink-attempt").unwrap();
    let path = worktree.path.clone();
    let real_file = path.join("dotfile-target.txt");
    std::fs::write(&real_file, "live config").unwrap();

    crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
        let link = home.join(".dotfile-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        let outcome = manager.attempt_remove_worker("symlink-attempt").unwrap();

        match outcome {
            RemoveOutcome::ExternalSymlinksBlocked(warning) => {
                assert_eq!(warning.worker_name, "symlink-attempt");
                assert_eq!(warning.links.len(), 1);
                assert_eq!(warning.links[0].link, link);
            }
            other => panic!("expected ExternalSymlinksBlocked, got {other:?}"),
        }
        assert!(link.exists());
    });

    assert!(path.exists(), "worktree must survive the refusal");
    assert!(
        manager.get_worker("symlink-attempt").is_some(),
        "manager must keep tracking the blocked worker so it can be retried later"
    );
    assert!(real_file.exists());
}

/// Same guard via `cleanup_workers`, surfaced through
/// `CleanupReport::external_symlinks_blocked` per AC2 — and proven
/// independent of `force`, since force means "bypass git dirty-tree
/// protection", not "destroy my $HOME symlinks".
#[test]
fn cleanup_workers_external_symlink_blocks_even_with_force() {
    let (_temp, repo_path) = create_test_repo();
    let config = WorktreeConfig::default();
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    let worktree = manager.ensure_worker_worktree("symlink-cleanup").unwrap();
    let path = worktree.path.clone();
    let real_file = path.join("dotfile-target.txt");
    std::fs::write(&real_file, "live config").unwrap();

    crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
        let link = home.join(".dotfile-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        let report = manager.cleanup_workers(true).unwrap();

        assert!(report.cleaned.is_empty());
        assert_eq!(report.external_symlinks_blocked.len(), 1);
        assert_eq!(report.external_symlinks_blocked[0].worker_name, "symlink-cleanup");
        assert_eq!(report.external_symlinks_blocked[0].links[0].link, link);
        assert!(link.exists());
    });

    assert!(path.exists(), "worktree must survive even force=true cleanup");
    assert!(real_file.exists());
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

// --- cas-e18f: conflicting worktree_merge must not leave the shared
// checkout mid-merge -------------------------------------------------------

fn commit_file(repo_or_worktree_path: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(repo_or_worktree_path.join(name), content).unwrap();
    Command::new("git")
        .args(["add", name])
        .current_dir(repo_or_worktree_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(repo_or_worktree_path)
        .output()
        .unwrap();
}

/// AC1 + AC3: a genuinely conflicting merge is refused with the conflicting
/// path named, and leaves the shared checkout exactly as it was before the
/// attempt — no `MERGE_HEAD`, no staged conflict markers.
#[test]
fn merge_and_cleanup_conflict_leaves_checkout_clean_and_names_path() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    commit_file(&repo_path, "shared.txt", "base\n", "add shared.txt");

    let epic_branch = manager.create_epic_branch("Conflict Merge").unwrap();
    manager.git().checkout(&epic_branch).unwrap();

    let mut worktree = manager.create_for_worker("conflict-worker").unwrap();
    let wt_path = worktree.path.clone();
    commit_file(&wt_path, "shared.txt", "worker-version\n", "worker edit");

    // Diverge the epic branch itself so the merge has real content to
    // reconcile (both sides touched the same line).
    commit_file(&repo_path, "shared.txt", "epic-version\n", "epic edit");

    let pre_head = manager.git().ref_sha(&epic_branch).unwrap();

    worktree.parent_branch = epic_branch.clone();
    let err = manager
        .merge_and_cleanup(&mut worktree, false, false)
        .expect_err("conflicting content must fail the merge");

    match &err {
        WorktreeError::Git(GitError::MergeConflictPaths(paths)) => {
            assert!(
                paths.iter().any(|p| p == "shared.txt"),
                "conflict error must name shared.txt, got {paths:?}"
            );
        }
        other => panic!("expected MergeConflictPaths naming shared.txt, got {other:?}"),
    }

    // No trace of the failed merge: no MERGE_HEAD, no staged conflict, HEAD
    // unchanged, working tree clean.
    assert!(
        !manager.git().merge_in_progress(),
        "a failed merge must leave no MERGE_HEAD behind"
    );
    // Only tracked entries matter here — the worktree store/config under
    // `.cas/` is untracked scaffolding the manager itself creates and is
    // present regardless of merge outcome; the assertion is about the
    // *merge* leaving no trace (no staged conflict, no modified tracked
    // files), not about a pristine `git status` overall.
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    let tracked_changes: Vec<String> = String::from_utf8_lossy(&status.stdout)
        .lines()
        .filter(|line| !line.starts_with("??"))
        .map(|line| line.to_string())
        .collect();
    assert!(
        tracked_changes.is_empty(),
        "shared checkout must have no tracked/staged changes after a failed merge, got: {tracked_changes:?}"
    );
    assert_eq!(
        manager.git().ref_sha(&epic_branch).unwrap(),
        pre_head,
        "HEAD of the target branch must be unchanged by a failed merge"
    );
}

/// AC2: an unrelated, non-conflicting merge must succeed immediately after a
/// conflicting one failed — the whole point of aborting on failure is that
/// it doesn't cascade into blocking every other merge behind it.
#[test]
fn merge_and_cleanup_unrelated_merge_succeeds_after_prior_conflict() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    commit_file(&repo_path, "shared.txt", "base\n", "add shared.txt");

    let epic_branch = manager.create_epic_branch("Cascade Merge").unwrap();
    manager.git().checkout(&epic_branch).unwrap();

    let mut conflicting = manager.create_for_worker("cascade-conflict-worker").unwrap();
    let conflict_path = conflicting.path.clone();
    commit_file(&conflict_path, "shared.txt", "worker-version\n", "worker edit");
    commit_file(&repo_path, "shared.txt", "epic-version\n", "epic edit");

    conflicting.parent_branch = epic_branch.clone();
    manager
        .merge_and_cleanup(&mut conflicting, false, false)
        .expect_err("first merge must conflict");

    // A second, unrelated worker touching a different file must merge
    // cleanly right away — no residual MERGE_HEAD blocking it.
    manager.git().checkout(&epic_branch).unwrap();
    let mut unrelated = manager.create_for_worker("cascade-unrelated-worker").unwrap();
    let unrelated_path = unrelated.path.clone();
    commit_file(&unrelated_path, "unrelated.txt", "fine\n", "unrelated work");

    unrelated.parent_branch = epic_branch.clone();
    let commit = manager
        .merge_and_cleanup(&mut unrelated, false, false)
        .expect("unrelated merge must succeed immediately after the prior conflict");
    assert!(commit.is_some());

    manager.git().checkout(&epic_branch).unwrap();
    assert!(repo_path.join("unrelated.txt").exists());
}

/// AC4: a `MERGE_HEAD` already present on entry (e.g. left by some other
/// process, or a bug that predates this fix) must be reported as its own
/// distinct error rather than surfacing as an opaque failure on whatever
/// unrelated merge happens to run next.
#[test]
fn merge_and_cleanup_detects_pre_existing_merge_in_progress() {
    let (_temp, repo_path) = create_test_repo();
    let mut config = WorktreeConfig::default();
    config.auto_merge = true;
    let mut manager = WorktreeManager::new(&repo_path, config).unwrap();

    commit_file(&repo_path, "shared.txt", "base\n", "add shared.txt");

    let epic_branch = manager.create_epic_branch("Preexisting Merge").unwrap();
    manager.git().checkout(&epic_branch).unwrap();

    let mut worktree = manager.create_for_worker("preexisting-merge-worker").unwrap();
    let wt_path = worktree.path.clone();
    commit_file(&wt_path, "unrelated.txt", "fine\n", "unrelated work");
    worktree.parent_branch = epic_branch.clone();

    // Simulate a merge left mid-flight by something other than
    // `merge_and_cleanup` itself: start a real conflicting merge directly
    // and leave it unresolved (no --abort).
    commit_file(&repo_path, "shared.txt", "epic-version\n", "epic edit");
    let stray_branch = "stray-conflicting-branch";
    Command::new("git")
        .args(["checkout", "-b", stray_branch])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    commit_file(&repo_path, "shared.txt", "stray-version\n", "stray edit");
    manager.git().checkout(&epic_branch).unwrap();
    // Diverge epic_branch too, past the common ancestor with stray_branch,
    // so the two sides actually conflict on the same line instead of one
    // being a fast-forward-able descendant of the other.
    commit_file(&repo_path, "shared.txt", "epic-version-2\n", "further epic edit");
    let merge_output = Command::new("git")
        .args(["merge", "--no-ff", stray_branch])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    assert!(!merge_output.status.success(), "setup merge must conflict");
    assert!(
        manager.git().merge_in_progress(),
        "setup must leave MERGE_HEAD in place (no abort) to simulate the pre-existing state"
    );

    let err = manager
        .merge_and_cleanup(&mut worktree, false, false)
        .expect_err("entry must refuse when a merge is already in progress");

    assert!(
        matches!(err, WorktreeError::Git(GitError::MergeInProgress(_))),
        "expected MergeInProgress, got {err:?}"
    );
    // The pre-existing merge state must be left exactly as found — this
    // check does not itself abort or otherwise touch it.
    assert!(manager.git().merge_in_progress());
}
