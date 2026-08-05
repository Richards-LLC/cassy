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

// --- cas-006c: classify_dirty_status (blocking vs warning vs CAS-excluded) --

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
        "husky artifact must not even warn — it's CAS's own droppings: {:?}",
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
        "untracked non-CAS paths must not block a merge/removal"
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

    // Real modified work, an unrelated untracked scratch file, AND the CAS
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
