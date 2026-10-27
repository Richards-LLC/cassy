#[path = "../build_support.rs"]
mod build_support;

use build_support::rust_target_to_zig;

#[test]
fn native_release_targets_are_pinned_to_zig_baselines() {
    assert_eq!(
        rust_target_to_zig("x86_64-unknown-linux-gnu"),
        Ok("x86_64-linux-gnu")
    );
    assert_eq!(
        rust_target_to_zig("aarch64-apple-darwin"),
        Ok("aarch64-macos")
    );
}

#[test]
fn supported_cross_targets_keep_their_abi() {
    assert_eq!(
        rust_target_to_zig("x86_64-unknown-linux-musl"),
        Ok("x86_64-linux-musl")
    );
    assert_eq!(
        rust_target_to_zig("aarch64-unknown-linux-gnu"),
        Ok("aarch64-linux-gnu")
    );
}

#[test]
fn unknown_targets_fail_closed_with_actionable_diagnostics() {
    let error = rust_target_to_zig("riscv64gc-unknown-linux-gnu").unwrap_err();
    assert!(error.contains("refusing to build"), "{error}");
    assert!(error.contains("riscv64gc-unknown-linux-gnu"), "{error}");
    assert!(error.contains("x86_64-unknown-linux-gnu"), "{error}");
}

#[cfg(unix)]
#[test]
fn linked_worktree_finds_main_checkout_zig_without_env_or_local_install() {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main checkout");
    let worktree = temp.path().join("linked worktree");
    std::fs::create_dir(&main).unwrap();
    let main = main.canonicalize().unwrap();
    let git = |args: &[&OsStr]| {
        let output = Command::new("git")
            .current_dir(&main)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&[OsStr::new("init"), OsStr::new("-q")]);
    git(&[
        OsStr::new("commit"),
        OsStr::new("--allow-empty"),
        OsStr::new("-qm"),
        OsStr::new("fixture"),
    ]);
    git(&[
        OsStr::new("worktree"),
        OsStr::new("add"),
        OsStr::new("--detach"),
        worktree.as_os_str(),
    ]);
    let worktree = worktree.canonicalize().unwrap();
    let zig = main.join(".context/zig/zig");
    std::fs::create_dir_all(zig.parent().unwrap()).unwrap();
    std::fs::write(&zig, "#!/bin/sh\nprintf '0.15.2\\n'\n").unwrap();
    std::fs::set_permissions(&zig, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!worktree.join(".context").exists());

    // Use the same resolver as build.rs; only Git is needed on the host PATH.
    assert_eq!(
        build_support::find_zig(&worktree, None, Some(OsStr::new(""))).unwrap(),
        zig
    );

    // A broken shared installation must report every attempted candidate and
    // distinguish an incompatible version from an unexecutable compiler.
    for (body, mode, diagnostic) in [
        (
            "#!/bin/sh\nprintf '0.16.0\\n'\n",
            0o755,
            "version 0.16.0, expected 0.15.2",
        ),
        ("#!/bin/sh\nprintf '0.15.2\\n'\n", 0o644, "cannot execute"),
    ] {
        std::fs::write(&zig, body).unwrap();
        std::fs::set_permissions(&zig, std::fs::Permissions::from_mode(mode)).unwrap();
        let configured = worktree.join("missing-configured-zig");
        let path = worktree.join("empty-bin");
        let error = build_support::find_zig(
            &worktree,
            Some(configured.as_os_str()),
            Some(path.as_os_str()),
        )
        .unwrap_err();
        for candidate in [
            &configured,
            &path.join("zig"),
            &worktree.join(".context/zig/zig"),
            &zig,
        ] {
            assert!(error.contains(&candidate.display().to_string()), "{error}");
        }
        assert!(error.contains(diagnostic), "{error}");
        assert!(error.contains("./scripts/bootstrap-zig.sh"), "{error}");
    }
    // In the main checkout, the local and Git-common-dir candidate coincide.
    let error = build_support::find_zig(&main, None, Some(OsStr::new(""))).unwrap_err();
    let candidate = format!("  - {}:", zig.display());
    assert_eq!(
        error
            .lines()
            .filter(|line| line.starts_with(&candidate))
            .count(),
        1,
        "{error}"
    );
}

// Moved from factory_worker_check: discovery belongs to the build script,
// including explicit overrides and relative ZIG/PATH entries.
#[cfg(unix)]
#[test]
fn zig_resolution_precedence() {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo with spaces");
    let install = |path: &std::path::Path| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\nprintf '0.15.2\\n'\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let local = repo.join(".context/zig/zig");
    let configured = repo.join("configured-zig");
    let on_path = repo.join("bin/zig");
    for path in [&local, &configured, &on_path] {
        install(path);
    }
    assert_eq!(
        build_support::find_zig(
            &repo,
            Some(OsStr::new("configured-zig")),
            Some(OsStr::new("bin"))
        )
        .unwrap(),
        configured
    );
    assert_eq!(
        build_support::find_zig(&repo, Some(OsStr::new("missing")), Some(OsStr::new("bin")))
            .unwrap(),
        on_path
    );
    assert_eq!(
        build_support::find_zig(&repo, None, Some(OsStr::new(""))).unwrap(),
        local
    );
}
