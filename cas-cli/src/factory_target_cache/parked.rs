//! Keep only a bounded number of parked, private check caches. Proof logs and
//! nextest reports live outside `target/debug` and survive reclamation.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct ParkedCache {
    worktree: PathBuf,
    admin: PathBuf,
    head: String,
}

fn park_path(cas_root: &Path, worktree: &Path) -> PathBuf {
    let key = hex::encode(Sha256::digest(worktree.as_os_str().as_encoded_bytes()));
    cas_root
        .join("worker-target-parks")
        .join(format!("{key}.json"))
}

pub(crate) fn resume(cas_root: &Path, worktree: &Path) -> io::Result<()> {
    match fs::remove_file(park_path(cas_root, worktree)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn identity(cas_root: &Path, worktree: &Path) -> Option<ParkedCache> {
    let worktree = worktree.canonicalize().ok()?;
    let workers = cas_root.join("worktrees").canonicalize().ok()?;
    // Never prune the main checkout, an external worktree or a lane preview here.
    if worktree.parent() != Some(workers.as_path()) {
        return None;
    }
    let candidate = list_validated_git_worktrees(cas_root.parent()?)
        .into_iter()
        .find(|candidate| candidate.path.canonicalize().ok().as_ref() == Some(&worktree))?;
    Some(ParkedCache {
        worktree,
        admin: candidate.git_admin_dir?,
        head: candidate.commit?,
    })
}

/// Called after the delivery parks/closes, never from the critical task write.
/// It is safe to skip a contended cache; the next park retries the inventory.
pub(crate) fn park(
    cas_root: &Path,
    worktree: &Path,
    retention: usize,
    expected_head: Option<&str>,
) -> io::Result<()> {
    let Some(current) = identity(cas_root, worktree) else {
        return Ok(());
    };
    if expected_head.is_some_and(|head| head != current.head) {
        return Ok(()); // A delayed retirement job must not mark a newer delivery parked.
    }
    let directory = cas_root.join("worker-target-parks");
    fs::create_dir_all(&directory)?;
    // The runner removes its parked marker under this same lane lock before
    // reseeding/spawning. No evictor can mistake a resumed builder for parked.
    let Some(lane) = crate::factory_worker_check::try_lock_lane(cas_root, &current.worktree)?
    else {
        return Ok(());
    };
    let path = park_path(cas_root, &current.worktree);
    let temporary = path.with_extension("partial");
    fs::write(
        &temporary,
        serde_json::to_vec(&current).map_err(io::Error::other)?,
    )?;
    fs::rename(temporary, path)?;
    drop(lane);

    let gc = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cas_root.join("target-cache-gc.lock"))?;
    if gc.try_lock_exclusive().is_err() {
        return Ok(());
    }

    let mut inventory: Vec<_> = fs::read_dir(&directory)?
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|entry| {
            let record: ParkedCache = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let now = identity(cas_root, &record.worktree)?;
            if now.admin != record.admin
                || now.head != record.head
                || !record.worktree.join("target/debug").is_dir()
            {
                return None;
            }
            Some((
                entry.metadata().ok()?.modified().ok()?,
                entry.path(),
                record,
            ))
        })
        .collect();
    inventory.sort_by_key(|(modified, path, _)| (std::cmp::Reverse(*modified), path.clone()));
    for (_, path, parked) in inventory.into_iter().skip(retention) {
        let Some(lane) = crate::factory_worker_check::try_lock_lane(cas_root, &parked.worktree)?
        else {
            continue;
        };
        // Re-read the marker and Git identity after lock acquisition: a new
        // check could have resumed between inventory and admission.
        if !path.exists()
            || !identity(cas_root, &parked.worktree)
                .is_some_and(|now| now.admin == parked.admin && now.head == parked.head)
        {
            continue;
        }
        prune_debug(&parked.worktree)?;
        drop(lane);
    }
    FileExt::unlock(&gc)?;
    Ok(())
}

fn prune_debug(worktree: &Path) -> io::Result<()> {
    let target = worktree.join("target");
    let debug = target.join("debug");
    if !debug.exists() {
        return Ok(());
    }
    // Refuse symlinks at both roots; neither source nor output may escape.
    if fs::symlink_metadata(&target)?.file_type().is_symlink()
        || fs::symlink_metadata(&debug)?.file_type().is_symlink()
        || debug.canonicalize()?.parent() != Some(target.canonicalize()?.as_path())
    {
        return Ok(());
    }
    if fs::symlink_metadata(debug.join(".cargo-lock"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Ok(());
    }
    let cargo = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(debug.join(".cargo-lock"))?;
    if cargo.try_lock_exclusive().is_err() {
        return Ok(());
    }
    // Ignore only this process's own lock handle. An unrelated open artifact
    // (including a still-running test binary) preserves the complete cache.
    if output_in_use(&debug) {
        return Ok(());
    }
    let quarantine = target.join(format!(".cas-parked-debug-{}", std::process::id()));
    if quarantine.exists() {
        return Ok(());
    }
    fs::rename(&debug, &quarantine)?;
    if output_in_use(&quarantine) || output_in_use(&debug) {
        if !debug.exists() {
            fs::rename(&quarantine, &debug)?;
        }
        return Ok(());
    }
    let result = fs::remove_dir_all(quarantine);
    FileExt::unlock(&cargo)?;
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        super::super::tests::git(&repo, &["init", "-q"]);
        super::super::tests::git(&repo, &["config", "user.email", "cache@example.invalid"]);
        super::super::tests::git(&repo, &["config", "user.name", "Cache fixture"]);
        fs::write(repo.join(".gitignore"), "/target/\n/.cas/\n").unwrap();
        fs::write(repo.join("source.rs"), "fixture").unwrap();
        super::super::tests::git(&repo, &["add", "."]);
        super::super::tests::git(&repo, &["commit", "-qm", "fixture"]);
        fs::create_dir_all(repo.join(".cas/worktrees")).unwrap();
        (temp, repo.join(".cas"))
    }

    fn worker(root: &Path, name: &str) -> PathBuf {
        let path = root.join("worktrees").join(name);
        super::super::tests::git(
            root.parent().unwrap(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("factory/{name}"),
                path.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(path.join("target/debug/deps")).unwrap();
        fs::write(path.join("target/debug/deps/output"), "regenerable").unwrap();
        fs::write(path.join("target/worker-check.log"), "proof").unwrap();
        fs::create_dir_all(path.join("target/nextest")).unwrap();
        fs::write(path.join("target/nextest/proof.json"), "evidence").unwrap();
        path
    }

    #[test]
    fn bounded_parked_targets_preserve_logs_sources_and_independent_lanes_cas_29b0() {
        let (_temp, root) = fixture();
        let first = worker(&root, "first");
        let second = worker(&root, "second");
        // Independent builders never share a lock or mutable target.
        let a = crate::factory_worker_check::try_lock_lane(&root, &first)
            .unwrap()
            .unwrap();
        let b = crate::factory_worker_check::try_lock_lane(&root, &second)
            .unwrap()
            .unwrap();
        assert!(
            crate::factory_worker_check::try_lock_lane(&root, &first)
                .unwrap()
                .is_none()
        );
        drop((a, b));
        park(&root, &first, 0, None).unwrap();
        assert!(!first.join("target/debug").exists());
        park(&root, &second, 1, None).unwrap();
        assert!(second.join("target/debug").exists());
        for path in [&first, &second] {
            assert_eq!(
                fs::read_to_string(path.join("target/worker-check.log")).unwrap(),
                "proof"
            );
            assert_eq!(
                fs::read_to_string(path.join("target/nextest/proof.json")).unwrap(),
                "evidence"
            );
            assert_eq!(
                fs::read_to_string(path.join("source.rs")).unwrap(),
                "fixture"
            );
        }
        // N sequential deliveries leave at most the configured warm count.
        for index in 0..4 {
            let path = worker(&root, &format!("later-{index}"));
            park(&root, &path, 1, None).unwrap();
        }
        let count = fs::read_dir(root.join("worktrees"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().join("target/debug").exists())
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn live_builder_and_resumed_lane_cannot_be_pruned_cas_29b0() {
        let (_temp, root) = fixture();
        let worker = worker(&root, "builder");
        park(&root, &worker, 0, Some("stale delivery head")).unwrap();
        assert!(worker.join("target/debug").exists());
        assert!(!park_path(&root, &worker).exists());
        let held = crate::factory_worker_check::try_lock_lane(&root, &worker)
            .unwrap()
            .unwrap();
        park(&root, &worker, 0, None).unwrap();
        assert!(worker.join("target/debug").exists());
        drop(held);
        park(&root, &worker, 1, None).unwrap();
        let _held = crate::factory_worker_check::try_lock_lane(&root, &worker)
            .unwrap()
            .unwrap();
        resume(&root, &worker).unwrap();
        assert!(!park_path(&root, &worker).exists());
        assert!(worker.join("target/debug").exists());
    }

    #[test]
    fn open_output_handle_preserves_parked_cache_cas_29b0() {
        let (_temp, root) = fixture();
        let worker = worker(&root, "open-output");
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec 3<\"$1\"; printf ready; exec sleep 30")
            .arg("open-output-fixture")
            .arg(worker.join("target/debug/deps/output"))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Read;
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        let result = park(&root, &worker, 0, None);
        let preserved = worker.join("target/debug/deps/output").exists();
        let _ = child.kill();
        child.wait().unwrap();
        result.unwrap();
        assert!(preserved);
        park(&root, &worker, 0, None).unwrap();
        assert!(!worker.join("target/debug").exists());
    }
}
