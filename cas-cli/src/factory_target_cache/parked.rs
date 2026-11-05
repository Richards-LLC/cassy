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
    trim_debug(&current.worktree)?;
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
        prune_debug(&parked.worktree, None)?;
        drop(lane);
    }
    let budget = crate::config::Config::load(cas_root)
        .map(|config| config.factory().target_cache_budget_gb)
        .unwrap_or(0)
        .saturating_mul(1 << 30);
    if budget > 0 {
        enforce_budget(cas_root, &current.worktree, budget, BUDGET_MIN_IDLE)?;
    }
    FileExt::unlock(&gc)?;
    Ok(())
}

/// A worker cache built within this window is never evicted for the budget.
const BUDGET_MIN_IDLE: Duration = Duration::from_secs(30 * 60);

/// cas-49163: prune the least recently built idle worker caches until all
/// worker `target/debug` trees fit `budget` bytes. Busy workers that never
/// park otherwise keep 15-50 GB each until the disk watermark refuses lanes.
/// Called under the GC lock; `keep` is the cache that was just parked.
fn enforce_budget(cas_root: &Path, keep: &Path, budget: u64, min_idle: Duration) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(cas_root.join("worktrees")) else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    let mut caches: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter_map(|worktree| {
            identity(cas_root, &worktree)?;
            let debug = crate::factory_scratch::worker_target_dir(&worktree).join("debug");
            let built = ["deps", ".fingerprint"]
                .iter()
                .filter_map(|name| debug.join(name).metadata().ok()?.modified().ok())
                .max()?;
            Some((built, disk_bytes(&debug, &mut seen), worktree))
        })
        .collect();
    let mut total: u64 = caches.iter().map(|(_, bytes, _)| bytes).sum();
    caches.sort();
    let keep = keep.canonicalize().ok();
    let now = SystemTime::now();
    for (built, bytes, worktree) in caches {
        if total <= budget {
            break;
        }
        if worktree.canonicalize().ok() == keep
            || now.duration_since(built).unwrap_or_default() < min_idle
        {
            continue;
        }
        let Some(lane) = crate::factory_worker_check::try_lock_lane(cas_root, &worktree)? else {
            continue;
        };
        // Any process working in the checkout (cargo, rustc, nextest or a test
        // binary) preserves its cache, not only one holding output open.
        if prune_debug(&worktree, Some(&worktree))? {
            total = total.saturating_sub(bytes);
            tracing::info!(worktree = %worktree.display(), bytes, "worker cache pruned for target_cache_budget_gb");
        }
        drop(lane);
    }
    Ok(())
}

/// Allocated bytes under `path`, counting each hard-linked inode once across
/// every call sharing `seen` (seeded dependencies are shared between workers).
fn disk_bytes(path: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .filter(|metadata| metadata.nlink() < 2 || seen.insert((metadata.dev(), metadata.ino())))
            .map(|metadata| metadata.blocks().saturating_mul(512))
            .sum()
    }
    #[cfg(not(unix))]
    {
        let _ = seen;
        WalkDir::new(path)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .sum()
    }
}

fn marker_is_current(path: &Path, recorded: &ParkedCache) -> bool {
    !recorded.generation.is_empty()
        && fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ParkedCache>(&bytes).ok())
            .is_some_and(|current| current == *recorded)
}

/// Lock and inspect `target/debug` for exclusive removal. `None` means the
/// cache is unsafe, contended or in use (see [`in_use`]).
fn lock_debug(worktree: &Path, checkout: Option<&Path>) -> io::Result<Option<(PathBuf, PathBuf, fs::File)>> {
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
    if cargo.try_lock_exclusive().is_err() {
        return Ok(None);
    }
    if in_use(checkout, &debug, &cargo) {
        return Ok(None);
    }
    Ok(Some((target, debug, cargo)))
}

/// Ignore only this process's own lock handle. An unrelated open artifact
/// (including a still-running test binary) preserves the complete cache; with
/// `checkout`, so does any process whose cwd or executable is in the checkout.
fn in_use(checkout: Option<&Path>, debug: &Path, cargo: &fs::File) -> bool {
    match checkout {
        None => output_in_use(debug, cargo),
        #[cfg(unix)]
        Some(checkout) => {
            use std::os::fd::AsRawFd;
            process_uses(checkout, debug, Some(cargo.as_raw_fd()))
        }
        #[cfg(not(unix))]
        Some(_) => true,
    }
}

/// Remove the whole `target/debug`; true when it was removed.
fn prune_debug(worktree: &Path, checkout: Option<&Path>) -> io::Result<bool> {
    let Some((target, debug, cargo)) = lock_debug(worktree, checkout)? else {
        return Ok(false);
    };
    let quarantine = target.join(format!(".cas-parked-debug-{}", std::process::id()));
    if quarantine.exists() {
        return Ok(false);
    }
    fs::rename(&debug, &quarantine)?;
    if in_use(checkout, &quarantine, &cargo) || output_in_use(&debug, &cargo) {
        if !debug.exists() {
            fs::rename(&quarantine, &debug)?;
        }
        return Ok(false);
    }
    let result = fs::remove_dir_all(quarantine);
    FileExt::unlock(&cargo)?;
    result.map(|()| true)
}

/// cas-49163: drop regenerable per-build state from a retained cache while
/// keeping compiled dependencies: `incremental/` and test executables in
/// `deps/` (regular files with an execute bit and no extension).
fn trim_debug(worktree: &Path) -> io::Result<()> {
    let Some((_, debug, cargo)) = lock_debug(worktree, None)? else {
        return Ok(());
    };
    let incremental = debug.join("incremental");
    if fs::symlink_metadata(&incremental).is_ok_and(|metadata| metadata.is_dir()) {
        let quarantine = debug.join(format!(".cas-trim-incremental-{}", std::process::id()));
        if !quarantine.exists() {
            fs::rename(&incremental, &quarantine)?;
            fs::remove_dir_all(&quarantine)?;
        }
    }
    #[cfg(unix)]
    if let Ok(entries) = fs::read_dir(debug.join("deps")) {
        use std::os::unix::fs::PermissionsExt;
        for entry in entries.flatten() {
            let path = entry.path();
            let executable = fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
            if executable && path.extension().is_none() {
                fs::remove_file(&path)?;
            }
        }
    }
    FileExt::unlock(&cargo)?;
    Ok(())
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

    /// cas-49163: parking trims the retained cache to its compiled
    /// dependencies and drops a removed worktree's marker.
    #[test]
    fn park_trims_retained_cache_and_stale_markers_cas_49163() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, root) = fixture();
        let reclaim = super::super::tests::reclamation_available();
        let worker = worker(&root, "trimmed");
        let debug = worker.join("target/debug");
        fs::create_dir_all(debug.join("incremental/cas-1")).unwrap();
        fs::write(debug.join("incremental/cas-1/query.bin"), "state").unwrap();
        fs::write(debug.join("deps/libcas-1.rlib"), "dependency").unwrap();
        fs::write(debug.join("deps/cas-1.d"), "depinfo").unwrap();
        let test_binary = debug.join("deps/integration_factory-1");
        fs::write(&test_binary, "executable").unwrap();
        fs::set_permissions(&test_binary, fs::Permissions::from_mode(0o755)).unwrap();
        let stale = root.join("worker-target-parks/stale.json");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(
            &stale,
            r#"{"worktree":"/nonexistent/removed","admin":"/x","head":"y","generation":"z"}"#,
        )
        .unwrap();

        park(&root, &worker, 1, None).unwrap();

        assert_eq!(debug.join("incremental").exists(), !reclaim);
        assert_eq!(test_binary.exists(), !reclaim);
        assert!(debug.join("deps/libcas-1.rlib").exists());
        assert!(debug.join("deps/cas-1.d").exists());
        assert!(debug.join("deps/output").exists());
        assert!(!stale.exists(), "a removed worktree's marker is swept");
        assert!(park_path(&root, &worker).exists());
    }

    /// cas-49163: over budget, the least recently built idle cache goes first;
    /// the just-parked cache, a held lane and a recent build are kept.
    #[test]
    fn budget_prunes_least_recently_built_idle_cache_cas_49163() {
        let (_temp, root) = fixture();
        let reclaim = super::super::tests::reclamation_available();
        let [oldest, newer, held, parked] =
            ["oldest", "newer", "held", "parked"].map(|name| worker(&root, name));
        let hour = |hours: u64| {
            filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(hours * 3600))
        };
        for (path, hours) in [(&held, 9), (&oldest, 8), (&newer, 7), (&parked, 9)] {
            filetime::set_file_mtime(path.join("target/debug/deps"), hour(hours)).unwrap();
        }
        let total: u64 = [&oldest, &newer, &held, &parked]
            .map(|path| disk_bytes(&path.join("target/debug"), &mut HashSet::new()))
            .iter()
            .sum();
        assert!(total > 0);
        let _lane = crate::factory_worker_check::try_lock_lane(&root, &held)
            .unwrap()
            .unwrap();

        // One cache over budget: the held lane and the kept cache are skipped.
        enforce_budget(&root, &parked, total - 1, Duration::ZERO).unwrap();
        assert_eq!(oldest.join("target/debug").exists(), !reclaim);
        for path in [&newer, &held, &parked] {
            assert!(path.join("target/debug").exists(), "{}", path.display());
        }
        // A recent build is never evicted for the budget.
        enforce_budget(&root, &parked, 0, Duration::from_secs(365 * 24 * 3600)).unwrap();
        assert!(newer.join("target/debug").exists());
        assert!(oldest.join("target/worker-check.log").exists());
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
        let reclaim = super::super::tests::reclamation_available();
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
        let reclaim = super::super::tests::reclamation_available();
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
        let reclaim = super::super::tests::reclamation_available();
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
        let reclaim = super::super::tests::reclamation_available();
        park(&root, &worker, 0, None).unwrap();
        assert_eq!(worker.join("target/debug").exists(), !reclaim);
    }
}
