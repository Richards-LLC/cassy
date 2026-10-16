//! `[factory] scratch_root` resolution for worker build output (cas-3f93).
//!
//! Every reader of a worker's Cargo target or the published build-cache
//! baseline resolves it here, so spawn, seeding, the capped worker check,
//! ownership, parking and retirement agree on one location. The path layout
//! itself is `cas_types::factory_scratch`; this module adds the project
//! configuration lookup and the same-filesystem guard that seeding needs.
//!
//! Unset (the default) keeps `<worktree>/target` and `<cas>/build-cache`.

use std::io;
use std::path::{Path, PathBuf};

/// The configured scratch root for one Cassy store, if it relocates anything.
/// A value that is not absolute, or a config that cannot load, keeps the
/// legacy layout and says so in the log rather than guessing a base.
pub(crate) fn configured_scratch_root(cas_dir: &Path) -> Option<PathBuf> {
    if !cas_dir.join("config.toml").exists() && !cas_dir.join("config.yaml").exists() {
        return None;
    }
    let configured = match crate::config::Config::load(cas_dir) {
        Ok(config) => config.factory().scratch_root,
        Err(error) => {
            tracing::warn!(cas_dir = %cas_dir.display(), %error,
                "factory.scratch_root unreadable; worker build output stays in-tree");
            return None;
        }
    };
    let parsed = cas_types::factory_scratch::parse_scratch_root(configured.as_deref());
    if parsed.is_none() && configured.as_deref().is_some_and(|value| !value.trim().is_empty()) {
        tracing::warn!(value = ?configured,
            "factory.scratch_root is not an absolute path; worker build output stays in-tree");
    }
    parsed
}

/// The private Cargo target directory of a worker checkout.
pub(crate) fn worker_target_dir(worktree: &Path) -> PathBuf {
    let root = cas_types::factory_scratch::managed_worktree_store(worktree)
        .and_then(configured_scratch_root);
    cas_types::factory_scratch::worker_target_dir(root.as_deref(), worktree)
}

/// Where published target baselines (`current` and `snapshots/`) live.
pub(crate) fn build_cache_dir(cas_dir: &Path) -> PathBuf {
    cas_types::factory_scratch::build_cache_dir(configured_scratch_root(cas_dir).as_deref(), cas_dir)
}

/// True when `target` is not the checkout's in-tree `target/`.
pub(crate) fn is_relocated(worktree: &Path, target: &Path) -> bool {
    target != worktree.join("target")
}

/// The path itself, or its nearest existing ancestor.
pub(crate) fn existing_ancestor(path: &Path) -> Option<&Path> {
    let mut current = path;
    while std::fs::symlink_metadata(current).is_err() {
        current = current.parent()?;
    }
    Some(current)
}

/// Device id of the filesystem that holds (or would hold) `path`.
#[cfg(unix)]
pub(crate) fn filesystem_id(path: &Path) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let existing = existing_ancestor(path).ok_or_else(|| {
        io::Error::other(format!("no existing ancestor for {}", path.display()))
    })?;
    Ok(std::fs::metadata(existing)?.dev())
}

#[cfg(not(unix))]
pub(crate) fn filesystem_id(_path: &Path) -> io::Result<u64> {
    Ok(0)
}

/// Seeding hard-links baseline artifacts into a worker target, which only
/// works on one filesystem. A split layout is refused before any byte moves:
/// copying a multi-GB baseline per worker would silently trade the disk
/// problem for a slower one.
#[derive(Debug, thiserror::Error)]
#[error(
    "worker target seed refused: build-cache {} (device {build_cache_dev}) and worker target {} (device {target_dev}) are on different filesystems, so hard-link seeding is impossible and Cassy will not copy the baseline; keep both under one [factory] scratch_root (or both in-tree), then rebuild the baseline with scripts/refresh-worker-build-cache.sh",
    build_cache.display(),
    target.display()
)]
pub(crate) struct CrossFilesystemSeed {
    pub build_cache: PathBuf,
    pub build_cache_dev: u64,
    pub target: PathBuf,
    pub target_dev: u64,
}

pub(crate) fn ensure_same_filesystem(build_cache: &Path, target: &Path) -> anyhow::Result<()> {
    ensure_same_filesystem_with(build_cache, target, filesystem_id)
}

fn ensure_same_filesystem_with(
    build_cache: &Path,
    target: &Path,
    device: impl Fn(&Path) -> io::Result<u64>,
) -> anyhow::Result<()> {
    let build_cache_dev = device(build_cache)?;
    let target_dev = device(target)?;
    if build_cache_dev != target_dev {
        return Err(CrossFilesystemSeed {
            build_cache: build_cache.to_path_buf(),
            build_cache_dev,
            target: target.to_path_buf(),
            target_dev,
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed(dir: &Path, config: Option<&str>) -> (PathBuf, PathBuf) {
        let cas = dir.join(".cas");
        let worktree = cas.join("worktrees/worker-1");
        std::fs::create_dir_all(&worktree).unwrap();
        if let Some(config) = config {
            std::fs::write(cas.join("config.toml"), config).unwrap();
        }
        (cas, worktree)
    }

    #[test]
    fn scratch_root_unset_keeps_in_tree_target_and_build_cache() {
        let dir = tempfile::tempdir().unwrap();
        for config in [None, Some("[factory]\nspawn_min_free_gib = 0\n")] {
            let (cas, worktree) = managed(dir.path(), config);
            assert_eq!(worker_target_dir(&worktree), worktree.join("target"));
            assert_eq!(build_cache_dir(&cas), cas.join("build-cache"));
            assert!(!is_relocated(&worktree, &worker_target_dir(&worktree)));
        }
    }

    #[test]
    fn scratch_root_set_relocates_worker_target_and_build_cache() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("rewind");
        let (cas, worktree) = managed(
            dir.path(),
            Some(&format!("[factory]\nscratch_root = {:?}\n", scratch.display().to_string())),
        );
        let project = scratch.join(cas_types::factory_scratch::project_key(&cas));
        assert_eq!(worker_target_dir(&worktree), project.join("targets/worker-1"));
        assert_eq!(build_cache_dir(&cas), project.join("build-cache"));
        assert!(is_relocated(&worktree, &worker_target_dir(&worktree)));
        // The main checkout and other unmanaged checkouts keep target/.
        let main = dir.path().to_path_buf();
        assert_eq!(worker_target_dir(&main), main.join("target"));
    }

    #[test]
    fn scratch_root_relative_value_keeps_legacy_layout() {
        let dir = tempfile::tempdir().unwrap();
        let (cas, worktree) = managed(dir.path(), Some("[factory]\nscratch_root = \"rewind\"\n"));
        assert_eq!(worker_target_dir(&worktree), worktree.join("target"));
        assert_eq!(build_cache_dir(&cas), cas.join("build-cache"));
    }

    #[test]
    fn scratch_root_cross_filesystem_seed_is_refused_never_copied() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let target = dir.path().join("elsewhere/target");
        let error = ensure_same_filesystem_with(&cache, &target, |path| {
            Ok(if path.starts_with(dir.path().join("elsewhere")) { 2 } else { 1 })
        })
        .unwrap_err();
        let refusal = error.downcast_ref::<CrossFilesystemSeed>().expect("typed refusal");
        assert_eq!((refusal.build_cache_dev, refusal.target_dev), (1, 2));
        assert!(error.to_string().contains("different filesystems"), "{error}");
        assert!(error.to_string().contains("will not copy"), "{error}");
        ensure_same_filesystem_with(&cache, &target, |_| Ok(7)).unwrap();
        // The real probe measures the nearest existing ancestor.
        ensure_same_filesystem(&cache, &target).unwrap();
    }
}
