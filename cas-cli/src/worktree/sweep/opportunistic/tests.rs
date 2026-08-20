//! Integration tests for the opportunistic cross-repo sweep.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::config::WorktreesConfig;
use crate::store::KnownRepoStore;
use crate::store::known_repos::{ensure_host_schema, open_host_known_repo_store};
use crate::test_support::TestEnvGuard;
use crate::worktree::sweep::opportunistic::{
    OpportunisticOutcome, debounce_file, is_due, log_file, run_forced, run_if_due,
};

fn git_ok(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn bootstrap_repo(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    fs::create_dir_all(repo.join(".cas")).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "main"]);
    git_ok(&repo, &["config", "user.email", "t@t.com"]);
    git_ok(&repo, &["config", "user.name", "Test"]);
    git_ok(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    git_ok(&repo, &["add", "."]);
    git_ok(&repo, &["commit", "-q", "-m", "initial"]);
    repo
}

fn add_worktree_aged(repo: &Path, name: &str, age: Duration) -> PathBuf {
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
    git_ok(&path, &["config", "user.email", "t@t.com"]);
    git_ok(&path, &["config", "user.name", "Test"]);
    git_ok(&path, &["config", "commit.gpgsign", "false"]);
    set_mtime_back(&path, age);
    path
}

fn set_mtime_back(path: &Path, age: Duration) {
    let target = std::time::SystemTime::now() - age;
    let ft = filetime::FileTime::from_system_time(target);
    filetime::set_file_mtime(path, ft).unwrap();
}

fn register(repo: &Path) {
    let store = open_host_known_repo_store().unwrap();
    store.upsert(repo).unwrap();
}

fn ttl_zero() -> WorktreesConfig {
    WorktreesConfig {
        abandon_ttl_hours: 0,
        global_sweep_debounce_secs: 0,
        sweep_claude_agent_dirs: true,
        ..Default::default()
    }
}

#[test]
fn reclaims_old_clean_worktree() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        let wt = add_worktree_aged(&repo, "old-clean", Duration::from_secs(48 * 3600));
        register(&repo);

        let s = run_forced(&ttl_zero()).unwrap();
        assert_eq!(s.reclaimed, 1);
        assert_eq!(s.salvaged, 0);
        assert_eq!(s.young_preserved, 0);
        assert!(!wt.exists(), "clean old worktree must be removed");
    });
}

/// GH #529: the TTL-driven GC has a separate deletion path from the manual
/// sweep. A live pnpm-style primary-checkout link into the aged worktree must
/// block this path too, otherwise GC creates broken imports long after the
/// linking mistake.
#[cfg(unix)]
#[test]
fn preserves_old_worktree_linked_from_primary_node_modules() {
    use std::os::unix::fs::symlink;

    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        fs::write(repo.join("package.json"), "{}").unwrap();
        let wt = add_worktree_aged(&repo, "pnpm-link", Duration::from_secs(48 * 3600));
        let package = wt.join("node_modules/.pnpm/pkg@1/node_modules/pkg");
        fs::create_dir_all(&package).unwrap();
        let link = repo.join("node_modules/pkg");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&package, &link).unwrap();
        register(&repo);

        let summary = run_forced(&ttl_zero()).unwrap();
        let outcome = &summary.per_repo[0].entries[0].1;
        match outcome {
            OpportunisticOutcome::InboundSymlinksBlocked { links } => {
                assert_eq!(links.len(), 1, "links: {links:?}");
                assert_eq!(links[0].link, link);
            }
            other => panic!("expected inbound-link refusal, got {other:?}"),
        }
        assert!(wt.exists(), "worktree must survive the refusal");
        assert!(link.exists(), "dependency link must remain live");
    });
}

#[test]
fn salvages_old_dirty_worktree() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        let wt = add_worktree_aged(&repo, "old-dirty", Duration::from_secs(48 * 3600));
        fs::write(wt.join("wip.txt"), "uncommitted work\n").unwrap();
        register(&repo);

        let s = run_forced(&ttl_zero()).unwrap();
        assert_eq!(s.salvaged, 1);
        assert_eq!(s.reclaimed, 0);
        assert!(
            !wt.exists(),
            "dirty old worktree must be removed after salvage"
        );
        let salvage_dir = repo.join(".cas/salvage");
        let patches: Vec<_> = fs::read_dir(&salvage_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(patches.len(), 1, "salvage patch must be on disk");
        assert!(patches[0].extension().and_then(|s| s.to_str()) == Some("patch"));
    });
}

#[test]
fn preserves_young_worktree() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        let wt = add_worktree_aged(&repo, "fresh", Duration::from_secs(10));
        register(&repo);

        let config = WorktreesConfig {
            abandon_ttl_hours: 24,
            ..ttl_zero()
        };
        let s = run_forced(&config).unwrap();
        assert_eq!(s.young_preserved, 1);
        assert_eq!(s.reclaimed + s.salvaged, 0);
        assert!(wt.exists());
    });
}

#[test]
fn claude_agent_dirs_feature_flag_respected() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        // Simulated .claude worktree: a real git worktree with agent- prefix.
        let agent_dir = repo.join(".claude/worktrees/agent-foo");
        fs::create_dir_all(agent_dir.parent().unwrap()).unwrap();
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "agent/foo",
                agent_dir.to_str().unwrap(),
                "main",
            ],
        );
        set_mtime_back(&agent_dir, Duration::from_secs(48 * 3600));
        register(&repo);

        // Flag off → agent dir must be preserved.
        let off = WorktreesConfig {
            sweep_claude_agent_dirs: false,
            ..ttl_zero()
        };
        let s = run_forced(&off).unwrap();
        assert_eq!(s.reclaimed + s.salvaged, 0);
        assert!(agent_dir.exists(), "feature flag off preserves agent dir");

        // Flag on → agent dir reclaimed.
        let s = run_forced(&ttl_zero()).unwrap();
        assert_eq!(s.reclaimed, 1);
        assert!(!agent_dir.exists());
    });
}

#[test]
fn non_agent_prefix_in_claude_dir_is_ignored() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        // A random dir that isn't an agent-* worktree — must not be touched.
        let random = repo.join(".claude/worktrees/not-an-agent");
        fs::create_dir_all(&random).unwrap();
        fs::write(random.join("do-not-delete"), "preserve me").unwrap();
        set_mtime_back(&random, Duration::from_secs(48 * 3600));
        register(&repo);

        let s = run_forced(&ttl_zero()).unwrap();
        assert_eq!(s.reclaimed + s.salvaged, 0);
        assert!(random.exists(), "non-agent-* dir must not be swept");
    });
}

#[test]
fn cross_repo_iteration_continues_on_failure() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let a = bootstrap_repo(home, "a");
        let wt_a = add_worktree_aged(&a, "w1", Duration::from_secs(48 * 3600));
        // b is registered but has no .cas/ — unhealthy.
        let b = home.join("b-moved");
        register(&a);
        let store = open_host_known_repo_store().unwrap();
        store.upsert(&b).unwrap();

        let s = run_forced(&ttl_zero()).unwrap();
        assert_eq!(s.repos_visited, 2);
        assert_eq!(
            s.reclaimed, 1,
            "healthy repo swept despite unhealthy sibling"
        );
        assert!(!wt_a.exists());
        let unhealthy_rec = s
            .per_repo
            .iter()
            .find(|r| r.repo_root.ends_with("b-moved"))
            .unwrap();
        assert!(unhealthy_rec.repo_error.is_some());
    });
}

#[test]
fn debounce_gates_run_if_due() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let _ = bootstrap_repo(home, "repo");
        register(&home.join("repo"));

        let config = WorktreesConfig {
            global_sweep_debounce_secs: 3600,
            ..ttl_zero()
        };
        // No debounce file yet → due.
        assert!(is_due(config.global_sweep_debounce_secs).unwrap());
        let first = run_if_due(&config).unwrap();
        assert!(first.is_some());
        assert!(debounce_file().exists());

        // Immediately re-run → debounce blocks.
        let second = run_if_due(&config).unwrap();
        assert!(second.is_none(), "debounce must block immediate re-run");
    });
}

#[test]
fn writes_summary_line_to_global_sweep_log() {
    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        let _wt = add_worktree_aged(&repo, "old", Duration::from_secs(48 * 3600));
        register(&repo);

        let _s = run_forced(&ttl_zero()).unwrap();
        let path = log_file();
        assert!(path.exists(), "sweep log must be created");
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("opportunistic-sweep"));
        assert!(contents.contains("reclaimed=1"));
        // Log must be additive across runs.
        let _s2 = run_forced(&ttl_zero()).unwrap();
        let contents2 = fs::read_to_string(&path).unwrap();
        let line_count = contents2.lines().count();
        assert!(line_count >= 2, "log must append, not truncate");
    });
}

#[test]
fn symlink_worktree_is_refused() {
    use std::os::unix::fs::symlink;

    TestEnvGuard::run_with_temp_home(|home| {
        ensure_host_schema().unwrap();
        let repo = bootstrap_repo(home, "repo");
        let real = add_worktree_aged(&repo, "real", Duration::from_secs(48 * 3600));
        // Place a symlink next to it pointing at itself-ish external target.
        let external = home.join("external");
        fs::create_dir_all(external.join(".git")).unwrap();
        set_mtime_back(&external, Duration::from_secs(48 * 3600));
        let link = repo.join(".cas/worktrees/linky");
        symlink(&external, &link).unwrap();
        register(&repo);

        let s = run_forced(&ttl_zero()).unwrap();
        let linky = s
            .per_repo
            .iter()
            .flat_map(|r| r.entries.iter())
            .find(|(p, _)| p == &link);
        if let Some((_, outcome)) = linky {
            assert!(matches!(outcome, OpportunisticOutcome::RefusedSymlink));
        }
        // External target must be untouched.
        assert!(external.join(".git").exists());
        // Real worktree DID get reclaimed (it's not a symlink).
        assert!(!real.exists());
    });
}
