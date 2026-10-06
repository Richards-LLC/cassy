//! Conservative pathname reclamation, independent of whether metadata survived.
//! Linux's Unix socket table includes non-listening sockets too. A failed
//! connect alone is insufficient proof that nobody holds a socket.

use std::{fs, io, path::Path};

#[cfg(unix)]
fn transition_lock(base: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = fs::symlink_metadata(base)?;
    let uid = unsafe { libc::geteuid() };
    if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o002 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "factory socket directory is not owned",
        ));
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(base.join("factory-sockets.lock"))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "factory socket lock is not owned",
        ));
    }
    fs2::FileExt::lock_exclusive(&lock)?;
    Ok(lock)
}

/// Serialize stale checks/unlink/bind with GC. After bind, the kernel holder
/// inventory protects the listener, so the transition lock need not outlive it.
#[cfg(unix)]
pub(super) fn bind(path: &Path, sessions: &Path) -> io::Result<std::os::unix::net::UnixListener> {
    let base = sessions
        .parent()
        .ok_or_else(|| io::Error::other("missing session base"))?;
    fs::create_dir_all(sessions)?;
    let _lock = transition_lock(base)?;
    #[cfg(target_os = "linux")]
    {
        remove_locked(path, sessions, true)?;
        // The fork-after-init path writes a temporary parent-PID receipt before
        // binding GUI in the child. Clear an unheld old GUI path before that
        // receipt replaces the dead daemon's ownership evidence.
        if !path.to_string_lossy().ends_with(".gui.sock") {
            remove_locked(&path.with_extension("gui.sock"), sessions, true)?;
        }
    }
    std::os::unix::net::UnixListener::bind(path)
}

#[cfg(target_os = "linux")]
pub(super) fn remove_if_unheld(path: &Path, sessions: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound) {
        return Ok(());
    }
    let Some(base) = sessions.parent() else {
        return Ok(());
    };
    let _lock = match transition_lock(base) {
        Ok(lock) => lock,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    remove_locked(path, sessions, false)
}

#[cfg(target_os = "linux")]
fn remove_locked(path: &Path, sessions: &Path, binding: bool) -> io::Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let Some(base) = sessions.parent() else {
        return Ok(());
    };
    // Refuse symlinked directories, foreign owners, special files and hard-link
    // aliases. The caller only supplies canonical factory names under this base.
    let uid = unsafe { libc::geteuid() };
    for directory in [base, sessions] {
        let Ok(meta) = fs::symlink_metadata(directory) else {
            return Ok(());
        };
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o002 != 0 {
            return Ok(());
        }
    }
    let original = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !original.file_type().is_socket() || original.uid() != uid || original.nlink() != 1 {
        return Ok(());
    }
    if !owners_are_dead(path, sessions, binding) || !kernel_proves_unheld(path, &original) {
        return Ok(());
    }
    let Ok(current) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if current.dev() != original.dev()
        || current.ino() != original.ino()
        || current.uid() != original.uid()
        || !current.file_type().is_socket()
        || current.nlink() != 1
        || !owners_are_dead(path, sessions, binding)
        || !kernel_proves_unheld(path, &current)
    {
        return Ok(());
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(target_os = "linux")]
fn owners_are_dead(path: &Path, sessions: &Path, binding: bool) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = fs::read_dir(sessions) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        if entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } {
            return false;
        }
        // A broken receipt cannot prove ownership or death: never infer an
        // orphan from a failed metadata load.
        let Ok(json) = fs::read_to_string(entry.path()) else {
            return false;
        };
        let Ok(owner) = serde_json::from_str::<super::SessionMetadata>(&json) else {
            return false;
        };
        if !super::valid_session_name(&owner.name) {
            return false;
        }
        let base = sessions.parent().unwrap();
        let claims_path = Path::new(&owner.socket_path) == path
            || base.join(format!("factory-{}.sock", owner.name)) == path
            || base.join(format!("factory-{}.gui.sock", owner.name)) == path;
        // Any live PID is spared here, even when its fingerprint was replaced.
        // The actual holder check below supplies a second, independent fence.
        if claims_path
            && super::is_process_running(owner.daemon_pid)
            && !(binding && owner.daemon_pid == std::process::id())
        {
            return false;
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn kernel_proves_unheld(path: &Path, candidate: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(table) = fs::read_to_string("/proc/net/unix") else {
        return false;
    };
    if !table.starts_with("Num") {
        return false;
    }
    for line in table.lines().skip(1) {
        let mut rest = line;
        // The path is the remainder after seven columns, so embedded spaces
        // in HOME are preserved. The table also contains unnamed sockets.
        for column in 0..7 {
            rest = rest.trim_start();
            match rest.find(char::is_whitespace) {
                Some(end) => rest = &rest[end..],
                None if column == 6 => {
                    rest = "";
                    break;
                }
                None => return false,
            }
        }
        let held_path = Path::new(rest.trim_start());
        if held_path.as_os_str().is_empty() {
            continue;
        }
        if held_path == path {
            return false;
        }
        if let Ok(held) = fs::symlink_metadata(held_path) {
            if held.dev() == candidate.dev() && held.ino() == candidate.ino() {
                return false;
            }
        }
    }
    true
}

#[cfg(not(target_os = "linux"))]
pub(super) fn remove_if_unheld(_path: &Path, _sessions: &Path) -> io::Result<()> {
    // No reliable holder inventory on this platform: retain the pathname.
    Ok(())
}
