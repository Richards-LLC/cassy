use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should start");
    assert_success("git", args, &output);
    String::from_utf8(output.stdout).expect("git output should be UTF-8")
}

fn assert_success(program: &str, args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "{program} {args:?} failed ({}):\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cargo_bin() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

fn run_cargo(package_root: &Path, target_dir: &Path, args: &[&str]) -> String {
    let manifest = package_root.join("Cargo.toml");
    let mut cargo_args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    cargo_args.push("--offline".to_string());
    cargo_args.extend([
        "--manifest-path".to_string(),
        manifest.display().to_string(),
    ]);
    if args.first() != Some(&"generate-lockfile") {
        cargo_args.extend(["--target-dir".to_string(), target_dir.display().to_string()]);
    }
    let output = Command::new(cargo_bin())
        .args(&cargo_args)
        .current_dir(package_root)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_NET_OFFLINE", "true")
        .env_remove("CAS_POSTHOG_API_KEY")
        .env_remove("CAS_SENTRY_DSN")
        .env_remove("POSTHOG_API_KEY")
        .env_remove("SENTRY_DSN")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .expect("cargo should start");
    assert_success("cargo", args, &output);
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn cargo_run_hash(package_root: &Path, target_dir: &Path) -> String {
    let output = run_cargo(package_root, target_dir, &["run", "--quiet"]);
    output.trim().to_string()
}

fn assert_no_missing_input(output: &str) {
    assert!(
        !output.contains("Dirty") || !output.contains("missing"),
        "Cargo reported a missing rerun input:\n{output}"
    );
}

#[test]
fn offline_fixture_registry_classifier_accepts_only_missing_fixture_dependencies() {
    assert!(offline_fixture_registry_missing(
        "error: no matching package named `chrono` found\n\
         location searched: crates.io index\n\
         As a reminder, you're using offline mode (--offline)"
    ));
    assert!(offline_fixture_registry_missing(
        "error: failed to select a version for the requirement `dotenvy = \"^0.15\"`\n\
         location searched: crates.io index\n\
         As a reminder, you're using offline mode (--offline)"
    ));
    assert!(!offline_fixture_registry_missing(
        "error: could not execute process `sccache rustc -vV` (never executed)\n\
         Caused by: No such file or directory"
    ));
    assert!(!offline_fixture_registry_missing(
        "error: no matching package named `serde` found\n\
         As a reminder, you're using offline mode (--offline)"
    ));
    assert!(!offline_fixture_registry_missing(
        "error: no matching package named `chrono` found\n\
         location searched: crates.io index"
    ));
}

fn create_fixture() -> (TempDir, PathBuf, PathBuf) {
    let repo = tempfile::tempdir().expect("fixture tempdir");
    let package = repo.path().join("cas-cli");
    let source = package.join("src");
    let dist = repo.path().join("hub-web/dist");
    fs::create_dir_all(&source).expect("fixture source directory");
    fs::create_dir_all(&dist).expect("fixture dist directory");

    let production_build_script = std::env::current_dir()
        .expect("test current directory")
        .join("build.rs");
    assert!(
        production_build_script.is_file(),
        "resolve production build script from runtime current_dir(): {}",
        production_build_script.display()
    );
    fs::copy(production_build_script, package.join("build.rs"))
        .expect("copy production build script");
    fs::write(
        package.join("Cargo.toml"),
        r#"[package]
name = "build-watch-fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

[build-dependencies]
chrono = "0.4"
dotenvy = "0.15"
"#,
    )
    .expect("fixture manifest");
    fs::write(
        source.join("main.rs"),
        r#"fn main() {
    println!("{}", option_env!("CAS_GIT_HASH").unwrap_or("unknown"));
    println!("{}", option_env!("CAS_POSTHOG_API_KEY").unwrap_or("missing"));
}
"#,
    )
    .expect("fixture source");
    for asset in [
        "index.html",
        "app.js",
        "app.css",
        "ghostty-vt.wasm",
        "ghostty-write-pty.wasm",
        "symbols.woff2",
    ] {
        fs::write(dist.join(asset), asset).expect("fixture Hub asset");
    }

    run_git(repo.path(), &["init", "-q", "-b", "main"]);
    run_git(repo.path(), &["config", "user.name", "CAS build test"]);
    run_git(
        repo.path(),
        &["config", "user.email", "cas-build-test@example.invalid"],
    );
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "-q", "-m", "fixture"]);

    let linked = repo.path().join("linked");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    (repo, package, linked.join("cas-cli"))
}

#[test]
fn cargo_build_script_stays_fresh_and_tracks_worktree_transitions() {
    let (repo, normal_package, linked_package) = create_fixture();
    let normal_target = repo.path().join("target");
    let linked_target = repo.path().join("linked/target");
    // Generate each worktree's lockfile before the first build. Cargo creates
    // it lazily otherwise, which is itself a parent-inventory transition.
    run_cargo(&normal_package, &normal_target, &["generate-lockfile"]);
    run_cargo(&linked_package, &linked_target, &["generate-lockfile"]);

    fs::write(
        linked_package.join(".env"),
        "CAS_POSTHOG_API_KEY=package-initial\n",
    )
    .expect("create package-local optional .env before the first build");

    let normal_first = run_cargo(&normal_package, &normal_target, &["check"]);
    assert_no_missing_input(&normal_first);
    let normal_second = run_cargo(&normal_package, &normal_target, &["check"]);
    assert_no_missing_input(&normal_second);
    assert!(
        !normal_second.contains("Compiling build-watch-fixture"),
        "unchanged normal checkout reran its build script:\n{normal_second}"
    );

    let linked_first = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&linked_first);
    let linked_second = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&linked_second);
    assert!(
        !linked_second.contains("Compiling build-watch-fixture"),
        "unchanged linked checkout reran its build script:\n{linked_second}"
    );
    let initial_env_output = cargo_run_hash(&linked_package, &linked_target);
    assert!(
        initial_env_output
            .lines()
            .any(|line| line == "package-initial"),
        "the initial package-local .env was not embedded:\n{initial_env_output}"
    );

    run_git(
        repo.path(),
        &["-C", "linked", "pack-refs", "--all", "--prune"],
    );
    run_cargo(&linked_package, &linked_target, &["check"]);
    let packed_hash = cargo_run_hash(&linked_package, &linked_target);

    let linked_repo = repo.path().join("linked");
    let old_linked_sha = run_git(&linked_repo, &["rev-parse", "refs/heads/linked"])
        .trim()
        .to_string();
    let tree = run_git(&linked_repo, &["rev-parse", "HEAD^{tree}"])
        .trim()
        .to_string();
    let head_path = PathBuf::from(
        run_git(
            &linked_repo,
            &["rev-parse", "--path-format=absolute", "--git-path", "HEAD"],
        )
        .trim(),
    );
    let index_path = PathBuf::from(
        run_git(
            &linked_repo,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )
        .trim(),
    );
    let head_before = fs::read(&head_path).expect("read linked HEAD before update-ref");
    let index_before = fs::read(&index_path).expect("read linked index before update-ref");
    let new_linked_sha = run_git(
        &linked_repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            &old_linked_sha,
            "-m",
            "packed-to-loose",
        ],
    )
    .trim()
    .to_string();
    run_git(
        &linked_repo,
        &[
            "update-ref",
            "refs/heads/linked",
            &new_linked_sha,
            &old_linked_sha,
        ],
    );
    assert_eq!(
        fs::read(&head_path).expect("read linked HEAD after update-ref"),
        head_before,
        "update-ref must not rewrite linked HEAD"
    );
    assert_eq!(
        fs::read(&index_path).expect("read linked index after update-ref"),
        index_before,
        "update-ref must not rewrite linked index"
    );
    let loose_transition = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&loose_transition);
    let loose_hash = cargo_run_hash(&linked_package, &linked_target);
    assert_ne!(
        loose_hash, packed_hash,
        "packed-to-loose branch update did not refresh embedded metadata"
    );

    let newer_linked_sha = run_git(
        &linked_repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            &new_linked_sha,
            "-m",
            "loose-to-loose",
        ],
    )
    .trim()
    .to_string();
    run_git(
        &linked_repo,
        &[
            "update-ref",
            "refs/heads/linked",
            &newer_linked_sha,
            &new_linked_sha,
        ],
    );
    assert_eq!(
        fs::read(&head_path).expect("read linked HEAD after loose update-ref"),
        head_before,
        "updating a loose branch ref must not rewrite linked HEAD"
    );
    assert_eq!(
        fs::read(&index_path).expect("read linked index after loose update-ref"),
        index_before,
        "updating a loose branch ref must not rewrite linked index"
    );
    let loose_update = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&loose_update);
    let loose_update_hash = cargo_run_hash(&linked_package, &linked_target);
    assert_ne!(
        loose_update_hash, loose_hash,
        "updating an existing loose branch ref did not refresh embedded metadata"
    );

    let detached_sha = run_git(
        &linked_repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            &newer_linked_sha,
            "-m",
            "detached-head",
        ],
    )
    .trim()
    .to_string();
    run_git(&linked_repo, &["checkout", "-q", "--detach", &detached_sha]);
    let detached_update = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&detached_update);
    let detached_hash = cargo_run_hash(&linked_package, &linked_target);
    assert_ne!(
        detached_hash, loose_update_hash,
        "checking out a detached HEAD did not refresh embedded metadata"
    );
    let detached_repeat = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&detached_repeat);
    assert!(
        !detached_repeat.contains("Compiling build-watch-fixture"),
        "unchanged detached HEAD reran its build script:\n{detached_repeat}"
    );

    fs::write(
        linked_package.join(".env"),
        "CAS_POSTHOG_API_KEY=package-updated\n",
    )
    .expect("update package-local optional .env");
    let package_env_update = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&package_env_update);
    let package_env_output = cargo_run_hash(&linked_package, &linked_target);
    assert!(
        package_env_output
            .lines()
            .any(|line| line == "package-updated"),
        "updating an existing package-local .env did not invalidate the build:\n{package_env_output}"
    );
    fs::remove_file(linked_package.join(".env")).expect("remove package-local optional .env");
    let package_env_delete = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&package_env_delete);
    let package_env_repeat = run_cargo(&linked_package, &linked_target, &["check"]);
    assert_no_missing_input(&package_env_repeat);
    assert!(
        !package_env_repeat.contains("Compiling build-watch-fixture"),
        "unchanged linked checkout after .env deletion reran its build script:\n{package_env_repeat}"
    );
}
