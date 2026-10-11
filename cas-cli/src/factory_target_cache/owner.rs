//! Provenance for newly created private targets. Legacy output is never adopted.
//! The external lease survives target quarantine and is inherited by builders.
use super::*;
use sha2::{Digest, Sha256};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

pub(crate) const MARKER: &str = ".cas-worker-target-owner";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Record {
    version: u32,
    worktree: PathBuf,
    worktree_dev: u64,
    worktree_ino: u64,
    target_dev: u64,
    target_ino: u64,
    lease_dev: u64,
    lease_ino: u64,
    pid: u32,
    start: u64,
    boot: String,
    active: bool,
    generation: String,
}

pub(crate) struct Lease {
    file: fs::File,
    record_path: PathBuf,
    lock_path: PathBuf,
    record: Record,
    builder: bool,
}

fn owned_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.file_type().is_symlink()
        || (metadata.is_file() && metadata.nlink() != 1)
    {
        return Err(io::Error::other("unverifiable target ownership path"));
    }
    Ok(metadata)
}

fn private_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = owned_metadata(path)?;
    if metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other("unverifiable target ownership path"));
    }
    Ok(metadata)
}

fn worktree_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = owned_metadata(path)?;
    // Git checkouts follow the host umask and may be group writable. Only
    // CAS-created target and provenance paths require private permissions.
    if !metadata.is_dir() || metadata.mode() & 0o002 != 0 {
        return Err(io::Error::other("unverifiable worker checkout"));
    }
    Ok(metadata)
}

fn target_is_ignored(worktree: &Path) -> io::Result<bool> {
    // Ownership-only fixtures have no Git checkout. Production checkouts must
    // ignore output already; never edit the operator's shared Git exclusions.
    if !worktree.join(".git").exists() {
        return Ok(true);
    }
    for path in ["target/".to_string(), format!("target/{MARKER}")] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["check-ignore", "-q", "--"])
            .arg(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .status()?;
        if status.code() == Some(1) {
            return Ok(false);
        }
        if !status.success() {
            return Err(io::Error::other("cannot verify worker target ignore policy"));
        }
    }
    Ok(true)
}

fn boot() -> io::Result<String> {
    #[cfg(target_os = "linux")]
    {
        Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(String::new())
    } // macOS start identity is an absolute timestamp.
}

fn owner_live(record: &Record) -> io::Result<bool> {
    if !record.active || record.boot != boot()? {
        return Ok(false);
    }
    if let Some(start) = crate::mcp::daemon::read_pid_starttime(record.pid) {
        #[cfg(target_os = "linux")]
        if fs::read_to_string(format!("/proc/{}/stat", record.pid))?
            .rsplit_once(')')
            .and_then(|(_, fields)| fields.split_whitespace().next())
            .is_some_and(|state| matches!(state, "Z" | "X"))
        {
            return Ok(false);
        }
        return Ok(start == record.start);
    }
    #[cfg(target_os = "linux")]
    {
        match fs::symlink_metadata(format!("/proc/{}", record.pid)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            _ => Err(io::Error::other(
                "target owner process identity unavailable",
            )),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: signal zero only probes existence; unknown identities retain output.
        if unsafe { libc::kill(record.pid as i32, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            Ok(false)
        } else {
            Err(io::Error::other(
                "target owner process identity unavailable",
            ))
        }
    }
}

fn write_record(path: &Path, record: &Record) -> io::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("partial-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(record).map_err(io::Error::other)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("missing ownership parent"))?,
        )?
        .sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn paths(cas_root: &Path, worktree: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let cas_root = cas_root.canonicalize()?;
    let worktree = worktree.canonicalize()?;
    if worktree.parent() != Some(cas_root.join("worktrees").canonicalize()?.as_path()) {
        return Err(io::Error::other(
            "target ownership requires a private worker checkout",
        ));
    }
    let directory = cas_root.join("worker-target-owners");
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !private_metadata(&directory)?.is_dir() {
        return Err(io::Error::other("invalid ownership directory"));
    }
    let key = hex::encode(Sha256::digest(worktree.as_os_str().as_encoded_bytes()));
    Ok((
        directory.join(format!("{key}.json")),
        directory.join(format!("{key}.lock")),
    ))
}

/// Must precede seed data or Cargo. Existing unmarked targets remain legacy.
pub(crate) fn acquire(cas_root: &Path, worktree: &Path) -> io::Result<Option<Lease>> {
    let mut lease = open(cas_root, worktree, true)?;
    if let Some(lease) = lease.as_mut() {
        lease.record.pid = std::process::id();
        lease.record.start = crate::mcp::daemon::read_pid_starttime(lease.record.pid)
            .ok_or_else(|| io::Error::other("builder start identity unavailable"))?;
        lease.record.boot = boot()?;
        lease.record.active = true;
        write_record(&lease.record_path, &lease.record)?;
        lease.builder = true;
    }
    Ok(lease)
}

pub(crate) fn for_retirement(cas_root: &Path, worktree: &Path) -> io::Result<Option<Lease>> {
    open(cas_root, worktree, false)
}

fn open(cas_root: &Path, worktree: &Path, create: bool) -> io::Result<Option<Lease>> {
    let worktree = worktree.canonicalize()?;
    let target = worktree.join("target");
    if fs::symlink_metadata(&target)
        .is_ok_and(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err(io::Error::other("private target must be a real directory"));
    }
    if create && !target.exists() && !target_is_ignored(&worktree)? {
        tracing::warn!(worktree = %worktree.display(), "worker target is not ignored; using legacy output without ownership; retirement disabled");
        return Ok(None);
    }
    let (record_path, lock_path) = paths(cas_root, &worktree)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&lock_path)?;
    let lock_meta = private_metadata(&lock_path)?;
    if !lock_meta.is_file() || file.metadata()?.ino() != lock_meta.ino() {
        return Err(io::Error::other("invalid target lease"));
    }
    file.try_lock_exclusive()?;
    let fresh = if create {
        match fs::DirBuilder::new().mode(0o700).create(&target) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error),
        }
    } else {
        false
    };
    if fresh {
        let tree = worktree_metadata(&worktree)?;
        let data = private_metadata(&target)?;
        let record = Record {
            version: 1,
            worktree: worktree.clone(),
            worktree_dev: tree.dev(),
            worktree_ino: tree.ino(),
            target_dev: data.dev(),
            target_ino: data.ino(),
            lease_dev: lock_meta.dev(),
            lease_ino: lock_meta.ino(),
            pid: std::process::id(),
            start: crate::mcp::daemon::read_pid_starttime(std::process::id())
                .ok_or_else(|| io::Error::other("creator start identity unavailable"))?,
            boot: boot()?,
            active: true,
            generation: uuid::Uuid::new_v4().to_string(),
        };
        use std::io::Write;
        let mut marker = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(target.join(MARKER))?;
        marker.write_all(record.generation.as_bytes())?;
        marker.sync_all()?;
        fs::File::open(&target)?.sync_all()?;
        fs::File::open(&worktree)?.sync_all()?;
        // Durable external provenance exists before any build data is written.
        write_record(&record_path, &record)?;
    }
    let record: Record = match private_metadata(&record_path)
        .and_then(|_| fs::read(&record_path))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::other))
    {
        Ok(record) => record,
        Err(error) => {
            if target.join(MARKER).exists() {
                return Err(error);
            }
            return Ok(None);
        }
    };
    if record.worktree != worktree {
        return Ok(None);
    }
    let mut lease = Lease {
        file,
        record_path,
        lock_path,
        record,
        builder: false,
    };
    if !lease.revalidate(&target).unwrap_or(false) {
        if target.join(MARKER).exists() {
            return Err(io::Error::other(
                "target ownership changed or is unverifiable",
            ));
        }
        return Ok(None);
    }
    if !fresh && owner_live(&lease.record)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "target owner is still live",
        ));
    }
    lease.builder = fresh;
    Ok(Some(lease))
}

impl Lease {
    pub(crate) fn revalidate(&self, actual_target: &Path) -> io::Result<bool> {
        let tree = worktree_metadata(&self.record.worktree)?;
        let target = private_metadata(actual_target)?;
        let lock = self.file.metadata()?;
        let current_lock = private_metadata(&self.lock_path)?;
        let marker = actual_target.join(MARKER);
        if !private_metadata(&marker)?.is_file() {
            return Ok(false);
        }
        private_metadata(&self.record_path)?;
        let current: Record =
            serde_json::from_slice(&fs::read(&self.record_path)?).map_err(io::Error::other)?;
        Ok(self.record.version == 1
            && self.record.start != 0
            && current == self.record
            && fs::read_to_string(marker)? == self.record.generation
            && tree.is_dir()
            && target.is_dir()
            && current_lock.is_file()
            && (tree.dev(), tree.ino()) == (self.record.worktree_dev, self.record.worktree_ino)
            && (target.dev(), target.ino()) == (self.record.target_dev, self.record.target_ino)
            && (lock.dev(), lock.ino()) == (self.record.lease_dev, self.record.lease_ino)
            && (current_lock.dev(), current_lock.ino()) == (lock.dev(), lock.ino()))
    }

    pub(crate) fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    /// Slot/lane locks stay CLOEXEC; only this output lifetime lease is inherited.
    pub(crate) fn inherit(&self, command: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        let fd = self.fd();
        // SAFETY: pre_exec only calls async-signal-safe fcntl; fd is held until wait.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// cas-4ad0: environment variable naming the inherited lease fd, so the
/// test runner wrapper can close it before a test binary starts.
pub(crate) const LEASE_FD_ENV: &str = "CAS_TARGET_LEASE_FD";

/// cas-4ad0: Cargo's target-runner wrapper for test binaries. Red stub.
pub(crate) const RELEASE_RUNNER: &str = "#!/bin/sh\nexec \"$@\"\n";

/// cas-4ad0: write [`RELEASE_RUNNER`] into `dir` (0700) and return its path.
pub(crate) fn write_release_runner(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("release-target-lease-runner.sh");
    fs::write(&path, RELEASE_RUNNER)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

/// cas-4ad0: the `CARGO_TARGET_<HOST>_RUNNER` variable for this host. Red stub.
pub(crate) fn host_runner_env() -> Option<String> {
    None
}

/// cas-4ad0: processes holding an open descriptor on `lock_path`. Red stub.
pub(crate) fn lease_holders(_lock_path: &Path) -> Vec<u32> {
    Vec::new()
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.builder {
            self.record.active = false;
            if let Err(error) = write_record(&self.record_path, &self.record) {
                tracing::warn!(%error, "target ownership remains active; reclamation deferred");
            }
        }
        // Do not explicitly unlock: inherited descendants must keep the lease.
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        let worker = root.join("worktrees/worker");
        fs::create_dir_all(&worker).unwrap();
        // Reproduce ordinary Git checkout permissions under umask 0002,
        // without changing the process-wide umask in parallel tests.
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o775)).unwrap();
        (temp, root, worker)
    }

    #[test]
    fn provenance_precedes_data_and_never_adopts_legacy_cas_f96d() {
        let (_temp, root, worker) = fixture();
        let lease = acquire(&root, &worker).unwrap().unwrap();
        let record_path = lease.record_path.clone();
        assert!(
            fs::read_dir(worker.join("target"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == MARKER)
        );
        assert!(lease.revalidate(&worker.join("target")).unwrap());
        assert_eq!(
            lease.record.start,
            crate::mcp::daemon::read_pid_starttime(std::process::id()).unwrap()
        );
        assert!(for_retirement(&root, &worker).is_err());
        drop(lease);
        assert!(for_retirement(&root, &worker).unwrap().is_some());
        fs::rename(worker.join("target"), worker.join("old-target")).unwrap();
        fs::create_dir(worker.join("target")).unwrap();
        fs::write(worker.join("target/legacy-output"), b"legacy").unwrap();
        // Model filesystem inode reuse: all inode fields can match the new
        // legacy directory, but its missing generation marker still refuses.
        let mut record: Record = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        let reused = fs::metadata(worker.join("target")).unwrap();
        record.target_dev = reused.dev();
        record.target_ino = reused.ino();
        write_record(&record_path, &record).unwrap();
        // Even an old marker at this path cannot adopt a replacement inode.
        assert!(acquire(&root, &worker).unwrap().is_none());
        assert!(for_retirement(&root, &worker).unwrap().is_none());
        assert_eq!(
            fs::read(worker.join("target/legacy-output")).unwrap(),
            b"legacy"
        );
    }

    #[test]
    fn start_identity_and_replaced_lease_defeat_reuse_cas_f96d() {
        let (_temp, root, worker) = fixture();
        let lease = acquire(&root, &worker).unwrap().unwrap();
        let path = lease.record_path.clone();
        let lock = lease.lock_path.clone();
        let mut record = lease.record.clone();
        drop(lease);
        record.active = true;
        write_record(&path, &record).unwrap();
        assert!(for_retirement(&root, &worker).is_err()); // matching live owner
        record.start += 1; // same PID, different process instance
        write_record(&path, &record).unwrap();
        let dead = for_retirement(&root, &worker).unwrap().unwrap();
        let renamed = worker.join("quarantined-target");
        fs::rename(worker.join("target"), &renamed).unwrap();
        assert!(dead.revalidate(&renamed).unwrap());
        fs::remove_file(&lock).unwrap();
        fs::write(&lock, b"").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!dead.revalidate(&renamed).unwrap());
    }

    #[test]
    fn group_writable_checkout_keeps_provenance_private_cas_f96d() {
        let (_temp, root, worker) = fixture();
        let lease = acquire(&root, &worker).unwrap().unwrap();
        let record = lease.record_path.clone();
        let lock = lease.lock_path.clone();
        let target = worker.join("target");
        let marker = target.join(MARKER);
        let owners = record.parent().unwrap().to_path_buf();
        assert_eq!(fs::metadata(&worker).unwrap().mode() & 0o777, 0o775);
        for (path, mode) in [
            (&target, 0o700),
            (&owners, 0o700),
            (&record, 0o600),
            (&lock, 0o600),
            (&marker, 0o600),
        ] {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, mode);
        }
        drop(lease);
        // Group write is allowed only on the checkout, never on target or
        // provenance files. Each independent mutation must retain output.
        for (path, private_mode) in [
            (&target, 0o700),
            (&owners, 0o700),
            (&record, 0o600),
            (&lock, 0o600),
            (&marker, 0o600),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(private_mode | 0o020)).unwrap();
            assert!(
                for_retirement(&root, &worker).is_err(),
                "{} must remain private",
                path.display()
            );
            fs::set_permissions(path, fs::Permissions::from_mode(private_mode)).unwrap();
            assert!(for_retirement(&root, &worker).unwrap().is_some());
        }
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            for_retirement(&root, &worker).is_err(),
            "world-writable checkout remains unverifiable"
        );
    }

    /// cas-4ad0: a test binary started through the release runner closes
    /// the inherited lease first, so a grandchild that outlives the run (a
    /// leaked fake server) no longer locks the worktree out.
    #[test]
    fn leaked_test_grandchild_does_not_hold_the_target_lease_cas_4ad0() {
        use std::io::BufRead;
        let (_temp, root, worker) = fixture();
        let runner = write_release_runner(&root.join("worker-check-slots")).unwrap();
        let lease = acquire(&root, &worker).unwrap().unwrap();
        let lock_path = lease.lock_path.clone();
        // The runner execs the "test binary", which leaks a sleeping child.
        let mut command = std::process::Command::new(&runner);
        command
            .args(["sh", "-c", "sleep 30 </dev/null >/dev/null 2>&1 & echo $!"])
            .stdout(std::process::Stdio::piped());
        lease.inherit(&mut command);
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let grandchild: i32 = line.trim().parse().unwrap();
        assert!(child.wait().unwrap().success());
        drop(lease);
        let reacquired = for_retirement(&root, &worker);
        let holders = lease_holders(&lock_path);
        // SAFETY: kill only signals the grandchild this test started.
        unsafe { libc::kill(grandchild, libc::SIGKILL) };
        assert!(
            reacquired.is_ok_and(|lease| lease.is_some()),
            "a leaked test grandchild still holds the lease"
        );
        assert!(!holders.contains(&(grandchild as u32)), "{holders:?}");
    }

    /// cas-4ad0: a held lease refuses with the lock path and the holder PID,
    /// not a bare "Resource temporarily unavailable (os error 11)".
    #[cfg(target_os = "linux")]
    #[test]
    fn held_lease_refusal_names_lock_path_and_holder_cas_4ad0() {
        use std::io::BufRead;
        let (_temp, root, worker) = fixture();
        let lease = acquire(&root, &worker).unwrap().unwrap();
        let lock_path = lease.lock_path.clone();
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "echo ready; read release"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        lease.inherit(&mut command);
        let mut holder = command.spawn().unwrap();
        let mut ready = String::new();
        std::io::BufReader::new(holder.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        drop(lease);
        let refusal = for_retirement(&root, &worker).err().map(|error| error.to_string());
        let holders = lease_holders(&lock_path);
        holder.kill().unwrap();
        holder.wait().unwrap();
        assert!(holders.contains(&holder.id()), "{holders:?}");
        let refusal = refusal.expect("the held lease refuses");
        assert!(refusal.contains(&lock_path.display().to_string()), "{refusal}");
        assert!(refusal.contains(&holder.id().to_string()), "{refusal}");
    }

    #[test]
    fn host_runner_env_names_this_targets_cargo_runner_cas_4ad0() {
        let name = host_runner_env().expect("supported host");
        assert!(name.starts_with("CARGO_TARGET_") && name.ends_with("_RUNNER"), "{name}");
        assert_eq!(name, name.to_uppercase());
        assert!(!name.contains('-'), "{name}");
        assert!(name.contains(&std::env::consts::ARCH.to_uppercase()), "{name}");
    }

    #[test]
    fn inherited_builder_lease_survives_runner_drop_cas_f96d() {
        use std::io::BufRead;
        let (_temp, root, worker) = fixture();
        let lease = acquire(&root, &worker).unwrap().unwrap();
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "echo ready; read release"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        lease.inherit(&mut command);
        let mut child = command.spawn().unwrap();
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        drop(lease);
        let retained = for_retirement(&root, &worker).is_err();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            retained,
            "descendant must retain the inherited output lease"
        );
        assert!(for_retirement(&root, &worker).unwrap().is_some());
    }
}
