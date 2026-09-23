//! Runtime path resolution for tests that can execute from a nextest archive.
//!
//! An archive may be compiled on a different machine from the one that runs
//! it. Compile-time Cargo paths therefore make suitable fallbacks, but not
//! runtime locations.

use std::path::{Path, PathBuf};

/// Install a shell test double at `path` after its first macOS launch has
/// completed outside the command-under-test's deadline. Nextest runs tests in
/// separate processes, so the content-addressed target and ready marker live
/// beside the test executable and are shared across those processes.
#[cfg(unix)]
pub fn warm_stub(path: &Path, script: &str) {
    use fs2::FileExt;
    use sha2::{Digest, Sha256};
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;

    let (shebang, body) = script
        .split_once('\n')
        .expect("shell stub must have a shebang and body");
    assert!(shebang.starts_with("#!"), "shell stub needs a shebang");
    let digest = hex::encode(Sha256::digest(script.as_bytes()));
    let cache = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("test executable directory")
        .join("warm-test-stubs");
    fs::create_dir_all(&cache).expect("stub cache directory");
    let shared = cache.join(format!("{digest}.sh"));
    let ready = cache.join(format!("{digest}.ready"));
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(cache.join(format!("{digest}.lock")))
        .expect("stub cache lock");
    lock.lock_exclusive().expect("lock stub cache");
    if !shared.exists() {
        let staged = cache.join(format!("{digest}.{}.tmp", std::process::id()));
        // The warm-up guard prevents commands in the fixture from recording
        // calls, sleeping, or changing state during the assessment launch.
        fs::write(
            &staged,
            format!("{shebang}\n[ \"${{CAS_TEST_STUB_WARMUP:-}}\" = 1 ] && exit 0\n{body}"),
        )
        .expect("write stub");
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))
            .expect("make stub executable");
        fs::rename(&staged, &shared).expect("publish stub");
    }
    if !ready.exists() {
        let output = Command::new(&shared)
            .env("CAS_TEST_STUB_WARMUP", "1")
            .output()
            .expect("launch stub warm-up");
        assert!(output.status.success(), "stub warm-up failed: {output:?}");
        fs::write(&ready, b"ready").expect("record warmed stub");
    }
    lock.unlock().expect("unlock stub cache");
    if path.exists() || path.is_symlink() {
        fs::remove_file(path).expect("replace test stub");
    }
    symlink(&shared, path).expect("link warmed test stub");
}

#[cfg(all(test, unix))]
mod warm_stub_tests {
    use super::warm_stub;
    use std::fs;
    use std::process::Command;

    #[test]
    fn warm_launch_is_side_effect_free_and_paths_share_an_assessed_target() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("called");
        let script = format!("#!/bin/sh\nprintf called >> '{}'\n", marker.display());
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        warm_stub(&first, &script);
        assert!(!marker.exists(), "warm-up must not run fixture commands");
        warm_stub(&second, &script);
        assert_eq!(first.canonicalize().unwrap(), second.canonicalize().unwrap());
        assert!(Command::new(&first).status().unwrap().success());
        assert_eq!(fs::read_to_string(marker).unwrap(), "called");
    }
}

#[cfg(not(unix))]
pub fn warm_stub(path: &Path, script: &str) {
    std::fs::write(path, script).expect("write test stub");
}

/// Create a private hub fixture beneath a canonical temporary directory.
/// macOS commonly spells TMPDIR through /var, a symlink rejected by hub state.
pub fn private_hub_tempdir() -> tempfile::TempDir {
    let parent = std::env::temp_dir()
        .canonicalize()
        .expect("temporary directory must be canonicalizable");
    let temp = tempfile::tempdir_in(parent).expect("canonical temporary fixture directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private temporary fixture directory");
    }
    temp
}

/// Finds the checkout containing the archived test at runtime.
pub fn workspace_root() -> PathBuf {
    for key in ["CAS_TEST_WORKSPACE_ROOT", "NEXTEST_WORKSPACE_ROOT"] {
        if let Some(path) = std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
        {
            return path;
        }
    }
    runtime_workspace_root().unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("cas-cli manifest has workspace parent")
            .to_path_buf()
    })
}

/// Returns the `cas-cli` crate root inside [`workspace_root`].
pub fn crate_root() -> PathBuf {
    workspace_root().join("cas-cli")
}

/// Returns the runtime parent for temporary test fixtures.
///
/// `CARGO_MANIFEST_DIR` records the path on the machine that built an
/// archived test. Nextest executes the archive from a checkout at a different
/// path, so fixture directories must be created beneath the process cwd.
pub fn runtime_fixture_parent() -> PathBuf {
    std::env::current_dir().expect("test current directory")
}

/// Finds the `cas` executable supplied alongside an archived test binary.
///
/// Unlike `assert_cmd::cargo::cargo_bin!`, this never embeds Cargo's producer
/// target directory into the consumer test executable.
pub fn cas_binary() -> PathBuf {
    binary("cas", None)
}

/// Reads a process starttime through the daemon's PID identity parser.
///
/// Integration tests use this to distinguish a dead process from a new
/// process that has been assigned the same PID by the kernel.
pub fn pid_starttime(pid: u32) -> Option<u64> {
    crate::mcp::daemon::read_pid_starttime(pid)
}

/// Finds an executable supplied alongside an archived test binary.
///
/// Explicit test configuration and nextest's runtime variable win. The
/// compile-time Cargo value is retained only for ordinary local test runs.
pub fn binary(name: &str, baked: Option<PathBuf>) -> PathBuf {
    let upper = name.to_ascii_uppercase().replace('-', "_");
    for key in [
        format!("CAS_TEST_BIN_{upper}"),
        format!("NEXTEST_BIN_EXE_{upper}"),
        format!("CARGO_BIN_EXE_{upper}"),
    ] {
        if let Some(path) = std::env::var_os(&key)
            .map(PathBuf::from)
            .filter(|path| path.is_file())
        {
            return path;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors() {
            let candidate = ancestor.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let candidate = std::env::current_dir()
        .unwrap_or_default()
        .join("target/debug")
        .join(name);
    if candidate.is_file() {
        return candidate;
    }
    baked.unwrap_or_else(|| PathBuf::from(name))
}

fn runtime_workspace_root() -> Option<PathBuf> {
    [std::env::current_dir().ok(), std::env::current_exe().ok()]
        .into_iter()
        .flatten()
        .find_map(|base| find_workspace_root(&base))
}

fn find_workspace_root(base: &Path) -> Option<PathBuf> {
    base.ancestors()
        .find(|path| {
            std::fs::read_to_string(path.join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace]"))
        })
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::{find_workspace_root, runtime_workspace_root};
    use tempfile::tempdir;

    #[test]
    fn finds_workspace_from_a_runtime_child_path() {
        let temp = tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();
        let child = temp.path().join("target/nextest/default");
        std::fs::create_dir_all(&child).unwrap();

        assert_eq!(find_workspace_root(&child).as_deref(), Some(temp.path()));
    }

    #[test]
    fn skips_member_manifests_for_the_workspace_root() {
        let temp = tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .unwrap();
        let member = temp.path().join("member");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(member.join("Cargo.toml"), "[package]\nname = \"member\"\n").unwrap();

        assert_eq!(find_workspace_root(&member).as_deref(), Some(temp.path()));
    }

    #[test]
    fn current_runtime_has_a_workspace_root() {
        assert!(runtime_workspace_root().is_some());
    }

    #[test]
    fn binary_uses_baked_path_only_after_runtime_candidates_miss() {
        let temp = tempdir().unwrap();
        let baked = temp.path().join("producer-only-cas");

        assert_eq!(
            super::binary("cas-f83c-no-such-binary", Some(baked.clone())),
            baked
        );
    }
}
