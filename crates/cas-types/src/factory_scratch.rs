//! Layout of the optional `[factory] scratch_root` (cas-3f93).
//!
//! When an operator sets `factory.scratch_root`, regenerable worker build
//! output moves off the checkout's filesystem to a per-project directory under
//! that root:
//!
//! ```text
//! <scratch_root>/<project-key>/build-cache/        published target baselines
//! <scratch_root>/<project-key>/targets/<worktree>  one private Cargo target per worker
//! ```
//!
//! Both live under one project directory so seeding can hard-link baseline
//! artifacts into a worker target. The project key is the same
//! `<label>-<sha256>` namespace the durable artifacts root uses, so two
//! projects sharing one scratch drive never share a cache.
//!
//! These functions are pure path arithmetic. Callers decide whether the root
//! is configured; unset keeps every legacy path unchanged.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Directory under a project's scratch namespace holding worker targets.
pub const TARGETS_DIR: &str = "targets";
/// Directory under a project's scratch namespace holding target baselines.
pub const BUILD_CACHE_DIR: &str = "build-cache";
/// Directory under a project's scratch namespace that is the release scratch
/// base: release-gate archives and every assembly clone and its target
/// (cas-baa8). It shares the build-cache's filesystem, so clone seeding can
/// hard-link the baseline.
pub const RELEASE_GATE_DIR: &str = "release-gate";

/// `<label>-<sha256>` namespace for one Cassy store. The label is the store's
/// project directory name (at most 48 characters, restricted to
/// `[A-Za-z0-9_-]`); the hash covers the canonical store path, so equally
/// named projects stay apart. `scripts/refresh-worker-build-cache.sh` derives
/// the same key; both are pinned to one golden vector.
pub fn project_key(cas_dir: &Path) -> String {
    let store = cas_dir
        .canonicalize()
        .unwrap_or_else(|_| cas_dir.to_path_buf());
    let label = store
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .chars()
        .take(48)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let hash = Sha256::digest(store.as_os_str().as_encoded_bytes());
    format!("{label}-{hash:x}")
}

/// The project's directory beneath a configured scratch root.
pub fn project_scratch_dir(scratch_root: &Path, cas_dir: &Path) -> PathBuf {
    scratch_root.join(project_key(cas_dir))
}

/// Where target baselines live: `<cas_dir>/build-cache` when unset.
pub fn build_cache_dir(scratch_root: Option<&Path>, cas_dir: &Path) -> PathBuf {
    match scratch_root {
        Some(root) => project_scratch_dir(root, cas_dir).join(BUILD_CACHE_DIR),
        None => cas_dir.join(BUILD_CACHE_DIR),
    }
}

/// The release scratch base under a configured scratch root (cas-baa8).
/// Unset has no answer here: the release resolver keeps deriving a base on
/// the checkout's filesystem.
pub fn release_gate_base(scratch_root: &Path, cas_dir: &Path) -> PathBuf {
    // cas-baa8 red: stubbed until the relocation lands.
    let _ = scratch_root;
    cas_dir.join("merge-sweeps")
}

/// A managed worker checkout is a direct child of `<cas_dir>/worktrees`.
/// Returns that store directory, or `None` for any other checkout (the main
/// checkout, a lane preview or a custom worktree root), which keeps its
/// in-tree `target/`.
pub fn managed_worktree_store(worktree: &Path) -> Option<&Path> {
    let parent = worktree.parent()?;
    if parent.file_name()? != "worktrees" {
        return None;
    }
    worktree.file_name()?;
    parent.parent()
}

/// Private Cargo target for a worker checkout: `<worktree>/target` when the
/// root is unset or the checkout is not managed, otherwise
/// `<scratch_root>/<project-key>/targets/<worktree-name>`.
pub fn worker_target_dir(scratch_root: Option<&Path>, worktree: &Path) -> PathBuf {
    let relocated = scratch_root.zip(managed_worktree_store(worktree));
    match (relocated, worktree.file_name()) {
        (Some((root, cas_dir)), Some(name)) => project_scratch_dir(root, cas_dir)
            .join(TARGETS_DIR)
            .join(name),
        _ => worktree.join("target"),
    }
}

/// Parse a configured value: only an absolute path relocates anything.
pub fn parse_scratch_root(value: Option<&str>) -> Option<PathBuf> {
    let value = value.map(str::trim).filter(|value| !value.is_empty())?;
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden vector shared with scripts/test-release-worker-build-cache.py.
    #[test]
    fn scratch_root_project_key_matches_golden_vector() {
        assert_eq!(
            project_key(Path::new("/srv/demo repo/.cas")),
            "demo-repo-57694f919d7a09378d7bd9dde4b21bd900b11644a97685c8a94b79ecc97046c8"
        );
    }

    #[test]
    fn scratch_root_unset_keeps_legacy_paths() {
        let worktree = Path::new("/srv/repo/.cas/worktrees/bold-parrot-33");
        assert_eq!(
            worker_target_dir(None, worktree),
            Path::new("/srv/repo/.cas/worktrees/bold-parrot-33/target")
        );
        assert_eq!(
            build_cache_dir(None, Path::new("/srv/repo/.cas")),
            Path::new("/srv/repo/.cas/build-cache")
        );
    }

    #[test]
    fn scratch_root_relocates_managed_worker_targets_and_build_cache() {
        let root = Path::new("/mnt/rewind/cas");
        let cas = Path::new("/srv/repo/.cas");
        let key = project_key(cas);
        assert_eq!(
            worker_target_dir(Some(root), &cas.join("worktrees/bold-parrot-33")),
            root.join(&key).join("targets/bold-parrot-33")
        );
        assert_eq!(
            build_cache_dir(Some(root), cas),
            root.join(&key).join("build-cache")
        );
    }

    #[test]
    fn scratch_root_release_gate_base_shares_the_project_namespace() {
        let root = Path::new("/mnt/rewind/cas");
        let cas = Path::new("/srv/repo/.cas");
        let project = root.join(project_key(cas));
        assert_eq!(release_gate_base(root, cas), project.join("release-gate"));
        assert_eq!(
            release_gate_base(root, cas).parent(),
            build_cache_dir(Some(root), cas).parent()
        );
    }

    #[test]
    fn scratch_root_leaves_unmanaged_checkouts_in_tree() {
        let root = Path::new("/mnt/rewind/cas");
        for checkout in ["/srv/repo", "/srv/repo/.cas/lanes/preview", "/"] {
            let checkout = Path::new(checkout);
            assert_eq!(worker_target_dir(Some(root), checkout), checkout.join("target"));
        }
    }

    #[test]
    fn scratch_root_accepts_only_absolute_values() {
        assert_eq!(parse_scratch_root(None), None);
        assert_eq!(parse_scratch_root(Some("  ")), None);
        assert_eq!(parse_scratch_root(Some("relative/cas")), None);
        assert_eq!(
            parse_scratch_root(Some(" /mnt/rewind/cas ")),
            Some(PathBuf::from("/mnt/rewind/cas"))
        );
    }
}
