use crate::worktree::git::*;
use tempfile::TempDir;

fn create_test_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().to_path_buf();

    // Initialize git repo
    Command::new("git")
        .args(["init"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    // Configure git user
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

    // Create initial commit
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
fn test_git_available() {
    // Git should be available in test environment
    assert!(GitOperations::is_git_available());
}

#[test]
fn test_detect_repo_root() {
    let (_temp, repo_path) = create_test_repo();

    let detected = GitOperations::detect_repo_root(&repo_path).unwrap();
    // Canonicalize both paths to handle macOS /var -> /private/var symlinks
    let detected_canon = detected.canonicalize().unwrap_or(detected);
    let repo_canon = repo_path.canonicalize().unwrap_or(repo_path);
    assert_eq!(detected_canon, repo_canon);
}

#[test]
fn test_current_branch() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path);

    let branch = git.current_branch().unwrap();
    // Default branch is usually "main" or "master"
    assert!(branch == "main" || branch == "master");
}

/// cas-9415: `git merge` commits to implicit HEAD. If another supervisor
/// parks the shared checkout on a foreign branch after the target was
/// resolved, the merge helper must refuse before changing any ref or file.
#[test]
fn merge_refuses_when_checkout_head_differs_from_resolved_target_cas_9415() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());
    let trunk = git.current_branch().unwrap();

    Command::new("git")
        .args(["branch", "epic/assembly"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["checkout", "-q", "-b", "factory/worker"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    std::fs::write(repo_path.join("worker.txt"), "worker change\n").unwrap();
    Command::new("git")
        .args(["add", "worker.txt"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "worker change"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["checkout", "-q", "-b", "scratch/foreign", &trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let epic_before = git.ref_sha("epic/assembly").unwrap();
    let scratch_before = git.ref_sha("scratch/foreign").unwrap();
    let error = git
        .merge_branch("epic/assembly", "factory/worker", true)
        .expect_err("a foreign checkout branch must abort the merge");

    match error {
        GitError::MergeTargetMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, "epic/assembly");
            assert_eq!(actual, "scratch/foreign");
        }
        other => panic!("expected target mismatch, got {other:?}"),
    }
    assert_eq!(git.current_branch().unwrap(), "scratch/foreign");
    assert_eq!(git.ref_sha("epic/assembly").unwrap(), epic_before);
    assert_eq!(git.ref_sha("scratch/foreign").unwrap(), scratch_before);
    assert!(!repo_path.join("worker.txt").exists());
    assert!(!git.merge_in_progress());
}

#[test]
fn test_detect_default_branch_ignores_current_feature_head() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());
    let trunk = git.current_branch().unwrap();

    Command::new("git")
        .args(["checkout", "-q", "-b", "feature/supervisor-head"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    assert_eq!(
        git.detect_default_branch(),
        trunk,
        "default branch detection must prefer the existing trunk ref over incidental supervisor HEAD"
    );
}

#[test]
fn test_create_and_remove_worktree() {
    let (temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path);

    let worktree_path = temp.path().join("feature-branch");

    // Create worktree
    git.create_worktree(&worktree_path, "feature-branch", None)
        .unwrap();
    assert!(worktree_path.exists());

    // List worktrees
    let worktrees = git.list_worktrees().unwrap();
    assert!(worktrees.len() >= 2); // Main + new worktree

    // Remove worktree
    git.remove_worktree(&worktree_path, false).unwrap();
    assert!(!worktree_path.exists());
}

#[test]
fn test_branch_exists() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path);

    let current = git.current_branch().unwrap();
    assert!(git.branch_exists(&current).unwrap());
    assert!(!git.branch_exists("nonexistent-branch").unwrap());
}

#[test]
fn test_get_context() {
    let (_temp, repo_path) = create_test_repo();

    let context = GitOperations::get_context(&repo_path).unwrap();
    assert!(context.branch.is_some());
    assert!(!context.is_worktree); // Main checkout is not a worktree
}

#[test]
fn test_init_submodules_no_submodules() {
    // Test that init_submodules succeeds when there are no submodules
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // Should succeed silently when no .gitmodules exists
    let result = git.init_submodules(&repo_path);
    assert!(result.is_ok());
}

#[test]
fn test_init_submodules_with_gitmodules() {
    // Test that init_submodules runs when .gitmodules exists
    let (temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // Create a .gitmodules file (simulating a repo with submodules)
    std::fs::write(
        repo_path.join(".gitmodules"),
        "[submodule \"vendor/test\"]\n\tpath = vendor/test\n\turl = https://example.com/test.git\n",
    )
    .unwrap();

    // Create a worktree
    let worktree_path = temp.path().join("test-worktree");
    git.create_worktree(&worktree_path, "test-branch", None)
        .unwrap();

    // The worktree should exist (submodule init may fail due to network,
    // but the worktree creation should still succeed)
    assert!(worktree_path.exists());
}

#[test]
fn test_worktree_with_submodule_init() {
    // Test that create_worktree calls init_submodules
    let (temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    let worktree_path = temp.path().join("sub-test");

    // Create worktree (should also init submodules if any)
    git.create_worktree(&worktree_path, "sub-test-branch", None)
        .unwrap();

    assert!(worktree_path.exists());

    // Verify we can manually call init_submodules again (idempotent)
    let result = git.init_submodules(&worktree_path);
    assert!(result.is_ok());
}

#[test]
fn test_get_submodule_paths() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // No .gitmodules - should return empty vec
    assert!(git.get_submodule_paths().unwrap().is_empty());

    // Create .gitmodules with submodule paths
    std::fs::write(
            repo_path.join(".gitmodules"),
            "[submodule \"vendor/ghostty\"]\n\tpath = vendor/ghostty\n\turl = https://example.com/ghostty.git\n\
             [submodule \"vendor/other\"]\n\tpath = vendor/other\n\turl = https://example.com/other.git\n",
        )
        .unwrap();

    let paths = git.get_submodule_paths().unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], std::path::PathBuf::from("vendor/ghostty"));
    assert_eq!(paths[1], std::path::PathBuf::from("vendor/other"));
}

#[test]
fn test_mark_config_skip_worktree() {
    let (temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // Create .claude directory with tracked files
    std::fs::create_dir_all(repo_path.join(".claude/rules")).unwrap();
    std::fs::write(repo_path.join(".claude/rules/test.md"), "test rule").unwrap();
    std::fs::write(repo_path.join("CLAUDE.md"), "# Claude").unwrap();

    Command::new("git")
        .args(["add", ".claude/", "CLAUDE.md"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Add config files"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    // Create a worktree
    let worktree_path = temp.path().join("test-skip-wt");
    git.create_worktree(&worktree_path, "test-skip-branch", None)
        .unwrap();

    // Mark skip-worktree
    git.mark_config_skip_worktree(&worktree_path).unwrap();

    // Modify a tracked file in the worktree
    std::fs::write(
        worktree_path.join(".claude/rules/test.md"),
        "modified rule content",
    )
    .unwrap();

    // The modification should NOT show up in git status
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&worktree_path)
        .output()
        .unwrap();

    let status = String::from_utf8_lossy(&output.stdout);
    assert!(
        !status.contains(".claude/rules/test.md"),
        "skip-worktree file should not appear in git status, got: {status}"
    );
}

#[test]
fn test_mark_config_skip_worktree_no_files() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // No .claude files tracked - should succeed silently
    let result = git.mark_config_skip_worktree(&repo_path);
    assert!(result.is_ok());
}

#[test]
fn test_fix_symlinked_submodules() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // Create .gitmodules
    std::fs::write(
        repo_path.join(".gitmodules"),
        "[submodule \"vendor/test\"]\n\tpath = vendor/test\n\turl = https://example.com/test.git\n",
    )
    .unwrap();

    // Create vendor directory
    std::fs::create_dir_all(repo_path.join("vendor")).unwrap();

    // Create a symlink for the submodule (simulating the legacy mitigation)
    let symlink_path = repo_path.join("vendor/test");
    let target = repo_path.join(".git"); // Just point to something that exists
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &symlink_path).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &symlink_path).unwrap();

    assert!(symlink_path.is_symlink());

    // Fix should remove the symlink
    git.fix_symlinked_submodules(&repo_path).unwrap();

    // Symlink should be gone (submodule init may or may not succeed, but symlink is removed)
    assert!(!symlink_path.is_symlink());
}

// --- cas-b082: resolve_fresh_base / fetch_branch / commits_behind ---------

/// Bare "origin" repo plus a local clone tracking it — a real git remote
/// setup (`create_test_repo` above is local-only). Returns
/// (tempdir, origin bare path, local clone path); both branches are named
/// "main" explicitly so tests don't depend on this system's
/// `init.defaultBranch`.
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
/// simulating upstream moving ahead while a different clone never fetches.
fn advance_origin_main(temp: &TempDir, origin_path: &Path, count: usize) {
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
fn test_resolve_fresh_base_no_remote_falls_back_to_local() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path);
    let trunk = git.detect_default_branch();

    let resolved = git.resolve_fresh_base(&trunk).unwrap();

    assert!(
        !resolved.used_remote,
        "local-only repo (no origin) must fall back to the local base"
    );
    assert_eq!(resolved.branch_ref, trunk);
    assert_eq!(resolved.behind_count, 0);
}

#[test]
fn test_resolve_fresh_base_up_to_date_remote() {
    let (_temp, _origin_path, local_path) = create_repo_with_origin();
    let git = GitOperations::new(local_path);

    let resolved = git.resolve_fresh_base("main").unwrap();

    assert!(resolved.used_remote);
    assert_eq!(resolved.branch_ref, "origin/main");
    assert_eq!(resolved.behind_count, 0);
    assert!(!resolved.sha.is_empty());
}

#[test]
fn test_resolve_fresh_base_reports_behind_count_and_uses_remote_tip() {
    let (temp, origin_path, local_path) = create_repo_with_origin();
    // origin/main gains 3 commits that local_path has never fetched — the
    // live BUG-epic-branch-stale-local-base-2026-07-08 scenario.
    advance_origin_main(&temp, &origin_path, 3);

    let git = GitOperations::new(local_path.clone());
    let stale_local_sha = git.ref_sha("main").unwrap();

    let resolved = git.resolve_fresh_base("main").unwrap();

    assert!(
        resolved.used_remote,
        "should resolve against the freshly fetched remote tracking branch"
    );
    assert_eq!(resolved.branch_ref, "origin/main");
    assert_eq!(
        resolved.behind_count, 3,
        "local base was exactly 3 commits behind origin/main"
    );
    assert_ne!(
        resolved.sha, stale_local_sha,
        "resolved sha must be the fetched remote tip, not the stale local head"
    );

    // Branching from the resolved ref must actually carry the 3 commits the
    // stale local `main` was missing — proves the fix, not just the report.
    Command::new("git")
        .args(["branch", "epic/test", &resolved.branch_ref])
        .current_dir(&local_path)
        .output()
        .unwrap();
    assert_eq!(git.ref_sha("epic/test").unwrap(), resolved.sha);
}

#[test]
fn test_commits_behind_counts_one_sided_divergence() {
    let (temp, origin_path, local_path) = create_repo_with_origin();
    advance_origin_main(&temp, &origin_path, 2);

    let git = GitOperations::new(local_path);
    git.fetch_branch("main").unwrap();

    assert_eq!(git.commits_behind("main", "origin/main").unwrap(), 2);
    // Local has nothing origin/main lacks — behind count the other way is 0.
    assert_eq!(git.commits_behind("origin/main", "main").unwrap(), 0);
}

// --- cas-0938: resolve_fresh_base must not silently drop local-ahead ------
// commits by unconditionally preferring origin/<base> whenever it exists.

#[test]
fn test_resolve_fresh_base_prefers_local_when_strictly_ahead_of_origin() {
    let (_temp, _origin_path, local_path) = create_repo_with_origin();

    // Add a local-only commit that is never pushed — origin/main stays at
    // the original tip while local main moves ahead.
    std::fs::write(local_path.join("unpushed.txt"), "local work").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "local-only commit"])
        .current_dir(&local_path)
        .output()
        .unwrap();

    let git = GitOperations::new(local_path.clone());
    let local_ahead_sha = git.ref_sha("main").unwrap();

    let resolved = git.resolve_fresh_base("main").unwrap();

    assert!(
        !resolved.used_remote,
        "local is strictly ahead of origin/main — origin is the stale ref here, \
         resolve_fresh_base must not take it and silently drop the local-only commit"
    );
    assert_eq!(resolved.branch_ref, "main");
    assert_eq!(resolved.sha, local_ahead_sha);
    assert_eq!(resolved.ahead_count, 1);
    assert_eq!(resolved.behind_count, 0);

    // Branching from the resolved ref must carry the local-only commit.
    Command::new("git")
        .args(["branch", "epic/test", &resolved.branch_ref])
        .current_dir(&local_path)
        .output()
        .unwrap();
    let tree = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "epic/test"])
        .current_dir(&local_path)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&tree.stdout).contains("unpushed.txt"),
        "epic branch must contain the local-only commit's file"
    );
}

#[test]
fn test_resolve_fresh_base_true_divergence_prefers_local_and_reports_both_counts() {
    let (temp, origin_path, local_path) = create_repo_with_origin();
    // origin/main gains 2 commits local never fetched...
    advance_origin_main(&temp, &origin_path, 2);
    // ...while local ALSO gains 1 commit of its own, never pushed. Local
    // has not fetched yet, so at resolution time this is genuine two-way
    // divergence once the fetch inside resolve_fresh_base runs.
    std::fs::write(local_path.join("local-only.txt"), "local work").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "local-only commit"])
        .current_dir(&local_path)
        .output()
        .unwrap();

    let git = GitOperations::new(local_path.clone());
    let local_sha = git.ref_sha("main").unwrap();

    let resolved = git.resolve_fresh_base("main").unwrap();

    assert!(
        !resolved.used_remote,
        "on true divergence the local ref must be preferred — never silently \
         drop the caller's own local-only commit by taking origin's tip"
    );
    assert_eq!(resolved.branch_ref, "main");
    assert_eq!(resolved.sha, local_sha);
    assert_eq!(resolved.ahead_count, 1, "local has exactly 1 commit origin lacks");
    assert_eq!(
        resolved.behind_count, 2,
        "origin has exactly 2 commits local lacks — still reported even though \
         local was preferred, so the caller can see what's missing"
    );
}

#[test]
fn test_resolve_fresh_base_no_divergence_still_prefers_remote_tip() {
    // Regression guard: the ahead-count fix must not disturb the original
    // cas-b082 behavior when local is ONLY behind (never ahead).
    let (temp, origin_path, local_path) = create_repo_with_origin();
    advance_origin_main(&temp, &origin_path, 1);

    let git = GitOperations::new(local_path);
    let resolved = git.resolve_fresh_base("main").unwrap();

    assert!(resolved.used_remote);
    assert_eq!(resolved.branch_ref, "origin/main");
    assert_eq!(resolved.ahead_count, 0);
    assert_eq!(resolved.behind_count, 1);
}

// --- cas-0938: fetch must be bounded, not block indefinitely on an -------
// unreachable remote. Tested via the generic process-bounding mechanism
// (not a real network hang, which would be slow/unreliable in CI) with a
// `sleep` child standing in for a hung `git fetch`.

#[test]
fn test_run_command_bounded_kills_hung_process_and_returns_promptly() {
    let mut cmd = Command::new("sleep");
    cmd.arg("5");

    let start = std::time::Instant::now();
    let result = GitOperations::run_command_bounded(cmd, std::time::Duration::from_millis(100));
    let elapsed = start.elapsed();

    assert!(
        result.is_err(),
        "a process that outlives the timeout must be reported as an error"
    );
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "must return promptly after the timeout fires, not wait out the full 5s hang; took {elapsed:?}"
    );
}

#[test]
fn test_run_command_bounded_returns_output_for_fast_process() {
    let mut cmd = Command::new("echo");
    cmd.arg("hello");

    let output =
        GitOperations::run_command_bounded(cmd, std::time::Duration::from_secs(5)).unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
}

#[test]
fn test_fetch_branch_bounded_times_out_fast_on_hung_remote() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    // Point origin at a non-routable, non-responding address so the fetch
    // hangs rather than fails fast — this is the scenario the timeout must
    // catch (a dead SSH host or a VPN that's down doesn't reject the
    // connection, it just never answers).
    Command::new("git")
        .args(["remote", "add", "origin", "git://10.255.255.1/nowhere.git"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let start = std::time::Instant::now();
    let result = git.fetch_branch_bounded("main", std::time::Duration::from_millis(200));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "an unreachable remote must not silently succeed");
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "fetch_branch must not block for git's full TCP connect/retry window; took {elapsed:?}"
    );
}

// --- cas-006c: classify_dirty_status (blocking vs warning vs Cassy-excluded) --

#[test]
fn test_classify_dirty_status_clean_repo_is_empty() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    let status = git.classify_dirty_status(&repo_path).unwrap();

    assert!(status.blocking.is_empty());
    assert!(status.warnings.is_empty());
    assert!(!status.is_blocked());
}

/// Commit a tracked `.husky/pre-commit` placeholder so `.husky/` itself is
/// already known to git — matching real repos where husky's tracked hook
/// scripts are committed. Only the `_` runner subdir is left untracked,
/// which is why git reports it individually (`?? .husky/_/`) instead of
/// collapsing the whole `.husky/` directory into one untracked entry.
fn commit_tracked_husky_dir(repo_path: &Path) {
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

#[test]
fn test_classify_dirty_status_husky_underscore_artifact_is_excluded_entirely() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());
    commit_tracked_husky_dir(&repo_path);

    // The exact false-positive from the bug report: an untracked `.husky/_/`
    // directory the worker startup hook creates itself.
    std::fs::create_dir_all(repo_path.join(".husky/_")).unwrap();
    std::fs::write(repo_path.join(".husky/_/husky.sh"), "# shim").unwrap();

    let status = git.classify_dirty_status(&repo_path).unwrap();

    assert!(
        status.blocking.is_empty(),
        "husky artifact must never block: {:?}",
        status.blocking
    );
    assert!(
        status.warnings.is_empty(),
        "husky artifact must not even warn — it's Cassy's own droppings: {:?}",
        status.warnings
    );
    assert!(!status.is_blocked());
}

#[test]
fn test_classify_dirty_status_modified_tracked_file_blocks_and_is_named() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    std::fs::write(repo_path.join("README.md"), "# Modified").unwrap();

    let status = git.classify_dirty_status(&repo_path).unwrap();

    assert!(status.is_blocked());
    assert_eq!(status.blocking.len(), 1);
    assert_eq!(status.blocking[0].path, "README.md");
    assert!(status.warnings.is_empty());
    assert!(status.describe_blocking().contains("README.md"));
}

#[test]
fn test_classify_dirty_status_untracked_non_cas_path_warns_not_blocks() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());

    std::fs::write(repo_path.join("scratch.txt"), "draft").unwrap();

    let status = git.classify_dirty_status(&repo_path).unwrap();

    assert!(
        !status.is_blocked(),
        "untracked non-Cassy paths must not block a merge/removal"
    );
    assert_eq!(status.warnings.len(), 1);
    assert_eq!(status.warnings[0].path, "scratch.txt");
    assert!(status.describe_warnings().contains("scratch.txt"));
}

#[test]
fn test_classify_dirty_status_mixed_only_tracked_change_blocks() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path.clone());
    commit_tracked_husky_dir(&repo_path);

    // Real modified work, an unrelated untracked scratch file, AND the Cassy
    // husky artifact all at once — only the modified file should block.
    std::fs::write(repo_path.join("README.md"), "# Modified").unwrap();
    std::fs::write(repo_path.join("scratch.txt"), "draft").unwrap();
    std::fs::create_dir_all(repo_path.join(".husky/_")).unwrap();
    std::fs::write(repo_path.join(".husky/_/husky.sh"), "# shim").unwrap();

    let status = git.classify_dirty_status(&repo_path).unwrap();

    assert!(status.is_blocked());
    assert_eq!(status.blocking.len(), 1);
    assert_eq!(status.blocking[0].path, "README.md");
    assert_eq!(status.warnings.len(), 1);
    assert_eq!(status.warnings[0].path, "scratch.txt");
}

// ---------------------------------------------------------------------------
// cas-a85e (GH #99): choosing the base for a NEW epic branch when the checkout
// is already sitting on a prior epic branch. Trunk stays the default anchor
// (cas-dc28); the previous-epic case must not be silent.
// ---------------------------------------------------------------------------

fn commit_on(repo: &std::path::Path, file: &str, contents: &str) {
    std::fs::write(repo.join(file), contents).unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", contents])
        .current_dir(repo)
        .output()
        .unwrap();
}

fn checkout_new(repo: &std::path::Path, branch: &str) {
    Command::new("git")
        .args(["checkout", "-q", "-b", branch])
        .current_dir(repo)
        .output()
        .unwrap();
}

fn trunk_of(repo: &std::path::Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn epic_base_prefers_the_active_epic_branch_when_head_is_ahead_of_trunk() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/first-cas-aaaa");
    commit_on(&repo_path, "one.txt", "epic work 1");
    commit_on(&repo_path, "two.txt", "epic work 2");

    let git = GitOperations::new(repo_path);
    let choice = git.resolve_epic_base(&trunk);

    assert!(choice.used_head, "a prior epic branch must win over trunk");
    assert_eq!(choice.base_ref, "epic/first-cas-aaaa");
    assert_eq!(choice.head_ahead, 2, "commit count must be exact");
    assert_eq!(choice.head_behind, 0);
    let notice = choice.notice.expect("the decision must be stated");
    assert!(
        notice.contains("epic/first-cas-aaaa") && notice.contains('2'),
        "notice must name the branch and the commit count: {notice}"
    );
}

#[test]
fn epic_base_keeps_trunk_and_warns_with_both_counts_when_epic_head_diverged() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/first-cas-aaaa");
    commit_on(&repo_path, "one.txt", "epic only");

    // Trunk moves on independently — now the two have genuinely diverged.
    Command::new("git")
        .args(["checkout", "-q", &trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    commit_on(&repo_path, "trunk.txt", "trunk only");
    Command::new("git")
        .args(["checkout", "-q", "epic/first-cas-aaaa"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let git = GitOperations::new(repo_path);
    let choice = git.resolve_epic_base(&trunk);

    assert!(
        !choice.used_head,
        "auto-stacking a diverged epic would silently drop the trunk-only commit"
    );
    assert_eq!(choice.base_ref, trunk);
    assert_eq!(choice.head_ahead, 1);
    assert_eq!(choice.head_behind, 1);
    let notice = choice.notice.expect("divergence must be surfaced");
    assert!(
        notice.contains("DIVERGED") && notice.contains("epic/first-cas-aaaa"),
        "diverged notice must be explicit: {notice}"
    );
}

#[test]
fn epic_base_stays_on_trunk_for_a_non_epic_head_but_still_reports_the_gap() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "feature/supervisor-head");
    commit_on(&repo_path, "feature.txt", "feature only");

    let git = GitOperations::new(repo_path);
    let choice = git.resolve_epic_base(&trunk);

    assert!(
        !choice.used_head,
        "an incidental feature HEAD must never seed an epic branch (cas-dc28)"
    );
    assert_eq!(choice.base_ref, trunk);
    assert_eq!(choice.head_ahead, 1);
    let notice = choice.notice.expect("a silent gap is the bug");
    assert!(
        notice.contains("feature/supervisor-head") && notice.contains("NOT included"),
        "note must name the excluded branch: {notice}"
    );
}

#[test]
fn epic_base_is_silent_when_head_is_trunk_or_not_ahead() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);

    let git = GitOperations::new(repo_path.clone());
    let on_trunk = git.resolve_epic_base(&trunk);
    assert_eq!(on_trunk.base_ref, trunk);
    assert!(on_trunk.notice.is_none(), "no divergence, no noise");
    assert_eq!(on_trunk.head_ahead, 0);

    // A branch that carries no commits of its own is equally uninteresting.
    checkout_new(&repo_path, "epic/empty-cas-bbbb");
    let no_commits = git.resolve_epic_base(&trunk);
    assert!(!no_commits.used_head);
    assert_eq!(no_commits.base_ref, trunk);
    assert!(no_commits.notice.is_none());
}

#[test]
fn epic_base_degrades_to_trunk_on_detached_head() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/first-cas-aaaa");
    commit_on(&repo_path, "one.txt", "epic only");
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();
    Command::new("git")
        .args(["checkout", "-q", "--detach", &sha])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let git = GitOperations::new(repo_path);
    let choice = git.resolve_epic_base(&trunk);

    assert!(!choice.used_head, "a detached HEAD names no epic to continue");
    assert_eq!(choice.base_ref, trunk);
    assert!(choice.notice.is_none());
}

#[test]
fn epic_base_states_the_merge_order_consequence_when_it_stacks() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/first-cas-aaaa");
    commit_on(&repo_path, "one.txt", "epic work");

    let git = GitOperations::new(repo_path);
    let notice = git
        .resolve_epic_base(&trunk)
        .notice
        .expect("stacking must be explained");

    assert!(
        notice.contains("CONTAINS") && notice.contains("epic/first-cas-aaaa"),
        "stacking carries the base epic's commits — the operator must be told: {notice}"
    );
}

#[test]
fn epic_base_degrades_to_trunk_when_git_cannot_answer() {
    // Not a git repository at all: every probe fails, and epic creation must
    // still get a usable base rather than an error.
    let temp = TempDir::new().unwrap();
    let git = GitOperations::new(temp.path().to_path_buf());

    let choice = git.resolve_epic_base("main");

    assert_eq!(choice.base_ref, "main");
    assert!(!choice.used_head);
    assert!(choice.notice.is_none());
    assert_eq!(choice.head_ahead, 0);
}

// ---------------------------------------------------------------------------
// cas-aae6 (GH #110): a stack deeper than one level. C on B on A used to
// present as "C is based on B" — the A → B → C landing order was invisible.
// ---------------------------------------------------------------------------

/// main → epic/a → epic/b → epic/c, each carrying one commit, none landed.
fn three_deep_stack() -> (TempDir, PathBuf, String) {
    let (temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/a");
    commit_on(&repo_path, "a.txt", "epic a work");
    checkout_new(&repo_path, "epic/b");
    commit_on(&repo_path, "b.txt", "epic b work");
    checkout_new(&repo_path, "epic/c");
    commit_on(&repo_path, "c.txt", "epic c work");
    (temp, repo_path, trunk)
}

#[test]
fn unlanded_epic_ancestry_reports_the_whole_chain_trunk_first() {
    let (_temp, repo_path, trunk) = three_deep_stack();
    let git = GitOperations::new(repo_path);

    assert_eq!(
        git.unlanded_epic_ancestry("epic/c", &trunk),
        vec!["epic/a".to_string(), "epic/b".to_string()],
        "the chain must be complete and ordered by the sequence they must land in"
    );
    assert_eq!(
        git.unlanded_epic_ancestry("epic/b", &trunk),
        vec!["epic/a".to_string()],
        "the middle of the stack sees only what is below it"
    );
    assert!(
        git.unlanded_epic_ancestry("epic/a", &trunk).is_empty(),
        "the bottom of the stack is stacked on nothing"
    );
}

#[test]
fn landed_epics_drop_out_of_the_chain() {
    let (_temp, repo_path, trunk) = three_deep_stack();
    // epic/a lands on trunk; it now constrains nothing.
    Command::new("git")
        .args(["checkout", "-q", &trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    Command::new("git")
        .args(["merge", "-q", "--no-ff", "epic/a", "-m", "land epic/a"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let git = GitOperations::new(repo_path);
    assert_eq!(
        git.unlanded_epic_ancestry("epic/c", &trunk),
        vec!["epic/b".to_string()],
        "a landed epic must not keep appearing as a blocker"
    );
}

/// cas-3afc (GH #299): staging promotions legitimately leave `main` behind.
/// Landing truth is relative to the epic's declared target, so an epic already
/// reachable from staging must not be shown as an unlanded stack dependency.
#[test]
fn epic_already_ancestor_of_declared_staging_target_is_not_unlanded_cas_3afc() {
    let (_temp, repo_path) = create_test_repo();
    let staging = "staging";
    Command::new("git")
        .args(["checkout", "-q", "-b", staging])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    commit_on(&repo_path, "promoted.txt", "promotion on staging");
    Command::new("git")
        .args(["branch", "epic/landed-on-staging"])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    commit_on(&repo_path, "more-staging.txt", "later staging work");

    let git = GitOperations::new(repo_path);
    assert!(
        git.unlanded_epic_ancestry(staging, staging).is_empty(),
        "an epic ancestor of its declared target must not appear stacked/unlanded"
    );
}

#[test]
fn sibling_epics_are_not_part_of_the_chain() {
    let (_temp, repo_path, trunk) = three_deep_stack();
    // An unrelated epic off trunk: unlanded, but not contained in epic/c.
    Command::new("git")
        .args(["checkout", "-q", &trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    checkout_new(&repo_path, "epic/unrelated");
    commit_on(&repo_path, "u.txt", "unrelated epic work");

    let git = GitOperations::new(repo_path);
    let chain = git.unlanded_epic_ancestry("epic/c", &trunk);
    assert_eq!(chain, vec!["epic/a".to_string(), "epic/b".to_string()]);
    assert!(
        !chain.contains(&"epic/unrelated".to_string()),
        "only branches actually contained in the base constrain it: {chain:?}"
    );
}

#[test]
fn epic_base_notice_names_the_full_stack_not_just_the_parent() {
    let (_temp, repo_path, trunk) = three_deep_stack();
    // Sitting on epic/c, create a fourth epic.
    let git = GitOperations::new(repo_path);
    let choice = git.resolve_epic_base(&trunk);

    assert!(choice.used_head);
    assert_eq!(choice.base_ref, "epic/c");
    assert_eq!(
        choice.stacked_on,
        vec!["epic/a".to_string(), "epic/b".to_string()],
        "the choice must carry the whole chain, not one level"
    );
    let notice = choice.notice.expect("stacking must be explained");
    assert!(
        notice.contains("STACK DEPTH 3"),
        "depth must be stated so a deep stack is obvious at a glance: {notice}"
    );
    assert!(
        notice.contains("'epic/a' → 'epic/b'")
            && notice.contains("'epic/a' → 'epic/b' → 'epic/c'"),
        "the notice must name the ancestry AND the landing order: {notice}"
    );
    assert!(
        notice.contains("already contains")
            && notice.contains("no separate merge of each is required"),
        "containment is the true claim — the branches ride along whether or not \
         anyone plans for it, and no per-branch merge is implied: {notice}"
    );
}

#[test]
fn single_level_stack_does_not_claim_a_chain() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/only");
    commit_on(&repo_path, "o.txt", "epic work");

    let git = GitOperations::new(repo_path);
    let choice = git.resolve_epic_base(&trunk);

    assert!(choice.used_head);
    assert!(
        choice.stacked_on.is_empty(),
        "one level is not a stack: {:?}",
        choice.stacked_on
    );
    let notice = choice.notice.expect("notice");
    assert!(
        !notice.contains("STACK DEPTH"),
        "no chain language when there is no chain: {notice}"
    );
}

#[test]
fn ancestry_is_empty_when_git_cannot_answer() {
    let temp = TempDir::new().unwrap();
    let git = GitOperations::new(temp.path().to_path_buf());
    assert!(
        git.unlanded_epic_ancestry("epic/c", "main").is_empty(),
        "an advisory display must never fail epic creation"
    );
}

/// An epic branch can MERGE two independent unlanded epics rather than stack on
/// one. Both are then ancestors while neither contains the other, so the report
/// must not claim a containment order between them — only that this epic
/// contains them all.
#[test]
fn independent_merged_epics_are_both_reported_without_claiming_a_chain_between_them() {
    let (_temp, repo_path) = create_test_repo();
    let trunk = trunk_of(&repo_path);
    checkout_new(&repo_path, "epic/x");
    commit_on(&repo_path, "x.txt", "epic x work");
    Command::new("git")
        .args(["checkout", "-q", &trunk])
        .current_dir(&repo_path)
        .output()
        .unwrap();
    checkout_new(&repo_path, "epic/y");
    commit_on(&repo_path, "y.txt", "epic y work");
    checkout_new(&repo_path, "epic/z");
    Command::new("git")
        .args(["merge", "-q", "--no-ff", "epic/x", "-m", "z merges x"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let git = GitOperations::new(repo_path);
    let chain = git.unlanded_epic_ancestry("epic/z", &trunk);
    assert!(
        chain.contains(&"epic/x".to_string()) && chain.contains(&"epic/y".to_string()),
        "both unlanded epics ride along and must be named: {chain:?}"
    );

    // Neither contains the other, so wording implying a required sequence
    // between them would be false. Assert on what the notice ACTUALLY says —
    // an earlier version of this test asserted a phrase that appears nowhere in
    // the source, so it passed no matter how misleading the text became.
    let choice = git.resolve_epic_base(&trunk);
    let notice = choice
        .notice
        .as_deref()
        .expect("stacking on epic/z must be explained");
    assert!(
        notice.contains("Landing this epic lands all of them together")
            && notice.contains("no separate merge of each is required"),
        "the claim must be containment, not a mandated sequence: {notice}"
    );
    assert!(
        !notice.contains("merge order is"),
        "'merge order' reads as an instruction to merge each in turn, which is \
         false for independent siblings: {notice}"
    );
}

/// A rename that left the old branch name behind is one rung, not two.
#[test]
fn duplicate_branch_names_for_one_commit_do_not_inflate_the_stack() {
    let (_temp, repo_path, trunk) = three_deep_stack();
    Command::new("git")
        .args(["branch", "epic/a-old-name", "epic/a"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    let git = GitOperations::new(repo_path);
    let chain = git.unlanded_epic_ancestry("epic/c", &trunk);

    assert_eq!(
        chain.len(),
        2,
        "two names for the same commit must count once: {chain:?}"
    );
    assert!(
        chain.contains(&"epic/b".to_string()),
        "the genuine second rung must survive dedupe: {chain:?}"
    );
}

// ---------------------------------------------------------------------------
// cas-5ee0 (GH #137): worktree_merge receipt tells the truth about push state.
//
// A merge that moves only the LOCAL target ref is invisible to every other
// checkout and to the task-close merge-state guard, which measures ancestry
// against BOTH `<parent>` and `origin/<parent>`. Before this, worktree_merge
// returned a bare "Merged ..." success with no push-state information at all,
// so the merge receipt and the close guard could disagree about which ref was
// authoritative — a guaranteed close-rejection loop that only a manual
// `git push origin <target>` could break.
// ---------------------------------------------------------------------------

/// Commit a file on the currently checked-out branch of `repo`.
fn commit_file(repo: &Path, name: &str, body: &str) {
    std::fs::write(repo.join(name), body).unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", name])
        .current_dir(repo)
        .output()
        .unwrap();
}

/// Reproduce the exact defect shape: a factory branch merged into LOCAL `main`
/// while `origin/main` stays at the pre-merge tip.
///
/// Returns (temp, origin bare path, local clone path, merge commit sha).
fn merged_locally_with_origin_behind() -> (TempDir, PathBuf, PathBuf, String) {
    let (temp, origin_path, local_path) = create_repo_with_origin();
    let pre_merge_origin_tip = GitOperations::new(local_path.clone())
        .resolve_commit("origin/main")
        .unwrap();

    Command::new("git")
        .args(["checkout", "-q", "-b", "factory/kind-newt-49"])
        .current_dir(&local_path)
        .output()
        .unwrap();
    commit_file(&local_path, "worker.txt", "worker work\n");

    Command::new("git")
        .args(["checkout", "-q", "main"])
        .current_dir(&local_path)
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "merge",
            "--no-ff",
            "-q",
            "-m",
            "merge worker",
            "factory/kind-newt-49",
        ])
        .current_dir(&local_path)
        .output()
        .unwrap();

    let git = GitOperations::new(local_path.clone());
    let merge_sha = git.resolve_commit("main").unwrap();
    assert_ne!(
        merge_sha, pre_merge_origin_tip,
        "fixture must actually advance local main"
    );
    assert_eq!(
        git.resolve_commit("origin/main").as_deref(),
        Some(pre_merge_origin_tip.as_str()),
        "fixture must leave origin/main at the PRE-merge tip — that is the defect"
    );

    (temp, origin_path, local_path, merge_sha)
}

#[test]
fn publish_branch_to_origin_reports_no_remote_for_local_only_repo() {
    let (_temp, repo_path) = create_test_repo();
    let git = GitOperations::new(repo_path);
    let trunk = git.detect_default_branch();

    // No `origin` at all: nothing downstream can consult a remote ref either,
    // so this must NOT read as "you forgot to push".
    assert_eq!(
        git.publish_branch_to_origin(&trunk),
        TargetPushOutcome::NoRemote
    );
    assert!(git.publish_branch_to_origin(&trunk).is_published());
}

#[test]
fn publish_branch_to_origin_never_creates_an_absent_remote_branch() {
    let (_temp, origin_path, local_path) = create_repo_with_origin();
    let git = GitOperations::new(local_path.clone());

    Command::new("git")
        .args(["checkout", "-q", "-b", "epic/local-only"])
        .current_dir(&local_path)
        .output()
        .unwrap();
    commit_file(&local_path, "epic.txt", "epic work\n");

    assert_eq!(
        git.publish_branch_to_origin("epic/local-only"),
        TargetPushOutcome::RemoteBranchAbsent,
        "a branch that was never published must not be created by a merge side effect"
    );

    // Prove the remote really is untouched.
    let refs = Command::new("git")
        .args([
            "--git-dir",
            origin_path.to_str().unwrap(),
            "branch",
            "--list",
        ])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&refs.stdout).contains("epic/local-only"),
        "origin must not have gained the branch"
    );
}

#[test]
fn publish_branch_to_origin_is_already_current_when_nothing_to_push() {
    let (_temp, _origin_path, local_path) = create_repo_with_origin();
    let git = GitOperations::new(local_path);

    let sha = git.resolve_commit("main").unwrap();
    assert_eq!(
        git.publish_branch_to_origin("main"),
        TargetPushOutcome::AlreadyCurrent { sha }
    );
}

/// AC1 + AC2: the merged-locally-but-origin-behind case is repaired by the
/// merge itself, so the receipt's target ref and the close guard's
/// `origin/<parent>` view agree without a manual push step.
#[test]
fn publish_branch_to_origin_pushes_a_locally_merged_target() {
    let (_temp, _origin_path, local_path, merge_sha) = merged_locally_with_origin_behind();
    let git = GitOperations::new(local_path);

    assert_eq!(
        git.publish_branch_to_origin("main"),
        TargetPushOutcome::Pushed {
            sha: merge_sha.clone()
        }
    );
    assert_eq!(
        git.resolve_commit("origin/main").as_deref(),
        Some(merge_sha.as_str()),
        "origin/main must now carry the merge"
    );
}

/// AC3: the close guard's own measurement — which is what rejected the worker
/// with "N commit(s) from this task not on main" — flips from stranded to
/// merged once the target is published, and nothing else changes.
#[test]
fn close_guard_origin_view_only_sees_the_merge_after_the_target_is_published() {
    use crate::mcp::tools::core::task::lifecycle::close_ops::count_unmerged_against_targets;

    let (_temp, _origin_path, local_path, merge_sha) = merged_locally_with_origin_behind();
    let git = GitOperations::new(local_path.clone());

    // A checkout that only ever sees origin (the shape of any other worker's
    // repo, and of the close guard after it fetches) still calls this work
    // stranded while the merge is local-only.
    let origin_only = Command::new("git")
        .args([
            "rev-list",
            "--count",
            "factory/kind-newt-49",
            "--not",
            "origin/main",
        ])
        .current_dir(&local_path)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&origin_only.stdout).trim(),
        "1",
        "pre-push, origin/main must not contain the worker commit"
    );

    assert!(matches!(
        git.publish_branch_to_origin("main"),
        TargetPushOutcome::Pushed { .. }
    ));

    // Post-push both refs agree, so the guard proceeds.
    assert_eq!(
        count_unmerged_against_targets(&local_path, "factory/kind-newt-49", "main"),
        Some(0),
        "after publishing, the merge-state guard must see zero stranded commits"
    );
    assert_eq!(
        git.resolve_commit("origin/main").as_deref(),
        Some(merge_sha.as_str())
    );
}

/// A diverged remote must be reported, never force-pushed over.
#[test]
fn publish_branch_to_origin_reports_not_pushed_on_diverged_remote() {
    let (temp, origin_path, local_path, merge_sha) = merged_locally_with_origin_behind();
    advance_origin_main(&temp, &origin_path, 1);

    let git = GitOperations::new(local_path.clone());
    let origin_tip_before = {
        // Read the remote's real tip, not this clone's stale tracking ref.
        let out = Command::new("git")
            .args([
                "--git-dir",
                origin_path.to_str().unwrap(),
                "rev-parse",
                "main",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let outcome = git.publish_branch_to_origin("main");
    match &outcome {
        TargetPushOutcome::NotPushed { sha, reason, .. } => {
            assert_eq!(sha.as_deref(), Some(merge_sha.as_str()));
            assert!(!reason.is_empty(), "the failure must carry a reason");
        }
        other => panic!("diverged remote must report NotPushed, got {other:?}"),
    }
    assert!(!outcome.is_published());

    let out = Command::new("git")
        .args([
            "--git-dir",
            origin_path.to_str().unwrap(),
            "rev-parse",
            "main",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        origin_tip_before,
        "a non-fast-forward must never be clobbered"
    );
}

#[test]
fn publish_branch_to_origin_bounds_its_wall_clock() {
    let (_temp, _origin_path, local_path, _sha) = merged_locally_with_origin_behind();
    let git = GitOperations::new(local_path);

    // A zero timeout kills the push before it can finish; the point is that a
    // hung remote degrades to a loud NotPushed instead of blocking the MCP
    // handler forever.
    let outcome = git.publish_branch_to_origin_bounded("main", std::time::Duration::from_millis(0));
    assert!(
        matches!(outcome, TargetPushOutcome::NotPushed { .. }),
        "a timed-out push must report NotPushed, got {outcome:?}"
    );
}
