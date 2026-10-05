//! Retired workers keep their checkout, but not regenerable target output.
//! Evidence is copied durably before quarantine and deletion; any doubt retains
//! the complete target. Parking a task is not worker retirement.
use super::*;

pub(crate) fn retire_worker(cas_root: &Path, agent: &cas_types::Agent) -> io::Result<bool> {
    let Some(worktree) = agent.metadata.get("clone_path") else {
        return Ok(false);
    };
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let artifacts = crate::config::project_factory_artifacts_root(
        cas_root,
        &crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref()),
    );
    // A worker's latest task is the receipt owner. Unassociated retired workers
    // have a separate inventory-only namespace, never closed-task artifact GC.
    let store = crate::store::open_task_store(cas_root).map_err(io::Error::other)?;
    let tasks = store.list(None).map_err(io::Error::other)?;
    let label = tasks
        .iter()
        .filter(|task| {
            task.assignee
                .as_deref()
                .is_some_and(|owner| owner == agent.id || owner == agent.name)
        })
        .max_by_key(|task| task.updated_at)
        .map(|task| task.id.as_str())
        .unwrap_or("_retired-workers");
    if Path::new(label).components().count() != 1 || label == "." || label == ".." {
        return Err(io::Error::other("invalid retirement artifact owner"));
    }
    retire(
        cas_root,
        Path::new(worktree),
        &artifacts.join(label),
        None,
        process_uses_many,
    )
}

fn retire(
    cas_root: &Path,
    worktree: &Path,
    artifacts: &Path,
    expected_head: Option<&str>,
    probe: impl Fn(&Path, &Path, &[i32]) -> bool,
) -> io::Result<bool> {
    let worktree = worktree.canonicalize()?;
    let workers = cas_root.join("worktrees").canonicalize()?;
    if worktree.parent() != Some(workers.as_path()) {
        return Ok(false);
    }
    let Some(repo) = cas_root.parent() else {
        return Ok(false);
    };
    let Some(identity) = list_validated_git_worktrees(repo)
        .into_iter()
        .find(|tree| tree.path.canonicalize().ok().as_ref() == Some(&worktree))
    else {
        return Ok(false);
    };
    let Some(head) = identity.commit.as_deref() else {
        return Ok(false);
    };
    if expected_head.is_some_and(|expected| expected != head) {
        return Ok(false);
    }
    let Some(_lane) = crate::factory_worker_check::try_lock_lane(cas_root, &worktree)? else {
        return Ok(false);
    };
    let target = worktree.join("target");
    if !target.exists() || fs::symlink_metadata(&target)?.file_type().is_symlink() {
        return Ok(false);
    }
    if list_validated_git_worktrees(repo)
        .iter()
        .any(|tree| tree.path.starts_with(&target))
    {
        return Ok(false);
    }
    // Acquire every Cargo profile/triple lock before moving any output. The
    // process probe excludes only these descriptors, never our entire PID.
    let mut locks = Vec::new();
    for entry in WalkDir::new(&target).max_depth(3).follow_links(false) {
        let entry = entry.map_err(io::Error::other)?;
        if entry.file_name() != ".cargo-lock" {
            continue;
        }
        if !entry.file_type().is_file() {
            return Ok(false);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry.path())?;
        if file.try_lock_exclusive().is_err() {
            return Ok(false);
        }
        locks.push(file);
    }
    #[cfg(unix)]
    let descriptors: Vec<_> = {
        use std::os::fd::AsRawFd;
        locks.iter().map(|file| file.as_raw_fd()).collect()
    };
    #[cfg(not(unix))]
    let descriptors: Vec<i32> = Vec::new();
    if probe(&target, &target, &descriptors) {
        let bytes: u64 = WalkDir::new(&target)
            .follow_links(false)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                entry
                    .metadata()
                    .ok()
                    .filter(|metadata| metadata.is_file())
                    .map(|metadata| metadata.len())
            })
            .sum();
        tracing::warn!(target = %target.display(), retained_bytes = bytes,
            "retired target deferred: live output or unavailable process evidence; legacy ownership not adopted");
        return Ok(false);
    }
    let stamp = uuid::Uuid::new_v4().to_string();
    let worker = worktree
        .file_name()
        .ok_or_else(|| io::Error::other("missing worker name"))?;
    let destination = artifacts
        .join("retired-target")
        .join(worker)
        .join(format!("{head}-{stamp}"));
    if destination.starts_with(&target) {
        return Err(io::Error::other("durable artifact root is inside target"));
    }
    fs::create_dir_all(&destination)?;
    if !destination
        .canonicalize()?
        .starts_with(artifacts.canonicalize()?)
    {
        return Err(io::Error::other(
            "retirement artifact path escapes durable root",
        ));
    }
    copy_evidence(&target, &destination)?;
    // Recheck identity/liveness after potentially large evidence copies.
    if probe(&target, &target, &descriptors)
        || !list_validated_git_worktrees(repo).iter().any(|tree| {
            tree.path.canonicalize().ok().as_ref() == Some(&worktree)
                && tree.commit.as_deref() == Some(head)
                && tree.git_admin_dir == identity.git_admin_dir
        })
    {
        return Ok(false);
    }
    let quarantine = worktree.join(format!("{QUARANTINE_PREFIX}retired-{stamp}"));
    fs::rename(&target, &quarantine)?;
    if probe(&target, &quarantine, &descriptors) {
        if !target.exists() {
            fs::rename(&quarantine, &target)?;
        }
        return Ok(false);
    }
    let receipt = serde_json::json!({
        "worktree": worktree, "head": head, "target": target,
        "evidence": destination, "status": "evidence-durable-before-reclamation",
    });
    let receipt_path = destination.join("RETIREMENT.json");
    if let Err(error) = durable_write(
        &receipt_path,
        &serde_json::to_vec_pretty(&receipt).map_err(io::Error::other)?,
    ) {
        if !target.exists() {
            fs::rename(&quarantine, &target)?;
        }
        return Err(error);
    }
    fs::remove_dir_all(&quarantine)?;
    tracing::info!(worktree = %worktree.display(), evidence = %destination.display(), "retired worker target reclaimed; checkout retained");
    Ok(true)
}

fn durable_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("missing artifact parent"))?,
    )?
    .sync_all()
}

fn copy_evidence(target: &Path, destination: &Path) -> io::Result<()> {
    // Standard profiles and Cargo target triples are regenerable. All other
    // files (logs, JSON receipts, nextest XML, fixture evidence) are preserved.
    let build_root = |entry: &walkdir::DirEntry| {
        entry.depth() == 1
            && (matches!(
                entry.file_name().to_str(),
                Some("debug" | "release" | "doc" | "package" | "tmp")
            ) || (entry.path().join("debug/.cargo-lock").exists()
                || entry.path().join("release/.cargo-lock").exists()))
    };
    for entry in WalkDir::new(target)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !build_root(entry)
                && !matches!(entry.file_name().to_str(), Some("node_modules" | ".cache"))
        })
    {
        let entry = entry.map_err(io::Error::other)?;
        let relative = entry
            .path()
            .strip_prefix(target)
            .map_err(io::Error::other)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if matches!(relative.to_str(), Some(".rustc_info.json" | "CACHEDIR.TAG")) {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "evidence symlink retained in target: {}",
                entry.path().display()
            )));
        }
        let output = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(output)?;
        } else if entry.file_type().is_file() {
            use std::io::Read;
            let mut source = fs::File::open(entry.path())?;
            let mut copied = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output)?;
            std::io::copy(&mut source, &mut copied)?;
            copied.sync_all()?;
            fs::File::open(
                output
                    .parent()
                    .ok_or_else(|| io::Error::other("missing artifact parent"))?,
            )?
            .sync_all()?;
            let mut source = fs::File::open(entry.path())?;
            let mut copied = fs::File::open(&output)?;
            let (mut original, mut duplicate) = ([0u8; 65536], [0u8; 65536]);
            loop {
                let count = source.read(&mut original)?;
                if count == 0 {
                    if copied.read(&mut duplicate[..1])? != 0 {
                        return Err(io::Error::other("evidence length verification failed"));
                    }
                    break;
                }
                copied.read_exact(&mut duplicate[..count])?;
                if original[..count] != duplicate[..count] {
                    return Err(io::Error::other("evidence verification failed"));
                }
            }
        } else {
            return Err(io::Error::other("non-regular target evidence retained"));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        super::super::tests::git(&repo, &["init", "-q"]);
        super::super::tests::git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        super::super::tests::git(&repo, &["config", "user.name", "Fixture"]);
        fs::write(repo.join("source"), "keep").unwrap();
        super::super::tests::git(&repo, &["add", "."]);
        super::super::tests::git(&repo, &["commit", "-qm", "fixture"]);
        let root = repo.join(".cas");
        let worker = root.join("worktrees/retired");
        fs::create_dir_all(worker.parent().unwrap()).unwrap();
        super::super::tests::git(
            &repo,
            &[
                "worktree",
                "add",
                "-qb",
                "factory/retired",
                worker.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(worker.join("target/debug/deps")).unwrap();
        fs::write(worker.join("target/debug/.cargo-lock"), "").unwrap();
        fs::write(worker.join("target/debug/deps/output"), "regenerable").unwrap();
        fs::write(worker.join("target/worker-check.log"), "proof log").unwrap();
        fs::create_dir_all(worker.join("target/nextest")).unwrap();
        fs::write(worker.join("target/nextest/junit.xml"), "proof receipt").unwrap();
        let artifacts = temp.path().join("artifacts/cas-fixture");
        (temp, root, worker, artifacts)
    }
    #[test]
    fn retirement_relocates_evidence_and_removes_whole_target_keeps_checkout_cas_72f4() {
        let (_temp, root, worker, artifacts) = fixture();
        assert!(retire(&root, &worker, &artifacts, None, |_, _, _| false).unwrap());
        assert!(!worker.join("target").exists());
        assert_eq!(fs::read_to_string(worker.join("source")).unwrap(), "keep");
        let paths: Vec<_> = WalkDir::new(&artifacts).into_iter().flatten().collect();
        assert!(
            paths
                .iter()
                .any(|entry| entry.file_name() == "worker-check.log"
                    && fs::read_to_string(entry.path()).unwrap() == "proof log")
        );
        assert!(paths.iter().any(|entry| entry.file_name() == "junit.xml"
            && fs::read_to_string(entry.path()).unwrap() == "proof receipt"));
        assert!(
            paths
                .iter()
                .any(|entry| entry.file_name() == "RETIREMENT.json")
        );
        assert!(!paths.iter().any(|entry| entry.file_name() == "output"));
    }
    #[test]
    fn retirement_preserves_live_lane_output_and_failed_evidence_copy_cas_72f4() {
        let (_temp, root, worker, artifacts) = fixture();
        let lane = crate::factory_worker_check::try_lock_lane(&root, &worker)
            .unwrap()
            .unwrap();
        assert!(!retire(&root, &worker, &artifacts, None, |_, _, _| false).unwrap());
        drop(lane);
        assert!(!retire(&root, &worker, &artifacts, None, |_, _, _| true).unwrap());
        fs::create_dir_all(&artifacts).unwrap();
        std::os::unix::fs::symlink(worker.join("source"), worker.join("target/unsafe-evidence"))
            .unwrap();
        assert!(retire(&root, &worker, &artifacts, None, |_, _, _| false).is_err());
        assert!(worker.join("target/worker-check.log").exists());
    }
}
