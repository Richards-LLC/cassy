//! Narrow exception for rebuildable legacy worker targets, never integration trees.
use super::*;

#[derive(Debug, Clone)]
pub(super) struct Snapshot {
    directories: Vec<(PathBuf, (u64, u64), SystemTime)>,
    locks: Vec<PathBuf>,
    safe: bool,
}

impl Snapshot {
    pub(super) fn new() -> Self {
        Self {
            directories: Vec::new(),
            locks: Vec::new(),
            safe: true,
        }
    }
    pub(super) fn record(
        &mut self,
        cache: &Path,
        entry: &walkdir::DirEntry,
        metadata: &fs::Metadata,
    ) -> io::Result<()> {
        let relative = entry
            .path()
            .strip_prefix(cache)
            .map_err(io::Error::other)?
            .to_path_buf();
        if entry.file_type().is_symlink() {
            self.safe = false;
        }
        if entry.file_type().is_dir() {
            self.directories.push((
                relative.clone(),
                identity(entry.path())?,
                metadata.modified()?,
            ));
        }
        // Cargo's current build lock name, at every profile/triple depth.
        if entry.file_name() == ".cargo-lock" {
            self.safe &= entry.file_type().is_file();
            self.locks.push(relative);
        }
        Ok(())
    }
    pub(super) fn admissible(&self) -> bool {
        self.safe
    }
    // Cargo locks serialize artifact writes. Directory stamps detect changes
    // to the tree/lock set without stat-ing every artifact again.
    fn unchanged(&self, cache: &Path) -> bool {
        self.safe
            && self
                .directories
                .iter()
                .all(|(relative, expected, modified)| {
                    let path = cache.join(relative);
                    identity(&path).ok() == Some(*expected)
                        && fs::symlink_metadata(path)
                            .and_then(|meta| meta.modified())
                            .ok()
                            == Some(*modified)
                })
    }
}

pub(super) struct Guard {
    _lane: crate::factory_worker_check::LaneLock,
    locks: Vec<(PathBuf, fs::File)>,
    external_locks: Vec<(PathBuf, fs::File)>,
    snapshot: Snapshot,
    worktree: PathBuf,
    worktree_identity: (u64, u64),
    target_identity: (u64, u64),
    #[cfg(unix)]
    lease: Option<owner::Lease>,
}

impl Guard {
    pub(super) fn descriptors(&self) -> Vec<i32> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            self.locks
                .iter()
                .map(|(_, file)| file.as_raw_fd())
                .chain(self.external_locks.iter().map(|(_, file)| file.as_raw_fd()))
                .chain(self.lease.iter().map(owner::Lease::fd))
                .collect()
        }
        #[cfg(not(unix))]
        {
            Vec::new()
        }
    }
    pub(super) fn unchanged(&self, cache: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if identity(cache).ok() != Some(self.target_identity)
                || identity(&self.worktree).ok() != Some(self.worktree_identity)
            {
                return false;
            }
            if self
                .lease
                .as_ref()
                .is_some_and(|lease| !lease.revalidate(cache).unwrap_or(false))
            {
                return false;
            }
            if !self.snapshot.unchanged(cache) {
                return false;
            }
            if !self.external_locks.iter().all(|(path, file)| {
                let (Ok(held), Ok(current)) = (file.metadata(), fs::symlink_metadata(path)) else { return false };
                current.is_file() && !current.file_type().is_symlink()
                    && held.dev() == current.dev() && held.ino() == current.ino()
            }) { return false; }
            self.locks.iter().all(|(relative, file)| {
                let (Ok(held), Ok(current)) =
                    (file.metadata(), fs::symlink_metadata(cache.join(relative)))
                else {
                    return false;
                };
                current.is_file()
                    && !current.file_type().is_symlink()
                    && held.ino() == current.ino()
                    && held.dev() == current.dev()
            })
        }
        #[cfg(not(unix))]
        {
            let _ = cache;
            false
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        for (_, lock) in self.locks.iter().chain(&self.external_locks) {
            let _ = FileExt::unlock(lock);
        }
    }
}

fn identity(path: &Path) -> io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other("target/worktree identity changed"));
        }
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::other("unavailable filesystem identity"))
    }
}

pub(crate) fn fingerprint_alive(agent: &cas_types::Agent) -> bool {
    crate::mcp::tools::service::agent_liveness::agent_process_is_alive_with(
        agent,
        |_| false,
        crate::mcp::daemon::pid_matches_fingerprint,
    )
}

pub(crate) fn registered_live(agent: &cas_types::Agent) -> bool {
    use crate::mcp::tools::service::agent_liveness::{WORKER_STALE_SECS, agent_heartbeat_age_secs};
    matches!(
        agent.status,
        cas_types::AgentStatus::Active | cas_types::AgentStatus::Idle
    ) && agent_heartbeat_age_secs(agent) < WORKER_STALE_SECS
        && fingerprint_alive(agent)
}

fn registry(cas_root: &Path, worktree: &Path) -> io::Result<bool> {
    let agents = crate::store::open_agent_store(cas_root)
        .map_err(io::Error::other)?
        .list(None)
        .map_err(io::Error::other)?;
    // Registry authority is clone_path only; readable process evidence separately protects other owners.
    let owners: Vec<_> = agents
        .iter()
        .filter(|agent| {
            agent
                .metadata
                .get("clone_path")
                .and_then(|path| Path::new(path).canonicalize().ok())
                .is_some_and(|path| path == worktree)
        })
        .collect();
    if owners.iter().any(|agent| registered_live(agent)) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "fresh fingerprinted registered owner",
        ));
    }
    // With a surviving owner, every associated task must be terminal. A gone
    // owner needs no task receipt; stale registry rows are not live authority.
    let surviving: Vec<_> = owners
        .iter()
        .filter(|agent| {
            crate::mcp::tools::service::agent_liveness::agent_process_is_alive_with(
                agent,
                crate::mcp::daemon::pid_alive,
                crate::mcp::daemon::pid_matches_fingerprint,
            )
        })
        .collect();
    if surviving.is_empty() {
        return Ok(true);
    }
    let tasks = crate::store::open_task_store(cas_root)
        .map_err(io::Error::other)?
        .list(None)
        .map_err(io::Error::other)?;
    Ok(surviving.iter().all(|owner| {
        let owned: Vec<_> = tasks
            .iter()
            .filter(|task| {
                task.assignee
                    .as_deref()
                    .is_some_and(|name| name == owner.id || name == owner.name)
            })
            .collect();
        !owned.is_empty() && owned.iter().all(|task| task.is_terminal())
    }))
}

/// Warm-cache pressure exemptions require positive ownership and terminal
/// work, unlike ordinary idle eligibility which also permits unknown/gone owners.
pub(super) fn terminal_dead_owner(cas_root: &Path, worktree: &Path) -> bool {
    let evidence = || -> io::Result<bool> {
        let agents = crate::store::open_agent_store(cas_root)
            .map_err(io::Error::other)?
            .list(None)
            .map_err(io::Error::other)?;
        let owners: Vec<_> = agents
            .iter()
            .filter(|agent| {
                agent
                    .metadata
                    .get("clone_path")
                    .and_then(|path| Path::new(path).canonicalize().ok())
                    .is_some_and(|path| path == worktree)
            })
            .collect();
        if owners.is_empty()
            || owners.iter().any(|owner| {
                owner.pid.is_none()
                    || registered_live(owner)
                    || crate::mcp::tools::service::agent_liveness::agent_process_is_alive_with(
                        owner,
                        crate::mcp::daemon::pid_alive,
                        crate::mcp::daemon::pid_matches_fingerprint,
                    )
            })
        {
            return Ok(false);
        }
        let tasks = crate::store::open_task_store(cas_root)
            .map_err(io::Error::other)?
            .list(None)
            .map_err(io::Error::other)?;
        Ok(owners.iter().all(|owner| {
            let mut owned = tasks
                .iter()
                .filter(|task| {
                    task.assignee
                        .as_deref()
                        .is_some_and(|name| name == owner.id || name == owner.name)
                })
                .peekable();
            owned.peek().is_some() && owned.all(|task| task.is_terminal())
        }))
    };
    evidence().unwrap_or(false)
}

pub(super) fn owner_gate(
    cas_root: &Path,
    worktree: &Path,
) -> Result<(), (CacheDisposition, String)> {
    match registry(cas_root, worktree) {
        Ok(true) => Ok(()),
        Ok(false) => Err((
            CacheDisposition::Indeterminate,
            "surviving owner has nonterminal or unknown tasks".into(),
        )),
        Err(error) => Err((
            if error.kind() == io::ErrorKind::WouldBlock {
                CacheDisposition::LiveProcess
            } else {
                CacheDisposition::Indeterminate
            },
            error.to_string(),
        )),
    }
}

pub(crate) fn lane_target(cas_root: &Path, worktree: &Path, cache: &Path) -> bool {
    let workers = cas_root.join("worktrees");
    // Equality to canonical paths rejects symlinked intermediate components too.
    workers.canonicalize().ok().as_ref() == Some(&workers)
        && worktree.canonicalize().ok().as_deref() == Some(worktree)
        && worktree.parent() == Some(workers.as_path())
        && (cache == worktree.join("target") || relocated_target(worktree, cache))
        && cache.canonicalize().ok().as_deref() == Some(cache)
        && !lane::is_preview_path(cas_root, worktree)
}

pub(super) fn acquire(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    snapshot: &Snapshot,
) -> io::Result<Guard> {
    acquire_scanned(cas_root, worktree, cache, snapshot, registry, false)
}

#[cfg(test)]
fn acquire_with(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    registry: impl Fn(&Path, &Path) -> io::Result<bool>,
) -> io::Result<Guard> {
    let snapshot = scan_cache(worktree, cache, false)?
        .snapshot
        .ok_or_else(|| io::Error::other("unsafe cache snapshot"))?;
    acquire_scanned(cas_root, worktree, cache, &snapshot, registry, false)
}

/// Same build/Cargo/lease guard, after the caller independently validates
/// exact operator consent and unknown-cache containment. No adoption occurs.
pub(super) fn acquire_explicit(
    cas_root: &Path, worktree: &Path, cache: &Path, snapshot: &Snapshot,
) -> io::Result<Guard> {
    acquire_scanned(cas_root, worktree, cache, snapshot, registry, true)
}

fn acquire_scanned(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    snapshot: &Snapshot,
    registry: impl Fn(&Path, &Path) -> io::Result<bool>,
    explicit: bool,
) -> io::Result<Guard> {
    if (!explicit && !lane_target(cas_root, worktree, cache)) || !snapshot.safe || !registry(cas_root, worktree)? {
        return Err(io::Error::other(
            "legacy lane containment or owner gate refused",
        ));
    }
    let lane = crate::factory_worker_check::try_lock_lane(cas_root, worktree)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "worker build owns lane lock"))?;
    #[cfg(unix)]
    let lease = if cache.join(owner::MARKER).exists() {
        Some(
            owner::for_gc(cas_root, worktree, cache)?
                .ok_or_else(|| io::Error::other("missing target lease"))?,
        )
    } else {
        None
    };
    let mut guard = Guard {
        _lane: lane,
        locks: Vec::new(),
        external_locks: Vec::new(),
        snapshot: snapshot.clone(),
        worktree: worktree.to_path_buf(),
        worktree_identity: identity(worktree)?,
        target_identity: identity(cache)?,
        #[cfg(unix)]
        lease,
    };
    if explicit && cache.file_name().is_some_and(|name| name == "assembly-target") {
        let path = cache.with_file_name("assembly-target.lock");
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)] {
            use std::os::unix::fs::{OpenOptionsExt, MetadataExt};
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            if fs::symlink_metadata(&path).is_ok_and(|md| !md.is_file() || md.uid() != unsafe { libc::geteuid() }) {
                return Err(io::Error::other("unsafe assembly target lease"));
            }
        }
        let file = options.open(&path)?;
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            if !file.metadata()?.is_file() || file.metadata()?.uid() != unsafe { libc::geteuid() } {
                return Err(io::Error::other("assembly target lease belongs to another user"));
            }
        }
        file.try_lock_exclusive().map_err(|error| io::Error::new(error.kind(), format!("assembly target lease {} unavailable: {error}", path.display())))?;
        guard.external_locks.push((path, file));
    }
    for relative in &snapshot.locks {
        let path = cache.join(relative);
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Cargo lock {} unavailable: {error}", path.display()),
            )
        })?;
        file.try_lock_exclusive().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Cargo lock {} unavailable: {error}", path.display()),
            )
        })?;
        guard.locks.push((relative.clone(), file));
    }
    if !guard.unchanged(cache) {
        return Err(io::Error::other(
            "directory or Cargo lock set changed under lock",
        ));
    }
    Ok(guard)
}

pub(super) fn registry_allows(cas_root: &Path, worktree: &Path) -> bool {
    registry(cas_root, worktree).unwrap_or(false)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join(".cas");
        let worker = root.join("worktrees/retired-worker");
        let target = worker.join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/.cargo-lock"), "").unwrap();
        fs::write(target.join("debug/output"), "rebuildable").unwrap();
        (temp, root, worker, target)
    }

    fn opaque_proc(root: &Path) -> PathBuf {
        let proc = root.join("proc-fixture");
        let process = proc.join("1");
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::write(process.join("stat"), "1 (opaque) S 0 0 0 0 0 0").unwrap();
        symlink("/unrelated", process.join("cwd")).unwrap();
        symlink("/unrelated/bin", process.join("exe")).unwrap();
        fs::write(process.join("cmdline"), "unrelated").unwrap();
        proc
    }

    #[test]
    fn dead_registered_owner_with_opaque_process_is_reclaimable_cas_cd3e() {
        let (_temp, root, worker, target) = fixture();
        let proc = opaque_proc(&root);
        let mut owner = cas_types::Agent::new("dead-owner".into(), "retired-worker".into());
        owner.pid = Some(i32::MAX as u32);
        owner.pid_starttime = Some(1);
        owner.last_heartbeat = chrono::Utc::now() - chrono::Duration::hours(3);
        owner
            .metadata
            .insert("clone_path".into(), worker.display().to_string());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&owner)
            .unwrap();
        let guard = linux_cache_admission(&root, &worker, &target, &proc)
            .unwrap()
            .unwrap();
        assert!(guard.unchanged(&target));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.join("debug/.cargo-lock"))
            .unwrap();
        assert!(
            lock.try_lock_exclusive().is_err(),
            "Cargo lock must remain held through rename"
        );
        let quarantine = worker.join(".cas-target-gc-fixture");
        fs::rename(&target, &quarantine).unwrap();
        assert!(guard.unchanged(&quarantine));
        assert!(lock.try_lock_exclusive().is_err());
        fs::remove_dir_all(&quarantine).unwrap();
        assert!(!quarantine.exists());
        drop(guard);
        assert!(lock.try_lock_exclusive().is_ok());
    }

    #[test]
    fn positive_cwd_and_non_lane_opacity_remain_protected_cas_cd3e() {
        let (_temp, root, worker, target) = fixture();
        let proc = opaque_proc(&root);
        fs::remove_file(proc.join("1/cwd")).unwrap();
        symlink(&worker, proc.join("1/cwd")).unwrap();
        assert_eq!(
            linux_cache_admission(&root, &worker, &target, &proc)
                .err()
                .unwrap()
                .0,
            CacheDisposition::LiveProcess
        );
        fs::remove_file(proc.join("1/cwd")).unwrap();
        symlink("/unrelated", proc.join("1/cwd")).unwrap();
        let integration = root.join("epic-integration-test");
        fs::create_dir_all(integration.join("target")).unwrap();
        assert_eq!(
            linux_cache_admission(&root, &integration, &integration.join("target"), &proc)
                .err()
                .unwrap()
                .0,
            CacheDisposition::Indeterminate
        );
        assert_eq!(
            linux_cache_admission(&root, &worker, &target, &proc.join("absent"))
                .err()
                .unwrap()
                .0,
            CacheDisposition::Indeterminate
        );
    }

    #[test]
    fn every_cargo_lock_lane_lock_and_symlink_are_gates_cas_cd3e() {
        let (_temp, root, worker, target) = fixture();
        fs::create_dir_all(target.join("a/b/c/d")).unwrap();
        let deep = target.join("a/b/c/d/.cargo-lock");
        fs::write(&deep, "").unwrap();
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&deep)
            .unwrap();
        held.try_lock_exclusive().unwrap();
        assert!(acquire_with(&root, &worker, &target, |_, _| Ok(true)).is_err());
        FileExt::unlock(&held).unwrap();
        let lane = crate::factory_worker_check::try_lock_lane(&root, &worker)
            .unwrap()
            .unwrap();
        assert!(acquire_with(&root, &worker, &target, |_, _| Ok(true)).is_err());
        drop(lane);
        assert!(acquire_with(&root, &worker, &target, |_, _| Ok(false)).is_err());
        assert!(
            acquire_with(&root, &worker, &target, |_, _| Err(io::Error::other(
                "unreadable registry"
            )))
            .is_err()
        );
        let guard = acquire_with(&root, &worker, &target, |_, _| Ok(true)).unwrap();
        fs::create_dir(target.join("new")).unwrap();
        fs::write(target.join("new/.cargo-lock"), "").unwrap();
        assert!(
            !guard.unchanged(&target),
            "new Cargo locks must invalidate rename admission"
        );
        drop(guard);
        let guard = acquire_with(&root, &worker, &target, |_, _| Ok(true)).unwrap();
        fs::rename(&deep, deep.with_file_name("old-lock")).unwrap();
        fs::write(&deep, "").unwrap();
        assert!(
            !guard.unchanged(&target),
            "replaced lock inode must invalidate admission"
        );
        drop(guard);
        let guard = acquire_with(&root, &worker, &target, |_, _| Ok(true)).unwrap();
        fs::rename(&target, worker.join("old-target")).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(
            !guard.unchanged(&target),
            "replaced target inode must invalidate admission"
        );
        drop(guard);
        symlink("/tmp", target.join("outside")).unwrap();
        assert!(acquire_with(&root, &worker, &target, |_, _| Ok(true)).is_err());
        fs::remove_file(target.join("outside")).unwrap();
        let alias = worker.with_file_name("alias");
        symlink(&worker, &alias).unwrap();
        assert!(!lane_target(&root, &alias, &alias.join("target")));
    }

    #[test]
    fn gc_registry_requires_fresh_heartbeat_and_matching_fingerprint_cas_cd3e() {
        let mut owner = cas_types::Agent::new("owner".into(), "owner".into());
        owner.pid = Some(std::process::id());
        owner.pid_starttime = crate::mcp::daemon::read_pid_starttime(std::process::id());
        assert!(registered_live(&owner));
        owner.last_heartbeat = chrono::Utc::now() - chrono::Duration::hours(3);
        assert!(!registered_live(&owner));
        owner.last_heartbeat = chrono::Utc::now();
        owner.pid_starttime = owner.pid_starttime.map(|time| time + 1);
        assert!(
            !registered_live(&owner),
            "reused PID cannot vouch for owner"
        );
        owner.pid_starttime = None;
        assert!(!registered_live(&owner), "bare PID cannot vouch for owner");
    }

    #[test]
    fn opaque_cargo_lock_holder_is_indeterminate_with_path_cas_cd3e_review() {
        let (_temp, root, worker, target) = fixture();
        let proc = opaque_proc(&root);
        let lock_path = target.join("debug/.cargo-lock");
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.try_lock_exclusive().unwrap();
        let (disposition, reason) = linux_cache_admission(&root, &worker, &target, &proc)
            .err()
            .unwrap();
        assert_eq!(disposition, CacheDisposition::Indeterminate);
        assert!(
            reason.contains(&lock_path.display().to_string()),
            "{reason}"
        );
    }

    #[test]
    fn preview_never_acquires_or_creates_build_locks_cas_cd3e_review() {
        let (_temp, root, _worker, _target) = fixture();
        let policy = TargetCachePolicy {
            high_watermark_percent: 1,
            low_watermark_percent: 0,
            min_idle_secs: 0,
            retention_count: 0,
        };
        let report = inspect(&root, policy, &[], &[], true).unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::Selected);
        assert!(
            !root.join("worker-check-slots").exists(),
            "preview must not create lane locks"
        );
    }

    #[test]
    fn locked_recheck_uses_directory_stamps_without_listing_artifacts_cas_cd3e_review() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, root, worker, target) = fixture();
        let scanned = scan_cache(&worker, &target, false).unwrap();
        let directory = target.join("debug");
        let previous = fs::metadata(&directory).unwrap().permissions();
        // Stat/open of known paths remains possible, but readdir is forbidden.
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o111)).unwrap();
        let guard = acquire_scanned(
            &root,
            &worker,
            &target,
            scanned.snapshot.as_ref().unwrap(),
            |_, _| Ok(true),
            false,
        );
        let unchanged = guard.as_ref().is_ok_and(|guard| guard.unchanged(&target));
        fs::set_permissions(directory, previous).unwrap();
        assert!(unchanged, "recheck must not enumerate all artifact files");
    }

    #[test]
    fn owned_target_keeps_lifetime_lease_and_provenance_gate_cas_cd3e() {
        let (_temp, root, worker, target) = fixture();
        fs::remove_dir_all(&target).unwrap();
        let lease = owner::acquire(&root, &worker).unwrap().unwrap();
        assert!(acquire_with(&root, &worker, &target, |_, _| Ok(true)).is_err());
        drop(lease);
        let guard = acquire_with(&root, &worker, &target, |_, _| Ok(true)).unwrap();
        assert!(guard.unchanged(&target));
        fs::write(target.join(owner::MARKER), "changed provenance").unwrap();
        assert!(!guard.unchanged(&target));
    }

    #[test]
    fn surviving_owner_needs_terminal_tasks_and_fresh_owner_blocks_cas_cd3e() {
        let (_temp, root, worker, target) = fixture();
        let proc = opaque_proc(&root);
        let agents = crate::store::open_agent_store(&root).unwrap();
        let mut owner = cas_types::Agent::new("owner".into(), "retired-worker".into());
        owner.pid = Some(std::process::id());
        owner.pid_starttime = crate::mcp::daemon::read_pid_starttime(std::process::id());
        owner
            .metadata
            .insert("clone_path".into(), worker.display().to_string());
        agents.register(&owner).unwrap();
        assert_eq!(
            linux_cache_admission(&root, &worker, &target, &proc)
                .err()
                .unwrap()
                .0,
            CacheDisposition::LiveProcess
        );
        owner.last_heartbeat = chrono::Utc::now() - chrono::Duration::hours(3);
        agents.update(&owner).unwrap();
        assert!(!registry_allows(&root, &worker));
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut task = cas_types::Task::new("cas-fixture".into(), "owner task".into());
        task.assignee = Some(owner.name.clone());
        tasks.add(&task).unwrap();
        assert!(!registry_allows(&root, &worker));
        task.status = cas_types::TaskStatus::Closed;
        tasks.update(&task).unwrap();
        assert!(registry_allows(&root, &worker));
        assert!(linux_cache_admission(&root, &worker, &target, &proc).is_ok());
    }

    #[test]
    fn cleanup_rechecks_recency_locks_registry_and_real_cwd_cas_cd3e() {
        let (_temp, root, worker, target) = fixture();
        let policy = TargetCachePolicy {
            high_watermark_percent: 1,
            low_watermark_percent: 0,
            min_idle_secs: 0,
            retention_count: 0,
        };
        let mut report = inspect(&root, policy, &[], &[], false).unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::Selected);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.join("debug/.cargo-lock"))
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        cleanup_selected(&root, &mut report, policy, &[]).unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::LiveProcess);
        assert!(target.exists());
        FileExt::unlock(&lock).unwrap();
        drop(lock);
        let mut report = inspect(&root, policy, &[], &[], false).unwrap();
        cleanup_selected(
            &root,
            &mut report,
            TargetCachePolicy {
                min_idle_secs: 7200,
                ..policy
            },
            &[],
        )
        .unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::RecentWrite);
        let mut report = inspect(&root, policy, &[], &[], false).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .current_dir(&worker)
            .spawn()
            .unwrap();
        cleanup_selected(&root, &mut report, policy, &[]).unwrap();
        let result = report.caches[0].disposition;
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(result, CacheDisposition::LiveProcess);
        assert!(target.exists());
        let mut report = inspect(&root, policy, &[], &[], false).unwrap();
        cleanup_selected(&root, &mut report, policy, &[]).unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::Reclaimed);
        assert!(!target.exists());
        assert!(report.reclaimed_bytes > 0);
    }
}
