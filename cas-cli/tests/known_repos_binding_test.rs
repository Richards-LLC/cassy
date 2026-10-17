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
    for (name, _) in
        std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("CAS_"))
    {
        command.env_remove(name);
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    command
}

fn json_output(home: &Path, args: &[&str]) -> serde_json::Value {
    let output = cas(home)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap_or_else(|error| {
        panic!(
            "{args:?} must emit one JSON document: {error}; stdout={}",
            String::from_utf8_lossy(&output)
        )
    })
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
    // pin: field names and empty collections are the public CLI JSON contract.
    for (command, empty) in [
        ("list", serde_json::json!({"repos": [], "binding_count": 0})),
        ("status", serde_json::json!({"bindings": []})),
    ] {
        for args in [
            vec!["known-repos", command, "--json"],
            vec!["--json", "known-repos", command],
        ] {
            assert_eq!(json_output(&home_path, &args), empty);
        }
    }
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

    for args in [
        ["known-repos", "list", "--json"],
        ["--json", "known-repos", "list"],
    ] {
        let report = json_output(&home_path, &args);
        assert_eq!(report["binding_count"], 1);
        let repos = report["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 2);
        // Binding registers/touches the chosen clone once more.
        for (path, touches) in [(&clone_a, 1), (&clone_b, 2)] {
            let row = repos
                .iter()
                .find(|row| row["path"] == path.to_str().unwrap())
                .unwrap();
            assert_eq!(row["healthy"], true);
            assert_eq!(row["touch_count"], touches);
        }
    }
    // pin: the status document preserves the selector/root and validated state.
    let expected = serde_json::json!({"bindings": [{
        "selector": "remote:github.com/org/shared",
        "repo_root": clone_b,
        "state": "valid",
    }]});
    for args in [
        ["known-repos", "status", "--json"],
        ["--json", "known-repos", "status"],
    ] {
        assert_eq!(json_output(&home_path, &args), expected);
    }

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
