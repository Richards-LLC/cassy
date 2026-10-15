//! Narrow exception for rebuildable legacy worker targets, never integration trees.
use super::*;

pub(super) struct Guard {
    _lane: crate::factory_worker_check::LaneLock,
    locks: Vec<(PathBuf, fs::File)>,
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
            let mut count = 0;
            for entry in WalkDir::new(cache).follow_links(false) {
                let Ok(entry) = entry else { return false };
                if entry.file_type().is_symlink() {
                    return false;
                }
                if entry.file_name() != ".cargo-lock" {
                    continue;
                }
                count += 1;
                let Some((_, file)) = self
                    .locks
                    .iter()
                    .find(|(relative, _)| cache.join(relative) == entry.path())
                else {
                    return false;
                };
                let (Ok(held), Ok(current)) = (file.metadata(), fs::symlink_metadata(entry.path()))
                else {
                    return false;
                };
                if !current.is_file() || held.ino() != current.ino() || held.dev() != current.dev()
                {
                    return false;
                }
            }
            count == self.locks.len()
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
        for (_, lock) in &self.locks {
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

pub(super) fn lane_target(cas_root: &Path, worktree: &Path, cache: &Path) -> bool {
    let workers = cas_root.join("worktrees");
    // Equality to canonical paths rejects symlinked intermediate components too.
    workers.canonicalize().ok().as_ref() == Some(&workers)
        && worktree.canonicalize().ok().as_deref() == Some(worktree)
        && worktree.parent() == Some(workers.as_path())
        && cache == worktree.join("target")
        && cache.canonicalize().ok().as_deref() == Some(cache)
        && !lane::is_preview_path(cas_root, worktree)
}

pub(super) fn acquire(cas_root: &Path, worktree: &Path, cache: &Path) -> io::Result<Guard> {
    acquire_with(cas_root, worktree, cache, registry)
}

fn acquire_with(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    registry: impl Fn(&Path, &Path) -> io::Result<bool>,
) -> io::Result<Guard> {
    if !lane_target(cas_root, worktree, cache) || !registry(cas_root, worktree)? {
        return Err(io::Error::other(
            "legacy lane containment or owner gate refused",
        ));
    }
    let lane = crate::factory_worker_check::try_lock_lane(cas_root, worktree)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "worker build owns lane lock"))?;
    #[cfg(unix)]
    let lease = if cache.join(owner::MARKER).exists() {
        Some(
            owner::for_retirement(cas_root, worktree)?
                .ok_or_else(|| io::Error::other("missing target lease"))?,
        )
    } else {
        None
    };
    let mut guard = Guard {
        _lane: lane,
        locks: Vec::new(),
        worktree: worktree.to_path_buf(),
        worktree_identity: identity(worktree)?,
        target_identity: identity(cache)?,
        #[cfg(unix)]
        lease,
    };
    for entry in WalkDir::new(cache).follow_links(false) {
        let entry = entry.map_err(io::Error::other)?;
        // No symlink within the target may hide Cargo locks or cross containment.
        if entry.file_type().is_symlink() {
            return Err(io::Error::other("legacy target contains a symlink"));
        }
        if entry.file_name() != ".cargo-lock" {
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(io::Error::other("invalid Cargo lock"));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(entry.path())?;
        file.try_lock_exclusive()?;
        guard.locks.push((
            entry
                .path()
                .strip_prefix(cache)
                .map_err(io::Error::other)?
                .to_path_buf(),
            file,
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
