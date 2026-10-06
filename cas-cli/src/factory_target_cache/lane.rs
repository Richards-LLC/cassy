//! Disposable lane previews have explicit provenance and a lifetime owner lock.
//! Naming alone never grants destructive ownership of a detached checkout.
use super::*;

const MARKER: &str = ".cas-lane-compile.json";
const LOCK: &str = ".cas-lane-compile.lock";

#[derive(Deserialize)]
struct Provenance {
    version: u32,
    git_common_dir: PathBuf,
    head: String,
    #[serde(default)]
    worktree: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanePreviewRecord {
    pub worktree: PathBuf,
    pub bytes: u64,
    pub disposition: CacheDisposition,
    pub reason: String,
    #[serde(skip)]
    admin: Option<PathBuf>,
    #[serde(skip)]
    head: Option<String>,
}

// Legacy previews are metadata/preview; new previews are metadata's sibling.
// Derive metadata from the checkout name, never from an untrusted marker path.
fn metadata_dir(worktree: &Path) -> Option<PathBuf> {
    let name = worktree.file_name()?.to_str()?;
    if name == "preview" {
        let parent = worktree.parent()?;
        return parent
            .file_name()?
            .to_str()?
            .starts_with("lane-compile-")
            .then(|| parent.to_path_buf());
    }
    let metadata = name.strip_suffix("-preview")?;
    (!metadata.strip_prefix("lane-compile-")?.is_empty()).then(|| worktree.with_file_name(metadata))
}

pub(super) fn is_preview_path(cas_root: &Path, worktree: &Path) -> bool {
    let Some(metadata) = metadata_dir(worktree) else {
        return false;
    };
    let Some(parent) = metadata.parent().and_then(|path| path.canonicalize().ok()) else {
        return false;
    };
    Some(parent) == cas_root.join("worktrees").canonicalize().ok()
}

fn owned(cas_root: &Path, worktree: &Path) -> Option<GitWorktreeCandidate> {
    let parent = metadata_dir(worktree)?;
    if !is_preview_path(cas_root, worktree)
        || fs::symlink_metadata(worktree)
            .ok()?
            .file_type()
            .is_symlink()
        || fs::symlink_metadata(&parent).ok()?.file_type().is_symlink()
        || fs::symlink_metadata(parent.join(MARKER))
            .ok()?
            .file_type()
            .is_symlink()
    {
        return None;
    }
    let provenance: Provenance =
        serde_json::from_slice(&fs::read(parent.join(MARKER)).ok()?).ok()?;
    if provenance.version != 1
        || Some(provenance.git_common_dir) != git_common_dir(cas_root.parent()?)
        || match provenance.worktree {
            Some(bound) => bound != worktree,
            None => worktree.file_name()? != "preview",
        }
    {
        return None;
    }
    list_validated_git_worktrees(cas_root.parent()?)
        .into_iter()
        .find(|candidate| {
            candidate.path == worktree
                && candidate.branch.is_none()
                && candidate.commit.as_deref() == Some(provenance.head.as_str())
                && candidate.git_admin_dir.is_some()
        })
}

fn owner_lock(worktree: &Path) -> io::Result<Option<fs::File>> {
    let path = metadata_dir(worktree)
        .ok_or_else(|| io::Error::other("invalid lane preview layout"))?
        .join(LOCK);
    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Ok(None);
    }
    let lock = OpenOptions::new().read(true).write(true).open(path)?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(lock)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error),
    }
}

fn recent(worktree: &Path, min_idle_secs: u64) -> io::Result<bool> {
    let now = SystemTime::now();
    for entry in WalkDir::new(worktree).follow_links(false) {
        let metadata = entry
            .map_err(io::Error::other)?
            .metadata()
            .map_err(io::Error::other)?;
        if now.duration_since(metadata.modified()?).unwrap_or_default()
            < Duration::from_secs(min_idle_secs)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn inspect(
    cas_root: &Path,
    policy: TargetCachePolicy,
    live: &[PathBuf],
) -> Vec<LanePreviewRecord> {
    let Ok(entries) = fs::read_dir(cas_root.join("worktrees")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let worktree = if is_preview_path(cas_root, &path) {
                path
            } else {
                path.join("preview")
            };
            let candidate = owned(cas_root, &worktree)?;
            let bytes = scan_cache(&worktree, &worktree.join("target"), false)
                .map(|scan| scan.bytes)
                .unwrap_or(0);
            let (disposition, reason) = if live.iter().any(|root| root.starts_with(&worktree))
                || owner_lock(&worktree).ok().flatten().is_none()
                || live_process_uses(&worktree, &worktree.join("target"))
            {
                (
                    CacheDisposition::LiveProcess,
                    "live owner or unverifiable process evidence",
                )
            } else if recent(&worktree, policy.min_idle_secs).unwrap_or(true) {
                (
                    CacheDisposition::RecentWrite,
                    "preview contains recent source/output/evidence",
                )
            } else {
                (
                    CacheDisposition::Selected,
                    "stale owned disposable lane preview",
                )
            };
            Some(LanePreviewRecord {
                worktree,
                bytes,
                disposition,
                reason: reason.into(),
                admin: candidate.git_admin_dir,
                head: candidate.commit,
            })
        })
        .collect()
}

pub(super) fn cleanup(
    cas_root: &Path,
    records: &mut [LanePreviewRecord],
    policy: TargetCachePolicy,
    live: &[PathBuf],
) {
    for record in records
        .iter_mut()
        .filter(|record| record.disposition == CacheDisposition::Selected)
    {
        let Some(candidate) = owned(cas_root, &record.worktree) else {
            continue;
        };
        if record.admin.is_none()
            || candidate.git_admin_dir != record.admin
            || candidate.commit != record.head
        {
            record.disposition = CacheDisposition::OwnershipChanged;
            continue;
        }
        let Ok(Some(lock)) = owner_lock(&record.worktree) else {
            record.disposition = CacheDisposition::LiveProcess;
            continue;
        };
        let Some(lane) = crate::factory_worker_check::try_lock_lane(cas_root, &record.worktree)
            .ok()
            .flatten()
        else {
            record.disposition = CacheDisposition::LiveProcess;
            continue;
        };
        if live.iter().any(|root| root.starts_with(&record.worktree))
            || live_process_uses(&record.worktree, &record.worktree.join("target"))
        {
            record.disposition = CacheDisposition::LiveProcess;
            continue;
        }
        if recent(&record.worktree, policy.min_idle_secs).unwrap_or(true) {
            record.disposition = CacheDisposition::RecentWrite;
            continue;
        }
        // No --force: Git independently refuses source edits or untracked files.
        // Revalidate provenance after locks and liveness probes as well.
        if !owned(cas_root, &record.worktree)
            .is_some_and(|now| now.git_admin_dir == record.admin && now.commit == record.head)
        {
            record.disposition = CacheDisposition::OwnershipChanged;
            continue;
        }
        let result = std::process::Command::new("git")
            .current_dir(cas_root.parent().unwrap())
            .args(["worktree", "remove"])
            .arg(&record.worktree)
            .output();
        match result {
            Ok(output) if output.status.success() => {
                record.disposition = CacheDisposition::Reclaimed;
                record.reason = "removed stale lane checkout and Git registration".into();
                let parent = metadata_dir(&record.worktree).unwrap();
                let _ = fs::remove_file(parent.join(MARKER));
                let _ = fs::remove_file(parent.join(LOCK));
                let _ = fs::remove_dir(parent); // Preserve any parent-level evidence.
            }
            _ => {
                record.disposition = CacheDisposition::CleanupError;
                record.reason = "Git refused preview removal; source and evidence preserved".into();
            }
        }
        drop(lane);
        let _ = FileExt::unlock(&lock);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        fixture_layout(false)
    }

    fn fixture_layout(flat: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let git = super::super::tests::git;
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "lane@example.invalid"]);
        git(&repo, &["config", "user.name", "Lane fixture"]);
        fs::write(repo.join(".gitignore"), "/target/\n/.cas/\n").unwrap();
        fs::write(repo.join("source.rs"), "source").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "fixture"]);
        let root = repo.join(".cas");
        let parent = root.join("worktrees/lane-compile-fixture");
        fs::create_dir_all(&parent).unwrap();
        let preview = if flat {
            parent.with_file_name("lane-compile-fixture-preview")
        } else {
            parent.join("preview")
        };
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                preview.to_str().unwrap(),
                "HEAD",
            ],
        );
        let candidate = list_validated_git_worktrees(&repo)
            .into_iter()
            .find(|candidate| candidate.path == preview)
            .unwrap();
        let mut provenance = serde_json::json!({
            "version": 1, "git_common_dir": git_common_dir(&repo).unwrap(), "head": candidate.commit.unwrap()
        });
        if flat {
            provenance["worktree"] = serde_json::json!(preview);
        }
        fs::write(
            parent.join(MARKER),
            serde_json::to_vec(&provenance).unwrap(),
        )
        .unwrap();
        fs::write(parent.join(LOCK), "").unwrap();
        fs::create_dir_all(preview.join("target/debug")).unwrap();
        fs::write(preview.join("target/debug/output"), "cache").unwrap();
        (temp, root, preview)
    }

    fn policy() -> TargetCachePolicy {
        TargetCachePolicy {
            min_idle_secs: 0,
            retention_count: 0,
            high_watermark_percent: 100,
            low_watermark_percent: 99,
        }
    }

    #[test]
    fn flat_preview_owned_locked_and_reclaimed_cas_9dd1() {
        let (_temp, root, preview) = fixture_layout(true);
        let metadata = root.join("worktrees/lane-compile-fixture");
        assert!(is_preview_path(&root, &preview));
        assert!(owned(&root, &preview).is_some());
        // Exercise the unchanged production target ownership guard too.
        fs::remove_dir_all(preview.join("target")).unwrap();
        let target_owner = owner::acquire(&root, &preview).unwrap().unwrap();
        drop(target_owner);
        let held = owner_lock(&preview).unwrap().unwrap();
        assert_eq!(
            inspect(&root, policy(), &[])[0].disposition,
            CacheDisposition::LiveProcess
        );
        FileExt::unlock(&held).unwrap();
        drop(held);
        let reclaim = super::super::tests::reclamation_available();
        assert_eq!(
            inspect(
                &root,
                TargetCachePolicy {
                    min_idle_secs: u64::MAX,
                    ..policy()
                },
                &[]
            )[0]
            .disposition,
            if reclaim {
                CacheDisposition::RecentWrite
            } else {
                CacheDisposition::LiveProcess
            }
        );
        let mut records = inspect(&root, policy(), &[]);
        fs::write(metadata.join("proof.log"), "durable proof").unwrap();
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(
            records[0].disposition,
            if reclaim {
                CacheDisposition::Reclaimed
            } else {
                CacheDisposition::LiveProcess
            }
        );
        assert_eq!(preview.exists(), !reclaim);
        assert!(root.join("worktrees").is_dir());
        assert_eq!(
            fs::read_to_string(metadata.join("proof.log")).unwrap(),
            "durable proof"
        );
    }

    #[test]
    fn flat_preview_requires_bound_provenance_cas_9dd1() {
        let (_temp, root, preview) = fixture_layout(true);
        let marker = root.join("worktrees/lane-compile-fixture").join(MARKER);
        assert!(owned(&root, &preview).is_some());
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
        for field in ["worktree", "head", "git_common_dir"] {
            let mut bad = original.clone();
            bad[field] = serde_json::json!("incorrect");
            fs::write(&marker, serde_json::to_vec(&bad).unwrap()).unwrap();
            assert!(owned(&root, &preview).is_none());
            assert!(inspect(&root, policy(), &[]).is_empty());
        }
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove("worktree");
        fs::write(&marker, serde_json::to_vec(&missing).unwrap()).unwrap();
        assert!(owned(&root, &preview).is_none());
        fs::remove_file(&marker).unwrap();
        let external = root.join("external-provenance");
        fs::write(&external, serde_json::to_vec(&original).unwrap()).unwrap();
        std::os::unix::fs::symlink(&external, &marker).unwrap();
        assert!(owned(&root, &preview).is_none());
        assert!(preview.exists());
    }

    #[test]
    fn flat_preview_revalidates_builder_and_dirty_source_cas_9dd1() {
        let (_temp, root, preview) = fixture_layout(true);
        let mut records = inspect(&root, policy(), &[]);
        assert_eq!(records.len(), 1);
        records[0].disposition = CacheDisposition::Selected;
        let held = crate::factory_worker_check::try_lock_lane(&root, &preview)
            .unwrap()
            .unwrap();
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(records[0].disposition, CacheDisposition::LiveProcess);
        drop(held);
        records[0].disposition = CacheDisposition::Selected;
        let mut untrusted: Vec<LanePreviewRecord> =
            serde_json::from_str(&serde_json::to_string(&records).unwrap()).unwrap();
        cleanup(&root, &mut untrusted, policy(), &[]);
        assert_eq!(untrusted[0].disposition, CacheDisposition::OwnershipChanged);
        fs::write(preview.join("source.rs"), "reader edits must survive").unwrap();
        // Preserve the reader's pending Git diff across the attempted cleanup.
        let pending_edit = || {
            let output = std::process::Command::new("git")
                .current_dir(&preview)
                .args(["diff", "--exit-code"])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(!output.stdout.is_empty());
            output.stdout
        };
        let before = pending_edit();
        let mut records = inspect(&root, policy(), &[]);
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(
            records[0].disposition,
            if super::super::tests::reclamation_available() {
                CacheDisposition::CleanupError
            } else {
                CacheDisposition::LiveProcess
            }
        );
        assert_eq!(pending_edit(), before);
        assert!(
            list_validated_git_worktrees(root.parent().unwrap())
                .iter()
                .any(|candidate| candidate.path == preview)
        );
    }

    #[test]
    fn live_owner_recent_preview_and_stale_removal_cas_29b0() {
        let (_temp, root, preview) = fixture();
        let reclaim = super::super::tests::reclamation_available();
        let held = owner_lock(&preview).unwrap().unwrap();
        assert_eq!(
            inspect(&root, policy(), &[])[0].disposition,
            CacheDisposition::LiveProcess
        );
        FileExt::unlock(&held).unwrap();
        drop(held);
        assert_eq!(
            inspect(
                &root,
                TargetCachePolicy {
                    min_idle_secs: u64::MAX,
                    ..policy()
                },
                &[]
            )[0]
            .disposition,
            if reclaim {
                CacheDisposition::RecentWrite
            } else {
                CacheDisposition::LiveProcess
            }
        );
        let mut stale = inspect(&root, policy(), &[]);
        let expected = if reclaim {
            CacheDisposition::Selected
        } else {
            CacheDisposition::LiveProcess
        };
        assert_eq!(stale[0].disposition, expected);
        assert_eq!(stale[0].bytes, 5);
        let report = super::super::inspect(&root, policy(), &[], &[], true).unwrap();
        assert!(
            report
                .caches
                .iter()
                .any(|cache| cache.worktree == preview && cache.bytes == 5)
        );
        assert_eq!(report.lane_previews[0].disposition, expected);
        let parent = preview.parent().unwrap().to_path_buf();
        fs::write(parent.join("proof.log"), "durable proof").unwrap();
        cleanup(&root, &mut stale, policy(), &[]);
        assert_eq!(
            stale[0].disposition,
            if reclaim {
                CacheDisposition::Reclaimed
            } else {
                CacheDisposition::LiveProcess
            }
        );
        assert_eq!(preview.exists(), !reclaim);
        assert_eq!(
            list_validated_git_worktrees(root.parent().unwrap())
                .iter()
                .any(|candidate| candidate.path == preview),
            !reclaim
        );
        assert_eq!(
            fs::read_to_string(parent.join("proof.log")).unwrap(),
            "durable proof"
        );
    }

    #[test]
    fn revalidation_preserves_new_builder_dirty_source_and_untrusted_report_cas_29b0() {
        let (_temp, root, preview) = fixture();
        let mut records = inspect(&root, policy(), &[]);
        // Exercise admission revalidation even when initial native inspection
        // cannot establish idle on this host.
        records[0].disposition = CacheDisposition::Selected;
        let held = crate::factory_worker_check::try_lock_lane(&root, &preview)
            .unwrap()
            .unwrap();
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(records[0].disposition, CacheDisposition::LiveProcess);
        assert!(preview.exists());
        drop(held);
        let mut records = inspect(&root, policy(), &[]);
        records[0].disposition = CacheDisposition::Selected;
        let mut untrusted: Vec<LanePreviewRecord> =
            serde_json::from_str(&serde_json::to_string(&records).unwrap()).unwrap();
        cleanup(&root, &mut untrusted, policy(), &[]);
        assert_eq!(untrusted[0].disposition, CacheDisposition::OwnershipChanged);
        assert!(preview.exists());
        fs::write(preview.join("source.rs"), "reader edits must survive").unwrap();
        let before = fs::read(preview.join("source.rs")).unwrap();
        let reclaim = super::super::tests::reclamation_available();
        let mut records = inspect(&root, policy(), &[]);
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(
            records[0].disposition,
            if reclaim {
                CacheDisposition::CleanupError
            } else {
                CacheDisposition::LiveProcess
            }
        );
        assert_eq!(fs::read(preview.join("source.rs")).unwrap(), before);
    }

    #[test]
    fn preview_without_provenance_is_inventory_only_cas_29b0() {
        let (_temp, root, preview) = fixture();
        fs::remove_file(preview.parent().unwrap().join(MARKER)).unwrap();
        assert!(inspect(&root, policy(), &[]).is_empty());
        let report = super::super::inspect(&root, policy(), &[], &[], true).unwrap();
        assert!(
            report
                .caches
                .iter()
                .any(|cache| cache.worktree == preview && cache.bytes == 5)
        );
        assert!(preview.exists());
    }
}
