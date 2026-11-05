//! Keep only a bounded number of parked, private check caches. Proof logs and
//! nextest reports live outside `target/debug` and survive reclamation.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct ParkedCache {
    worktree: PathBuf,
    admin: PathBuf,
    head: String,
    #[serde(default)]
    generation: String,
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
        generation: String::new(),
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
    park_at(cas_root, worktree, retention, expected_head, Path::new("/proc"))
}

fn park_at(
    cas_root: &Path,
    worktree: &Path,
    retention: usize,
    expected_head: Option<&str>,
    proc_root: &Path,
) -> io::Result<()> {
    let Some(mut current) = identity(cas_root, worktree) else {
        return Ok(());
    };
    if expected_head.is_some_and(|head| head != current.head) {
        return Ok(()); // A delayed retirement job must not mark a newer delivery parked.
    }
    // The cache can disappear after retirement was scheduled. There is no
    // inventory entry to create when only proof logs (or no target) remain.
    if !crate::factory_scratch::worker_target_dir(&current.worktree)
        .join("debug")
        .is_dir()
    {
        return Ok(());
    }
    let directory = cas_root.join("worker-target-parks");
    fs::create_dir_all(&directory)?;
    // The runner removes its parked marker under this same lane lock before
    // reseeding/spawning. No evictor can mistake a resumed builder for parked.
    let Some(lane) = crate::factory_worker_check::try_lock_lane(cas_root, &current.worktree)?
    else {
        return Ok(());
    };
    current.generation = uuid::Uuid::new_v4().to_string();
    let path = park_path(cas_root, &current.worktree);
    let temporary = path.with_extension("partial");
    fs::write(
        &temporary,
        serde_json::to_vec(&current).map_err(io::Error::other)?,
    )?;
    fs::rename(temporary, path)?;
    // cas-49163: the retained warm cache keeps its compiled dependencies but
    // not incremental state or per-commit test executables, which otherwise
    // grow without bound across every task the worker takes.
    trim_debug(&current.worktree, proc_root);
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
            // cas-49163: a removed worktree's marker can never resume; drop it.
            if !record.worktree.exists() {
                let _ = fs::remove_file(entry.path());
                return None;
            }
            let now = identity(cas_root, &record.worktree)?;
            if record.generation.is_empty()
                || now.admin != record.admin
                || now.head != record.head
                || !crate::factory_scratch::worker_target_dir(&record.worktree)
                    .join("debug")
                    .is_dir()
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
        if !marker_is_current(&path, &parked)
            || !identity(cas_root, &parked.worktree)
                .is_some_and(|now| now.admin == parked.admin && now.head == parked.head)
        {
            continue;
        }
        if let Err(error) = prune_debug(&parked.worktree, proc_root) {
            tracing::warn!(%error, worktree = %parked.worktree.display(), "parked worker cache prune failed");
        }
        drop(lane);
    }
    FileExt::unlock(&gc)?;
    // cas-49163: also reclaim idle targets of gone workers and resolved review
    // walks now, instead of only once admission is already refusing lanes.
    if let Err(error) = super::reclaim_idle_at(cas_root, proc_root) {
        tracing::warn!(%error, "idle target reclaim after park deferred");
    }
    Ok(())
}

fn marker_is_current(path: &Path, recorded: &ParkedCache) -> bool {
    !recorded.generation.is_empty()
        && fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ParkedCache>(&bytes).ok())
            .is_some_and(|current| current == *recorded)
}

/// cas-49163: the lane lock and this `.cargo-lock` are held, so the guarded
/// probe applies. Positive evidence on `old` or `path` (cwd, executable, argv,
/// mapping or open file of a cargo, rustc, nextest or test process) preserves
/// the cache; a process whose details are unreadable (another user's) does
/// not. Only this process's `.cargo-lock` descriptor is exempt.
fn in_use(proc_root: &Path, old: &Path, path: &Path, cargo: &fs::File) -> bool {
    #[cfg(unix)]
    let descriptors = {
        use std::os::fd::AsRawFd;
        vec![cargo.as_raw_fd()]
    };
    #[cfg(not(unix))]
    let descriptors = {
        let _ = cargo;
        Vec::new()
    };
    process_block_at(old, path, &descriptors, true, proc_root).is_some()
}

/// Lock `target/debug` for exclusive removal; `None` when it is absent,
/// symlinked, contended or in use.
fn lock_debug(worktree: &Path, proc_root: &Path) -> io::Result<Option<(PathBuf, PathBuf, fs::File)>> {
    // cas-3f93: follow a target relocated under `[factory] scratch_root`.
    let target = crate::factory_scratch::worker_target_dir(worktree);
    let debug = target.join("debug");
    if !debug.exists() {
        return Ok(None);
    }
    // Refuse symlinks at both roots; neither source nor output may escape.
    if fs::symlink_metadata(&target)?.file_type().is_symlink()
        || fs::symlink_metadata(&debug)?.file_type().is_symlink()
        || debug.canonicalize()?.parent() != Some(target.canonicalize()?.as_path())
    {
        return Ok(None);
    }
    if fs::symlink_metadata(debug.join(".cargo-lock"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Ok(None);
    }
    let cargo = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(debug.join(".cargo-lock"))?;
    if cargo.try_lock_exclusive().is_err() || in_use(proc_root, &debug, &debug, &cargo) {
        return Ok(None);
    }
    Ok(Some((target, debug, cargo)))
}

fn prune_debug(worktree: &Path, proc_root: &Path) -> io::Result<()> {
    let Some((target, debug, cargo)) = lock_debug(worktree, proc_root)? else {
        return Ok(());
    };
    let quarantine = target.join(format!(".cas-parked-debug-{}", std::process::id()));
    if quarantine.exists() {
        return Ok(());
    }
    fs::rename(&debug, &quarantine)?;
    if in_use(proc_root, &debug, &quarantine, &cargo) {
        if !debug.exists() {
            fs::rename(&quarantine, &debug)?;
        }
        return Ok(());
    }
    let result = fs::remove_dir_all(quarantine);
    FileExt::unlock(&cargo)?;
    if result.is_ok() {
        tracing::info!(worktree = %worktree.display(), "parked worker cache pruned");
    }
    result
}

/// cas-49163: drop regenerable per-build state from the retained cache and
/// keep compiled dependencies: `incremental/` and the test executables in
/// `deps/` (regular files with an execute bit and no extension). A symlinked
/// `incremental/` or `deps/` is refused; one failed removal is logged and the
/// trim continues.
fn trim_debug(worktree: &Path, proc_root: &Path) {
    let (debug, cargo) = match lock_debug(worktree, proc_root) {
        Ok(Some((_, debug, cargo))) => (debug, cargo),
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, worktree = %worktree.display(), "retained cache trim skipped");
            return;
        }
    };
    let real_dir = |path: &Path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
    let mut freed = 0u64;
    let incremental = debug.join("incremental");
    if real_dir(&incremental) {
        let bytes: u64 = WalkDir::new(&incremental)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .sum();
        let quarantine = debug.join(format!(".cas-trim-incremental-{}", std::process::id()));
        match fs::rename(&incremental, &quarantine).and_then(|()| fs::remove_dir_all(&quarantine)) {
            Ok(()) => freed += bytes,
            Err(error) => tracing::warn!(%error, path = %incremental.display(), "incremental trim failed"),
        }
    }
    #[cfg(unix)]
    if real_dir(&debug.join("deps")) {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(debug.join("deps")).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 || path.extension().is_some() {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => freed += metadata.len(),
                Err(error) => tracing::warn!(%error, path = %path.display(), "test executable trim failed"),
            }
        }
    }
    let _ = FileExt::unlock(&cargo);
    if freed > 0 {
        tracing::info!(
            worktree = %worktree.display(),
            gb_freed = format!("{:.1}", freed as f64 / f64::from(1u32 << 30)),
            "retained worker cache trimmed"
        );
    }
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

    /// Parking holds the lane and Cargo locks, so it uses the guarded probe:
    /// only unavailable process inventory (not unreadable details) preserves.
    fn guarded_reclaim_available() -> bool {
        let probe = tempfile::tempdir().unwrap();
        let path = probe.path().join("nonexistent-output");
        process_block_at(&path, &path, &[], true, Path::new("/proc")).is_none()
    }

    /// cas-49163: parking trims the retained cache to its compiled
    /// dependencies, prunes the cache beyond retention and sweeps a removed
    /// worktree's marker, despite an unreadable process and an MCP server in
    /// the checkout. A test binary running from a cache keeps all of it.
    #[cfg(target_os = "linux")]
    #[test]
    fn park_trims_and_prunes_despite_unreadable_and_checkout_processes_cas_49163() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (temp, root) = fixture();
        let [older, retained, busy] = ["older", "retained", "busy"].map(|name| worker(&root, name));
        for path in [&retained, &busy] {
            let debug = path.join("target/debug");
            fs::create_dir_all(debug.join("incremental/cas-1")).unwrap();
            fs::write(debug.join("incremental/cas-1/query.bin"), "state").unwrap();
            fs::write(debug.join("deps/libcas-1.rlib"), "dependency").unwrap();
            fs::write(debug.join("deps/cas-1.d"), "depinfo").unwrap();
            let binary = debug.join("deps/integration_factory-1");
            fs::write(&binary, "executable").unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let stale = root.join("worker-target-parks/stale.json");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, r#"{"worktree":"/nonexistent/removed","admin":"/x","head":"y","generation":"z"}"#).unwrap();
        let proc_root = temp.path().join("proc");
        let process = |pid: u32, cwd: Option<&Path>, exe: Option<&Path>| {
            let process = proc_root.join(pid.to_string());
            fs::create_dir_all(process.join("fd")).unwrap();
            fs::write(process.join("stat"), format!("{pid} (fixture) S 0 0 0 0 0 0")).unwrap();
            fs::write(process.join("cmdline"), b"fixture\0").unwrap();
            fs::write(process.join("maps"), b"").unwrap();
            for (link, path) in [("cwd", cwd), ("exe", exe)] {
                match path {
                    Some(path) => symlink(path, process.join(link)).unwrap(),
                    None => fs::create_dir(process.join(link)).unwrap(),
                }
            }
        };
        process(4_000_001, None, None);
        for (pid, path) in [(4_000_002, &older), (4_000_003, &retained)] {
            process(pid, Some(path), Some(Path::new("/usr/bin/cas")));
        }

        park_at(&root, &older, 1, None, &proc_root).unwrap();
        park_at(&root, &retained, 1, None, &proc_root).unwrap();

        assert!(!older.join("target/debug").exists(), "cache beyond retention is pruned");
        let debug = retained.join("target/debug");
        assert!(!debug.join("incremental").exists());
        assert!(!debug.join("deps/integration_factory-1").exists());
        for kept in ["deps/libcas-1.rlib", "deps/cas-1.d", "deps/output"] {
            assert!(debug.join(kept).exists(), "{kept}");
        }
        assert!(!stale.exists(), "a removed worktree's marker is swept");
        assert!(older.join("target/worker-check.log").exists());

        let binary = busy.join("target/debug/deps/integration_factory-1");
        process(4_000_004, Some(Path::new("/")), Some(&binary));
        park_at(&root, &busy, 1, None, &proc_root).unwrap();
        assert!(binary.exists() && busy.join("target/debug/incremental").exists());
        assert!(!debug.exists(), "the previously retained cache is pruned");
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
    fn cacheless_worker_does_not_create_retirement_metadata_cas_0c02() {
        let (_temp, root) = fixture();
        let worker = worker(&root, "cacheless");
        fs::remove_dir_all(worker.join("target/debug")).unwrap();

        park(&root, &worker, 0, None).unwrap();

        assert!(
            !root.join("worker-target-parks").exists(),
            "a worker without regenerable output must not write retirement metadata"
        );
        assert!(!root.join("worker-check-slots").exists());
        assert!(!root.join("target-cache-gc.lock").exists());
        assert_eq!(
            fs::read_to_string(worker.join("target/worker-check.log")).unwrap(),
            "proof"
        );
    }

    #[test]
    fn bounded_parked_targets_preserve_logs_sources_and_independent_lanes_cas_29b0() {
        let (_temp, root) = fixture();
        let reclaim = guarded_reclaim_available();
        let first = worker(&root, "first");
        let second = worker(&root, "second");
        let sources: Vec<_> = [&first, &second]
            .into_iter()
            .map(|path| (path.clone(), fs::read(path.join("source.rs")).unwrap()))
            .collect();
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
        assert_eq!(first.join("target/debug").exists(), !reclaim);
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
        }
        for (path, before) in sources {
            assert_eq!(fs::read(path.join("source.rs")).unwrap(), before);
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
        assert_eq!(count, if reclaim { 1 } else { 6 });
    }

    /// cas-3f93: parking prunes `debug/` of a target relocated under
    /// `[factory] scratch_root` and keeps its proof logs.
    #[test]
    fn scratch_root_parked_targets_prune_relocated_debug() {
        let (temp, root) = fixture();
        let reclaim = guarded_reclaim_available();
        let scratch = temp.path().canonicalize().unwrap().join("rewind");
        fs::write(
            root.join("config.toml"),
            format!("[factory]\nscratch_root = {:?}\n", scratch.display().to_string()),
        )
        .unwrap();
        let mut targets = Vec::new();
        for name in ["first", "second"] {
            let path = worker(&root, name);
            fs::remove_dir_all(path.join("target")).unwrap();
            let target = crate::factory_scratch::worker_target_dir(&path);
            assert!(target.starts_with(&scratch), "{}", target.display());
            fs::create_dir_all(target.join("debug/deps")).unwrap();
            fs::write(target.join("debug/deps/output"), "regenerable").unwrap();
            fs::write(target.join("worker-check.log"), "proof").unwrap();
            targets.push((path, target));
        }
        park(&root, &targets[0].0, 0, None).unwrap();
        assert_eq!(targets[0].1.join("debug").exists(), !reclaim);
        park(&root, &targets[1].0, 1, None).unwrap();
        assert!(targets[1].1.join("debug").exists(), "retention keeps the newest");
        for (path, target) in &targets {
            // pin: proof logs outside debug/ must survive pruning byte-for-byte.
            assert_eq!(fs::read_to_string(target.join("worker-check.log")).unwrap(), "proof");
            assert!(!path.join("target").exists());
        }
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
    fn same_head_repark_invalidates_old_retention_inventory_cas_29b0() {
        let (_temp, root) = fixture();
        let worker = worker(&root, "repark");
        park(&root, &worker, 1, None).unwrap();
        let path = park_path(&root, &worker);
        let before: ParkedCache = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(marker_is_current(&path, &before));
        park(&root, &worker, 1, None).unwrap();
        assert!(!marker_is_current(&path, &before));
        let after: ParkedCache = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(before.head, after.head);
        assert!(marker_is_current(&path, &after));
        assert!(worker.join("target/debug").exists());
    }

    #[test]
    fn same_process_output_handle_preserves_cache_but_eviction_lock_alone_prunes_cas_29b0() {
        let (_temp, root) = fixture();
        let worker = worker(&root, "own-open-output");
        let output = fs::File::open(worker.join("target/debug/deps/output")).unwrap();
        park(&root, &worker, 0, None).unwrap();
        assert!(worker.join("target/debug/deps/output").exists());
        drop(output);
        let reclaim = guarded_reclaim_available();
        // prune_debug holds its own .cargo-lock descriptor during inspection.
        // That exact descriptor alone must not keep every cache permanently warm.
        park(&root, &worker, 0, None).unwrap();
        assert_eq!(worker.join("target/debug").exists(), !reclaim);
        assert!(worker.join("target/worker-check.log").exists());
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
        let reclaim = guarded_reclaim_available();
        park(&root, &worker, 0, None).unwrap();
        assert_eq!(worker.join("target/debug").exists(), !reclaim);
    }
}
