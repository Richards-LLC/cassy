//! Conservative pathname reclamation, independent of whether metadata survived.
//! Linux's Unix socket table includes non-listening sockets too. A failed
//! connect alone is insufficient proof that nobody holds a socket.

use std::{fs, io, path::Path};

#[cfg(unix)]
pub(super) fn transition_lock(base: &Path) -> io::Result<fs::File> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = fs::symlink_metadata(base)?;
    let uid = unsafe { libc::geteuid() };
    if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o002 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "factory socket directory is not owned",
        ));
    }
    let mut lock = fs::OpenOptions::new()
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
    // A run-owned binding inventory must notice a bind or metadata writer
    // without rescanning the machine registry for every project/channel.
    lock.write_all(uuid::Uuid::new_v4().as_bytes())?;
    lock.set_len(16)?;
    Ok(lock)
}

pub(super) fn inventory_lock(sessions: &Path, reap: bool) -> io::Result<Option<fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;
    let Some(base) = sessions.parent() else {
        return Ok(None);
    };
    if !sessions.exists() {
        return Ok(None);
    }
    if reap {
        return transition_lock(base).map(Some);
    }
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(base.join("factory-sockets.lock"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    fs2::FileExt::lock_shared(&file)?;
    Ok(Some(file))
}

/// Reap only the exact dead receipt inspected by update. The GUI probe is
/// advisory for metadata; socket deletion still needs the Linux kernel proof.
pub(super) fn reap_metadata_locked(
    sessions: &Path,
    path: &Path,
    expected: &[u8],
    gui_identity: Option<(u64, u64)>,
) -> io::Result<bool> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
    let Some(base) = sessions.parent() else {
        return Ok(false);
    };
    // The update inventory holds the exclusive transition lock for its scan.
    let uid = unsafe { libc::geteuid() };
    let directory = fs::symlink_metadata(sessions)?;
    if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o002 != 0 {
        return Ok(false);
    }
    let receipt = fs::symlink_metadata(path)?;
    if !receipt.is_file() || receipt.uid() != uid || receipt.nlink() != 1 {
        return Ok(false);
    }
    // The app's progress/hold mutator uses this per-record lock. Take it after
    // the namespace lock so it cannot rename an older receipt over our reap.
    let json_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path.with_extension("json.lock"))?;
    let lock_metadata = json_lock.metadata()?;
    if !lock_metadata.is_file() || lock_metadata.uid() != uid || lock_metadata.nlink() != 1 {
        return Ok(false);
    }
    fs2::FileExt::lock_exclusive(&json_lock)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let opened = file.metadata()?;
    if opened.dev() != receipt.dev() || opened.ino() != receipt.ino() {
        return Ok(false);
    }
    let metadata: super::SessionMetadata = match serde_json::from_slice(expected) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(false),
    };
    if !super::valid_session_name(&metadata.name)
        || path != sessions.join(format!("{}.json", metadata.name))
        || super::is_process_running(metadata.daemon_pid)
        || fs::read(path)? != expected
    {
        return Ok(false);
    }
    let gui = base.join(format!("factory-{}.gui.sock", metadata.name));
    match (gui_identity, fs::symlink_metadata(&gui)) {
        (None, Err(error)) if error.kind() == io::ErrorKind::NotFound => {}
        (Some(identity), Ok(current))
            if identity == (current.dev(), current.ino())
                && current.file_type().is_socket()
                && current.uid() == uid
                && current.nlink() == 1 => {}
        _ => return Ok(false),
    }
    #[cfg(target_os = "linux")]
    for socket in [base.join(format!("factory-{}.sock", metadata.name)), gui] {
        remove_locked(&socket, sessions, false)?;
        if fs::symlink_metadata(socket).is_ok() {
            return Ok(false); // Held/unknown paths retain their attribution.
        }
    }
    let current = fs::symlink_metadata(path)?;
    if current.dev() != receipt.dev()
        || current.ino() != receipt.ino()
        || !current.is_file()
        || current.uid() != uid
        || current.nlink() != 1
        || fs::read(path)? != expected
        || super::is_process_running(metadata.daemon_pid)
    {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
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
