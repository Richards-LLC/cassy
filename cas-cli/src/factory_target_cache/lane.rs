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

pub(super) fn is_preview_path(cas_root: &Path, worktree: &Path) -> bool {
    worktree.file_name().is_some_and(|name| name == "preview")
        && worktree.parent().is_some_and(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lane-compile-"))
                && parent.parent().and_then(|path| path.canonicalize().ok())
                    == cas_root.join("worktrees").canonicalize().ok()
        })
}

fn owned(cas_root: &Path, worktree: &Path) -> Option<GitWorktreeCandidate> {
    let parent = worktree.parent()?;
    if worktree.file_name()? != "preview"
        || !parent
            .file_name()?
            .to_string_lossy()
            .starts_with("lane-compile-")
        || parent.parent()?.canonicalize().ok()?
            != cas_root.join("worktrees").canonicalize().ok()?
        || fs::symlink_metadata(parent).ok()?.file_type().is_symlink()
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
    let path = worktree.parent().unwrap().join(LOCK);
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
            let worktree = entry.path().join("preview");
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
                let parent = record.worktree.parent().unwrap();
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
        let preview = parent.join("preview");
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
        fs::write(parent.join(MARKER), serde_json::to_vec(&serde_json::json!({
            "version": 1, "git_common_dir": git_common_dir(&repo).unwrap(), "head": candidate.commit.unwrap()
        })).unwrap()).unwrap();
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
    fn live_owner_recent_preview_and_stale_removal_cas_29b0() {
        let (_temp, root, preview) = fixture();
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
            CacheDisposition::RecentWrite
        );
        let mut stale = inspect(&root, policy(), &[]);
        assert_eq!(stale[0].disposition, CacheDisposition::Selected);
        assert_eq!(stale[0].bytes, 5);
        let report = super::super::inspect(&root, policy(), &[], &[], true).unwrap();
        assert!(
            report
                .caches
                .iter()
                .any(|cache| cache.worktree == preview && cache.bytes == 5)
        );
        assert_eq!(
            report.lane_previews[0].disposition,
            CacheDisposition::Selected
        );
        let parent = preview.parent().unwrap().to_path_buf();
        fs::write(parent.join("proof.log"), "durable proof").unwrap();
        cleanup(&root, &mut stale, policy(), &[]);
        assert_eq!(stale[0].disposition, CacheDisposition::Reclaimed);
        assert!(!preview.exists());
        assert!(
            !list_validated_git_worktrees(root.parent().unwrap())
                .iter()
                .any(|candidate| candidate.path == preview)
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
        let held = crate::factory_worker_check::try_lock_lane(&root, &preview)
            .unwrap()
            .unwrap();
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(records[0].disposition, CacheDisposition::LiveProcess);
        assert!(preview.exists());
        drop(held);
        let records = inspect(&root, policy(), &[]);
        let mut untrusted: Vec<LanePreviewRecord> =
            serde_json::from_str(&serde_json::to_string(&records).unwrap()).unwrap();
        cleanup(&root, &mut untrusted, policy(), &[]);
        assert_eq!(untrusted[0].disposition, CacheDisposition::OwnershipChanged);
        assert!(preview.exists());
        fs::write(preview.join("source.rs"), "reader edits must survive").unwrap();
        let mut records = inspect(&root, policy(), &[]);
        cleanup(&root, &mut records, policy(), &[]);
        assert_eq!(records[0].disposition, CacheDisposition::CleanupError);
        assert_eq!(
            fs::read_to_string(preview.join("source.rs")).unwrap(),
            "reader edits must survive"
        );
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
