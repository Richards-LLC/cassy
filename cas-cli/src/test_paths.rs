//! Runtime path resolution for tests that can execute from a nextest archive.
//!
//! An archive may be compiled on a different machine from the one that runs
//! it. Compile-time Cargo paths therefore make suitable fallbacks, but not
//! runtime locations.

use std::path::{Path, PathBuf};

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
/// path, so fixture directories must be created beneath the process cwd. A
/// worker checkout's cwd may itself be under `.cas`; in that case, place the
/// fixture outside the checkout so discovery cannot inherit its Git origin or
/// walk through a `.cas` ancestor and mistake the fixture for Cassy state.
pub fn runtime_fixture_parent() -> PathBuf {
    let cwd = std::env::current_dir().expect("test current directory");
    fixture_parent_for(&cwd)
}

fn fixture_parent_for(cwd: &Path) -> PathBuf {
    let checkout_root = cwd
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == ".cas"))
        .and_then(Path::parent)
        .or_else(|| cwd.ancestors().find(|path| path.join(".git").exists()));
    checkout_root
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Finds the `cas` executable supplied alongside an archived test binary.
///
/// Unlike `assert_cmd::cargo::cargo_bin!`, this never embeds Cargo's producer
/// target directory into the consumer test executable.
pub fn cas_binary() -> PathBuf {
    binary("cas", None)
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
    use super::{find_workspace_root, fixture_parent_for, runtime_workspace_root};
    use std::path::Path;
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
    fn fixture_parent_escapes_a_worker_cas_directory() {
        let cwd = Path::new("/runner/cas-src/.cas/worktrees/worker");

        assert_eq!(fixture_parent_for(cwd), Path::new("/runner"));
    }

    #[test]
    fn fixture_parent_keeps_archive_cwd_outside_cas() {
        let cwd = Path::new("/runner/archive-run");

        assert_eq!(fixture_parent_for(cwd), cwd);
    }

    #[test]
    fn fixture_parent_escapes_a_regular_git_checkout() {
        let temp = tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".git")).unwrap();
        let cwd = temp.path().join("target/nextest");

        assert_eq!(fixture_parent_for(&cwd), temp.path().parent().unwrap());
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
