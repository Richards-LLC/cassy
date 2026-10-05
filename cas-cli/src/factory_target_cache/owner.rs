//! Provenance for newly created private targets. Legacy output is never adopted.
//! The external lease survives target quarantine and is inherited by builders.
use super::*;
use sha2::{Digest, Sha256};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

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

fn private_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o022 != 0
        || (metadata.is_file() && metadata.nlink() != 1)
    {
        return Err(io::Error::other("unverifiable target ownership path"));
    }
    Ok(metadata)
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
    match fs::create_dir(&directory) {
        Ok(()) => fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?,
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
        match fs::create_dir(&target) {
            Ok(()) => {
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error),
        }
    } else {
        false
    };
    if fresh {
        let tree = private_metadata(&worktree)?;
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
        Err(_) => return Ok(None),
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
        let tree = private_metadata(&self.record.worktree)?;
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

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        let worker = root.join("worktrees/worker");
        fs::create_dir_all(&worker).unwrap();
        (temp, root, worker)
    }

    #[test]
    fn provenance_precedes_data_and_never_adopts_legacy_cas_f96d() {
        let (_temp, root, worker) = fixture();
        let lease = acquire(&root, &worker).unwrap().unwrap();
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
        assert!(!dead.revalidate(&renamed).unwrap());
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
