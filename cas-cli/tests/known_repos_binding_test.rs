use std::path::Path;
use std::process::Command;

use cas_store::{KnownRepoStore, SqliteKnownRepoStore};
use predicates::prelude::*;
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cas(home: &Path) -> assert_cmd::Command {
    let mut command = assert_cmd::Command::new(cas::test_paths::cas_binary());
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    command
}

#[test]
fn explicit_binding_cli_recovers_two_live_clones_across_restart() {
    // The clone roots are real known-repository fixtures, not disposable
    // scratch state, so discovery must be able to see them.
    let home = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let clone_a = home_path.join("clone-a");
    let clone_b = home_path.join("clone-b");
    for repo in [&clone_a, &clone_b] {
        std::fs::create_dir(repo).unwrap();
        git(repo, &["init", "-q", "-b", "main"]);
        git(
            repo,
            &["remote", "add", "origin", "git@github.com:org/shared.git"],
        );
        std::fs::create_dir(repo.join(".cas")).unwrap();
    }

    // Bootstrap the production host schema, then model two already-known
    // live clones sharing one portable selector.
    cas(&home_path)
        .args(["known-repos", "list"])
        .assert()
        .success();
    let store = SqliteKnownRepoStore::open(&home_path.join(".cas")).unwrap();
    store.upsert(&clone_a).unwrap();
    store.upsert(&clone_b).unwrap();
    drop(store);

    cas(&home_path)
        .args(["known-repos", "bind", "--repo", clone_b.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Bound selector `remote:github.com/org/shared`",
        ))
        .stdout(predicate::str::contains(clone_b.to_str().unwrap()))
        .stdout(predicate::str::contains(
            "Portable task and delivery records remain path-free",
        ));

    // A new process observes the persisted valid binding.
    cas(&home_path)
        .args(["known-repos", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[valid] remote:github.com/org/shared",
        ))
        .stdout(predicate::str::contains(clone_b.to_str().unwrap()));

    // A different clone cannot silently replace the operator's live choice.
    cas(&home_path)
        .args(["known-repos", "bind", "--repo", clone_a.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "already bound to a different host repository",
        ));

    cas(&home_path)
        .args(["known-repos", "unbind", "remote:github.com/org/shared"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Repository registration and files were not changed",
        ));
    cas(&home_path)
        .args(["known-repos", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "No explicit host-local repository bindings",
        ));
    cas(&home_path)
        .args(["known-repos", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(clone_a.to_str().unwrap()))
        .stdout(predicate::str::contains(clone_b.to_str().unwrap()));
    assert!(clone_a.join(".git").is_dir());
    assert!(clone_b.join(".git").is_dir());
}
