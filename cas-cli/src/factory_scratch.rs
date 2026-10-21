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

/// The filesystem probed by build admission and target-cache GC, including
/// before a configured scratch directory has been created.
pub(crate) fn build_filesystem(cas_dir: &Path, config: &crate::config::FactoryConfig) -> PathBuf {
    let target = cas_types::factory_scratch::parse_scratch_root(config.scratch_root.as_deref())
        .unwrap_or_else(|| cas_dir.to_path_buf());
    existing_ancestor(&target).unwrap_or(&target).to_path_buf()
}

/// The private Cargo target directory of a worker checkout.
pub(crate) fn worker_target_dir(worktree: &Path) -> PathBuf {
    let root = cas_types::factory_scratch::managed_worktree_store(worktree)
        .and_then(configured_scratch_root);
    cas_types::factory_scratch::worker_target_dir(root.as_deref(), worktree)
}

/// Where published target baselines (`current` and `snapshots/`) live.
pub(crate) fn build_cache_dir(cas_dir: &Path) -> PathBuf {
    cas_types::factory_scratch::build_cache_dir(
        configured_scratch_root(cas_dir).as_deref(),
        cas_dir,
    )
}

/// The release scratch base under the configured root (cas-baa8): release-gate
/// archives and assembly clones with their targets. `None` when unset, so the
/// release resolver keeps its checkout-filesystem base.
pub(crate) fn release_gate_base(cas_dir: &Path) -> Option<PathBuf> {
    configured_scratch_root(cas_dir)
        .map(|root| cas_types::factory_scratch::release_gate_base(&root, cas_dir))
}

/// One regenerable cache that `factory.scratch_root` relocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelocatedCache {
    /// `build-cache`, `target <worker>` or `release-gate`.
    pub label: String,
    pub path: PathBuf,
    pub bytes: u64,
}

/// Every relocated cache under this project's scratch directory; empty when
/// the root is unset (cas-baa8).
pub(crate) fn relocated_caches(cas_dir: &Path) -> Vec<RelocatedCache> {
    use cas_types::factory_scratch::{BUILD_CACHE_DIR, RELEASE_GATE_DIR, TARGETS_DIR};
    let Some(root) = configured_scratch_root(cas_dir) else {
        return Vec::new();
    };
    let project = cas_types::factory_scratch::project_scratch_dir(&root, cas_dir);
    let cache = |label: String, path: PathBuf| RelocatedCache {
        bytes: tree_bytes(&path),
        label,
        path,
    };
    let mut caches = Vec::new();
    let build_cache = project.join(BUILD_CACHE_DIR);
    if build_cache.is_dir() {
        caches.push(cache(BUILD_CACHE_DIR.to_string(), build_cache));
    }
    let mut targets: Vec<PathBuf> = std::fs::read_dir(project.join(TARGETS_DIR))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    targets.sort();
    for target in targets {
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        caches.push(cache(format!("target {name}"), target));
    }
    let release_gate = project.join(RELEASE_GATE_DIR);
    if release_gate.is_dir() {
        caches.push(cache(RELEASE_GATE_DIR.to_string(), release_gate));
    }
    caches
}

/// Apparent bytes of every regular file under `path`; symlinks are not
/// followed, so a link out of the scratch root is never counted.
fn tree_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

/// The `gc_report` section naming the relocated caches. Read-only.
pub(crate) fn render_relocated(cas_dir: &Path) -> String {
    let caches = relocated_caches(cas_dir);
    let Some(root) = configured_scratch_root(cas_dir) else {
        return String::new();
    };
    let total: u64 = caches.iter().map(|cache| cache.bytes).sum();
    let mut out = format!(
        "\nScratch root caches (factory.scratch_root {}): {} total\n",
        root.display(),
        bytes_label(total)
    );
    if caches.is_empty() {
        out.push_str("  none yet\n");
    }
    for cache in &caches {
        out.push_str(&format!(
            "  {}: {} ({})\n",
            cache.label,
            cache.path.display(),
            bytes_label(cache.bytes)
        ));
    }
    out.push_str("  Worker targets use owner/liveness checks in TARGET_CACHE_STATUS_JSON; build-cache and unmanaged directories require review.\n");
    let own_project = cas_types::factory_scratch::project_scratch_dir(&root, cas_dir);
    let mut review: Vec<_> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_symlink())
        })
        .map(|entry| entry.path())
        .filter(|path| *path != own_project)
        // Other project namespaces are managed by their own store, never by this GC.
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.rsplit_once('-'))
                .is_some_and(|(_, key)| {
                    key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        })
        .collect();
    review.sort();
    for path in review {
        out.push_str(&format!(
            "  review-only unmanaged scratch directory: {} (never auto-deleted)\n",
            path.display()
        ));
    }
    out
}

fn bytes_label(bytes: u64) -> String {
    format!("{bytes} bytes")
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
    let existing = existing_ancestor(path)
        .ok_or_else(|| io::Error::other(format!("no existing ancestor for {}", path.display())))?;
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
    fn scratch_root_unset_has_no_release_gate_base_or_relocated_report() {
        let dir = tempfile::tempdir().unwrap();
        let (cas, _) = managed(dir.path(), None);
        assert_eq!(release_gate_base(&cas), None);
        assert!(relocated_caches(&cas).is_empty());
        assert_eq!(render_relocated(&cas), "");
    }

    #[test]
    fn scratch_root_gc_report_lists_relocated_caches_with_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("rewind");
        let (cas, _) = managed(
            dir.path(),
            Some(&format!("[factory]\nscratch_root = {:?}\n", scratch.display().to_string())),
        );
        let project = scratch.join(cas_types::factory_scratch::project_key(&cas));
        let write = |rel: &str, bytes: usize| {
            let path = project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![b'x'; bytes]).unwrap();
        };
        write("build-cache/snapshots/a/lib.rlib", 10);
        write("targets/worker-1/debug/lib.rlib", 20);
        write("targets/worker-2/debug/lib.rlib", 30);
        write("release-gate/assembly-clone-x/repo/target/f", 5);
        assert_eq!(release_gate_base(&cas), Some(project.join("release-gate/base")));
        let caches = relocated_caches(&cas);
        let summary: Vec<(&str, u64)> =
            caches.iter().map(|cache| (cache.label.as_str(), cache.bytes)).collect();
        assert_eq!(
            summary,
            [
                ("build-cache", 10),
                ("target worker-1", 20),
                ("target worker-2", 30),
                ("release-gate", 5),
            ]
        );
        let report = render_relocated(&cas);
        assert!(report.contains("Scratch root caches"), "{report}");
        assert!(report.contains(&project.join("targets/worker-2").display().to_string()), "{report}");
        assert!(report.contains("65 bytes"), "{report}");
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
