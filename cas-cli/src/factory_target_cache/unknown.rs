//! Explicit consent is authority to reclaim one cache, never proof of ownership.
use super::*;

/// Exact legacy layouts only. Missing Git metadata remains unknown ownership.
pub(super) fn worktree(cas_root: &Path, target: &Path) -> Option<PathBuf> {
    if !target.is_absolute()
        || target.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    if target == cas_root.join("merge-sweeps/assembly-target") {
        return Some(cas_root.join("merge-sweeps"));
    }
    let checkout = target.parent()?;
    (target.file_name().is_some_and(|name| name == "target")
        && missing_gitdir_checkout(cas_root, checkout).is_some())
    .then(|| checkout.to_path_buf())
}

pub(super) fn validate(
    cas_root: &Path,
    record: &TargetCacheRecord,
    consent: &Path,
    policy: TargetCachePolicy,
) -> Result<(), (CacheDisposition, String)> {
    let refuse = |reason: &str| (CacheDisposition::UnsafePath, reason.to_string());
    if record.ownership != CacheOwnership::Unknown || record.interrupted_cleanup {
        return Err(refuse(
            "explicit unknown-cache reclaim requires an inventoried unknown owner",
        ));
    }
    if policy.min_idle_secs == 0 {
        return Err((
            CacheDisposition::RecentWrite,
            "unknown-cache reclaim requires a positive target_cache_min_idle_secs quiet window"
                .into(),
        ));
    }
    if !consent.is_absolute() || consent.as_os_str() != record.path.as_os_str() {
        return Err(refuse(
            "opt-in must name the exact absolute cache path, never a checkout, parent or pattern",
        ));
    }
    for path in [cas_root, &record.worktree, &record.path] {
        if path.canonicalize().ok().as_deref() != Some(path) {
            return Err(refuse(
                "cache containment changed or contains a symlink/alias",
            ));
        }
    }
    if worktree(cas_root, &record.path).as_ref() != Some(&record.worktree) {
        return Err(refuse(
            "unknown-cache inventory changed; only missing-gitdir targets or the retired assembly target are reclaimable",
        ));
    }
    if matches!(
        record.disposition,
        CacheDisposition::Indeterminate
            | CacheDisposition::UnsafePath
            | CacheDisposition::LiveProcess
            | CacheDisposition::RecentWrite
    ) {
        return Err((
            record.disposition,
            format!("unknown-cache evidence is not eligible: {}", record.reason),
        ));
    }
    validate_evidence(cas_root, &record.path)?;
    for path in [&record.worktree, &record.path] {
        let metadata = fs::symlink_metadata(path).map_err(|error| refuse(&error.to_string()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || !owned_by_caller(&metadata) {
            return Err(refuse(
                "cache/checkout must be real directories owned by the caller",
            ));
        }
    }
    for entry in WalkDir::new(&record.path).follow_links(false) {
        let entry = entry.map_err(|error| refuse(&error.to_string()))?;
        let metadata = entry
            .metadata()
            .map_err(|error| refuse(&error.to_string()))?;
        if entry.file_type().is_symlink() || !owned_by_caller(&metadata) {
            return Err(refuse("cache contains a symlink or another user's files"));
        }
        if matches!(
            entry.file_name().to_str(),
            Some(".git" | "cas.db" | "LEDGER.md" | ".cas-scratch-owner.json")
        ) {
            return Err(refuse(
                "cache contains checkout/evidence or a managed scratch owner; use its owning protocol",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_evidence(
    cas_root: &Path,
    target: &Path,
) -> Result<(), (CacheDisposition, String)> {
    let refuse = |reason: &str| (CacheDisposition::UnsafePath, reason.to_string());
    // Evidence paths are independent of source/cache ownership, including a
    // custom artifacts root placed inside an otherwise regenerable target.
    let config = crate::config::Config::load(cas_root).map_err(|error| {
        (
            CacheDisposition::Indeterminate,
            format!("cannot establish artifact containment: {error}"),
        )
    })?;
    let artifacts = crate::config::resolved_factory_artifact_paths(
        cas_root,
        config.factory().artifacts_root.as_deref(),
    );
    for evidence in [
        artifacts.base,
        artifacts.project_root,
        cas_root.join("artifacts"),
    ] {
        let evidence = resolve_evidence_path(&evidence).map_err(|error| {
            (
                CacheDisposition::Indeterminate,
                format!("cannot resolve artifact containment: {error}"),
            )
        })?;
        if evidence.starts_with(target) || target.starts_with(&evidence) {
            return Err(refuse(
                "durable artifact evidence overlaps the named cache; nothing reclaimed",
            ));
        }
    }
    Ok(())
}

// Resolve existing symlink ancestors even when the artifact leaf has not been
// created yet. A lexical fallback could miss evidence aliased into the target.
fn resolve_evidence_path(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(io::Error::other(
            "artifact root must be an unambiguous absolute path",
        ));
    }
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while !ancestor.try_exists()? {
        if fs::symlink_metadata(ancestor).is_ok() {
            return Err(io::Error::other("artifact root has a dangling symlink"));
        }
        suffix.push(
            ancestor
                .file_name()
                .ok_or_else(|| io::Error::other("unresolved artifact root"))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| io::Error::other("unresolved artifact ancestor"))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

pub(super) fn owned_by_caller(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.uid() == unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}
