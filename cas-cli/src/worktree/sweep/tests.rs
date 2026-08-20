//! Integration tests for the sweep module.
//!
//! Each test builds a temp git repo with `.cas/worktrees/<name>` children
//! that are real `git worktree add` products (or plain dirs for negative
//! cases), then exercises `sweep_one_repo` and asserts on the Disposition.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::worktree::sweep::{Disposition, SweepOptions, sweep_one_repo};

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", dir.display()))
}

fn git_ok(dir: &Path, args: &[&str]) {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn bootstrap_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "main"]);
    git_ok(&repo, &["config", "user.email", "t@t.com"]);
    git_ok(&repo, &["config", "user.name", "Test"]);
    git_ok(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    git_ok(&repo, &["add", "."]);
    git_ok(&repo, &["commit", "-q", "-m", "initial"]);
    repo
}

/// Create a factory-style worktree under `<repo>/.cas/worktrees/<name>` on a
/// new branch forked from main. Returns its path.
fn add_worktree(repo: &Path, name: &str) -> PathBuf {
    let path = repo.join(".cas").join("worktrees").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    git_ok(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            &format!("factory/{name}"),
            path.to_str().unwrap(),
            "main",
        ],
    );
    // Make sure we have an identity inside the worktree for test commits.
    git_ok(&path, &["config", "user.email", "t@t.com"]);
    git_ok(&path, &["config", "user.name", "Test"]);
    git_ok(&path, &["config", "commit.gpgsign", "false"]);
    path
}

#[test]
fn sweep_clean_merged_worktree_removes_it() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let wt = add_worktree(&repo, "worker1");
    // No commits on the worktree branch — it's trivially merged with main.

    let report = sweep_one_repo(&repo, SweepOptions::default());

    assert_eq!(report.worktrees.len(), 1);
    assert!(matches!(
        report.worktrees[0].disposition,
        Disposition::Removed
    ));
    assert!(!wt.exists(), "worktree dir must be gone");
    assert!(report.prune_ran, "prune must run when worktrees existed");
}

/// GH #529: pnpm's package entries are symlinks. If the primary checkout has
/// been repointed into an otherwise clean worker's virtual store, the sweep
/// must preserve the worktree rather than creating a quiet module-resolution
/// failure after successful-looking tests.
#[cfg(unix)]
#[test]
fn sweep_refuses_worktree_with_primary_node_modules_link_into_it() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    git_ok(&repo, &["add", ".gitignore"]);
    git_ok(&repo, &["commit", "-q", "-m", "ignore dependency trees"]);
    std::fs::write(repo.join("package.json"), "{}").unwrap();
    let wt = add_worktree(&repo, "pnpm-store");
    let package = wt.join("node_modules/.pnpm/pkg@1/node_modules/pkg");
    fs::create_dir_all(&package).unwrap();
    let link = repo.join("node_modules/pkg");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&package, &link).unwrap();

    let report = sweep_one_repo(&repo, SweepOptions::default());
    match &report.worktrees[0].disposition {
        Disposition::SkippedExternalSymlinks { links } => {
            assert_eq!(links.len(), 1, "links: {links:?}");
            assert_eq!(links[0].link, link);
        }
        other => panic!("expected inbound-link refusal, got {other:?}"),
    }
    assert!(wt.exists(), "worktree must survive the refusal");
    assert!(link.exists(), "primary checkout link must remain live");
}

#[test]
fn sweep_dirty_worktree_skips_without_flag() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let wt = add_worktree(&repo, "worker2");
    fs::write(wt.join("dirty.txt"), "wip\n").unwrap();

    let report = sweep_one_repo(&repo, SweepOptions::default());

    assert_eq!(report.worktrees.len(), 1);
    assert!(
        matches!(
            report.worktrees[0].disposition,
            Disposition::SkippedDirty { modified_files: 1 }
        ),
        "got {:?}",
        report.worktrees[0].disposition,
    );
    assert!(wt.exists(), "dirty worktree must NOT be removed");
}

#[test]
fn sweep_dirty_worktree_salvages_and_removes_with_flag() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let wt = add_worktree(&repo, "worker3");
    fs::write(wt.join("dirty.txt"), "wip\n").unwrap();

    let opts = SweepOptions {
        dry_run: false,
        salvage_dirty: true,
    };
    let report = sweep_one_repo(&repo, opts);

    assert_eq!(report.worktrees.len(), 1);
    match &report.worktrees[0].disposition {
        Disposition::SalvagedAndRemoved { patch_path } => {
            assert!(patch_path.exists(), "salvage patch must be on disk");
            assert!(patch_path.starts_with(repo.join(".cas/salvage")));
        }
        other => panic!("expected SalvagedAndRemoved, got {other:?}"),
    }
    assert!(!wt.exists(), "worktree dir must be removed");
}

#[test]
fn sweep_unmerged_worktree_is_skipped() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let wt = add_worktree(&repo, "worker4");
    // Commit something on the worktree branch so it has unmerged commits vs main.
    fs::write(wt.join("feat.txt"), "feature\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "feature commit"]);

    let report = sweep_one_repo(&repo, SweepOptions::default());
    match &report.worktrees[0].disposition {
        Disposition::SkippedUnmerged { unmerged_commits } => {
            assert_eq!(*unmerged_commits, 1);
        }
        other => panic!("expected SkippedUnmerged, got {other:?}"),
    }
    assert!(wt.exists(), "unmerged worktree MUST NOT be removed");
}

#[test]
fn sweep_dirty_and_unmerged_is_skipped_even_with_salvage_flag() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let wt = add_worktree(&repo, "worker5");
    // Commit + dirty working tree.
    fs::write(wt.join("feat.txt"), "feature\n").unwrap();
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "feature commit"]);
    fs::write(wt.join("dirty.txt"), "extra wip\n").unwrap();

    let opts = SweepOptions {
        dry_run: false,
        salvage_dirty: true, // must not override unmerged guard
    };
    let report = sweep_one_repo(&repo, opts);
    match &report.worktrees[0].disposition {
        Disposition::SkippedDirtyUnmerged {
            modified_files,
            unmerged_commits,
        } => {
            assert_eq!(*modified_files, 1);
            assert_eq!(*unmerged_commits, 1);
        }
        other => panic!("expected SkippedDirtyUnmerged, got {other:?}"),
    }
    assert!(wt.exists(), "unmerged+dirty must not be removed");
}

#[test]
fn dry_run_makes_no_filesystem_changes() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let clean = add_worktree(&repo, "clean");
    let dirty = add_worktree(&repo, "dirty");
    fs::write(dirty.join("x.txt"), "wip\n").unwrap();

    let opts = SweepOptions {
        dry_run: true,
        salvage_dirty: true,
    };
    let report = sweep_one_repo(&repo, opts);

    assert!(clean.exists(), "dry run must not remove clean worktree");
    assert!(dirty.exists(), "dry run must not remove dirty worktree");
    assert!(
        !repo.join(".cas/salvage").exists()
            || repo
                .join(".cas/salvage")
                .read_dir()
                .unwrap()
                .next()
                .is_none(),
        "dry run must not write a salvage patch"
    );
    assert!(!report.prune_ran, "dry run must not invoke git prune");
    let kinds: Vec<_> = report
        .worktrees
        .iter()
        .map(|w| std::mem::discriminant(&w.disposition))
        .collect();
    assert_eq!(kinds.len(), 2);
}

#[test]
fn repo_with_no_worktrees_dir_returns_empty_report() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    // Deliberately no .cas/worktrees/.
    let report = sweep_one_repo(&repo, SweepOptions::default());
    assert!(report.worktrees.is_empty());
    assert!(report.repo_error.is_none());
    assert!(!report.prune_ran, "no worktrees → no prune");
}

#[test]
fn sweep_multiple_worktrees_per_repo() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    let _a = add_worktree(&repo, "a");
    let b = add_worktree(&repo, "b");
    let _c = add_worktree(&repo, "c");
    fs::write(b.join("wip.txt"), "wip\n").unwrap(); // dirty

    let report = sweep_one_repo(&repo, SweepOptions::default());
    assert_eq!(report.worktrees.len(), 3);
    assert_eq!(report.removed_count(), 2, "a + c removed");
    assert_eq!(report.skipped_count(), 1, "b skipped dirty");
    assert!(report.prune_ran);
}

#[test]
fn unknown_parent_branch_produces_error_not_silent_delete() {
    // Build a repo where `main`, `master`, `develop`, `trunk` all do NOT exist.
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "custom-default"]);
    git_ok(&repo, &["config", "user.email", "t@t.com"]);
    git_ok(&repo, &["config", "user.name", "Test"]);
    git_ok(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    git_ok(&repo, &["add", "."]);
    git_ok(&repo, &["commit", "-q", "-m", "initial"]);

    // Add a worktree on a branch forked from custom-default with a unique commit.
    let wt = repo.join(".cas/worktrees/worker");
    fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git_ok(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "factory/worker",
            wt.to_str().unwrap(),
            "custom-default",
        ],
    );
    fs::write(wt.join("feature.txt"), "committed work\n").unwrap();
    git_ok(&wt, &["config", "user.email", "t@t.com"]);
    git_ok(&wt, &["config", "user.name", "Test"]);
    git_ok(&wt, &["config", "commit.gpgsign", "false"]);
    git_ok(&wt, &["add", "."]);
    git_ok(&wt, &["commit", "-q", "-m", "unpushed work"]);

    // Without a resolvable parent branch, sweep MUST refuse to classify
    // and MUST NOT remove the worktree (committed work would be lost).
    let report = sweep_one_repo(&repo, SweepOptions::default());
    assert_eq!(report.worktrees.len(), 1);
    match &report.worktrees[0].disposition {
        Disposition::Error { reason } => {
            assert!(
                reason.contains("parent branch"),
                "expected parent-branch error, got {reason}"
            );
        }
        other => panic!(
            "expected Error(parent branch), got {other:?} — worktree with committed work was classified as something else"
        ),
    }
    assert!(
        wt.exists(),
        "worktree with committed work must NOT be removed"
    );
}

#[test]
fn symlink_worktree_path_is_refused_not_followed() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    // A side directory outside .cas/worktrees/ that stands in for a live
    // worker's checkout. A symlink inside .cas/worktrees/ will point at it.
    let external = temp.path().join("live-worker-dir");
    fs::create_dir_all(external.join(".git")).unwrap();
    fs::write(external.join("precious.txt"), "must not be touched\n").unwrap();

    let wt_root = repo.join(".cas/worktrees");
    fs::create_dir_all(&wt_root).unwrap();
    let link = wt_root.join("sneaky");
    symlink(&external, &link).unwrap();

    let report = sweep_one_repo(&repo, SweepOptions::default());
    let sneaky_rec = report.worktrees.iter().find(|r| r.worktree_path == link);
    if let Some(rec) = sneaky_rec {
        // If the sweep classified at all, it must be Error("symlink").
        match &rec.disposition {
            Disposition::Error { reason } => assert!(
                reason.contains("symlink"),
                "expected symlink-refuse reason, got {reason}"
            ),
            other => panic!("expected Error(symlink), got {other:?}"),
        }
    }
    // Regardless of classification outcome, the external "live worker"
    // directory and its files must be untouched.
    assert!(
        external.join(".git").exists() && external.join("precious.txt").exists(),
        "symlink target must not be touched by sweep"
    );
    let contents = fs::read_to_string(external.join("precious.txt")).unwrap();
    assert_eq!(contents, "must not be touched\n");
}

#[test]
fn sweep_all_known_iterates_registry_and_flags_unhealthy() {
    crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
        crate::store::known_repos::ensure_host_schema().unwrap();
        let a = bootstrap_repo_in(home, "a");
        add_worktree(&a, "w1");

        let b = home.join("b-moved");
        // b is registered but has no .cas/ → unhealthy
        use crate::store::KnownRepoStore;
        let store = crate::store::known_repos::open_host_known_repo_store().unwrap();
        store.upsert(&a).unwrap();
        store.upsert(&b).unwrap();

        let report = crate::worktree::sweep::sweep_all_known(SweepOptions::default()).unwrap();
        assert_eq!(report.repos.len(), 2);
        let unhealthy = report
            .repos
            .iter()
            .find(|r| r.repo_root.ends_with("b-moved"))
            .unwrap();
        assert!(unhealthy.repo_error.is_some());
        let healthy = report
            .repos
            .iter()
            .find(|r| r.repo_root == a.canonicalize().unwrap() || r.repo_root == a)
            .unwrap();
        assert_eq!(healthy.removed_count(), 1);
    });
}

fn bootstrap_repo_in(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "main"]);
    git_ok(&repo, &["config", "user.email", "t@t.com"]);
    git_ok(&repo, &["config", "user.name", "Test"]);
    git_ok(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    git_ok(&repo, &["add", "."]);
    git_ok(&repo, &["commit", "-q", "-m", "initial"]);
    // Mark .cas/ so list_tracked_repos reports it as healthy.
    fs::create_dir_all(repo.join(".cas")).unwrap();
    repo
}

#[test]
fn non_worktree_subdir_is_ignored() {
    let temp = TempDir::new().unwrap();
    let repo = bootstrap_repo(&temp);
    // Plain dir without a .git marker — must be skipped (not counted, not an error).
    let plain = repo.join(".cas/worktrees/just-a-dir");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("README"), "hi").unwrap();

    let report = sweep_one_repo(&repo, SweepOptions::default());
    assert_eq!(report.worktrees.len(), 0);
    assert!(plain.exists());
}
