//! Scoped detached hub fixture cleanup for integration test binaries.

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Mutex, Once, OnceLock};

/// A fixture HOME whose detached hub is terminated before the directory is
/// removed, including when a test unwinds through a panic.
pub struct PrivateHubTempDir {
    temp: tempfile::TempDir,
    test_root: PathBuf,
}

impl PrivateHubTempDir {
    pub fn path(&self) -> &Path {
        self.temp.path()
    }
}

impl Drop for PrivateHubTempDir {
    fn drop(&mut self) {
        cleanup_hub_test_home(self.path(), &self.test_root);
        #[cfg(unix)]
        if let Some(homes) = HUB_TEST_HOMES.get() {
            homes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(home, _)| home != self.path());
        }
    }
}

/// Create a private hub fixture beneath a canonical temporary directory.
/// macOS commonly spells TMPDIR through /var, a symlink rejected by hub state.
pub fn private_hub_tempdir() -> PrivateHubTempDir {
    let parent = std::env::temp_dir()
        .canonicalize()
        .expect("temporary directory must be canonicalizable");
    let temp = tempfile::tempdir_in(&parent).expect("canonical temporary fixture directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private temporary fixture directory");
    }
    std::fs::write(
        temp.path().join(".cas-test-hub-home"),
        std::process::id().to_string(),
    )
    .expect("mark test-owned hub HOME");
    #[cfg(unix)]
    {
        static REGISTER_EXIT: Once = Once::new();
        REGISTER_EXIT.call_once(|| {
            // SAFETY: the callback has C linkage, cannot unwind, and accesses
            // only a static registry whose lifetime extends through exit.
            unsafe { libc::atexit(cleanup_registered_hub_test_homes) };
        });
        HUB_TEST_HOMES
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((temp.path().to_path_buf(), parent.clone()));
    }
    PrivateHubTempDir {
        temp,
        test_root: parent,
    }
}

#[cfg(unix)]
static HUB_TEST_HOMES: OnceLock<Mutex<Vec<(PathBuf, PathBuf)>>> = OnceLock::new();

#[cfg(unix)]
extern "C" fn cleanup_registered_hub_test_homes() {
    if let Some(homes) = HUB_TEST_HOMES.get() {
        let homes = homes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for (home, root) in homes {
            cleanup_hub_test_home(&home, &root);
        }
    }
}

/// Only fixture HOMEs created beneath the canonical test temp root may be
/// cleaned. The marker also prevents a stale or unrelated temp directory from
/// becoming a cleanup target.
pub fn cleanup_detached_hub_test_home(home: &Path) {
    if let Ok(root) = std::env::temp_dir().canonicalize() {
        cleanup_hub_test_home(home, &root);
    }
}

#[cfg(unix)]
fn cleanup_hub_test_home(home: &Path, root: &Path) {
    use cas::hub::HubRuntimePaths;

    let Ok(home) = home.canonicalize() else {
        return;
    };
    if home == root || !home.starts_with(root) {
        return;
    }
    let marker = home.join(".cas-test-hub-home");
    if std::fs::read_to_string(marker)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        != Some(std::process::id())
    {
        return;
    }

    let paths = HubRuntimePaths::new(home.join(".cas/hub"));
    let record = paths.read_process_record().ok();
    let mut pids = Vec::new();
    if let Some(record) = &record {
        pids.push(record.pid);
    }
    if let Some(owner) = paths.read_lock_owner()
        && !pids.contains(&owner.pid)
    {
        pids.push(owner.pid);
    }
    for pid in pids {
        if pid <= 1 || pid == std::process::id() || !holds_hub_lock(pid, &paths.lock_path()) {
            continue;
        }
        let Some(command) = hub_process_command(pid) else {
            continue;
        };
        if !command.contains(" hub serve") {
            continue;
        }
        let group = record.as_ref().and_then(|value| {
            (value.pid == pid && value.pgid == Some(pid))
                .then(|| unsafe { libc::getpgid(pid as libc::pid_t) })
                .filter(|group| *group == pid as libc::pid_t)
                .filter(|_| unsafe { libc::getsid(pid as libc::pid_t) } == pid as libc::pid_t)
        });
        // SAFETY: the candidate owns this fixture's lock and has the exact
        // hub serve command; a group signal is used only for a proven setsid
        // leader, never for the test runner's process group.
        unsafe {
            libc::kill(group.map_or(pid as i32, |group| -group), libc::SIGTERM);
        }
        // cas-b86b: wait for the exit itself, not a fixed 0.5 s. Under a loaded
        // workspace nextest a graceful hub shutdown outlived the old budget, the
        // lock was already released so SIGKILL was skipped, and the caller saw
        // the hub still serving.
        if wait_for_hub_exit(pid, GRACEFUL_EXIT) {
            continue;
        }
        // Still running past a generous graceful deadline: force it, whether
        // or not it still holds the lock (shutdown may release it first), as
        // long as it is the same hub serve process and group.
        if hub_process_command(pid).is_some_and(|command| command.contains(" hub serve")) {
            let group = group.filter(|group| {
                let same_group = unsafe { libc::getpgid(pid as libc::pid_t) == *group };
                let same_session =
                    unsafe { libc::getsid(pid as libc::pid_t) == pid as libc::pid_t };
                same_group && same_session
            });
            unsafe {
                libc::kill(group.map_or(pid as i32, |group| -group), libc::SIGKILL);
            }
            wait_for_hub_exit(pid, FORCED_EXIT);
        }
    }
}

/// A loaded workspace nextest can stretch a graceful hub shutdown well past a
/// second; these are ceilings, and the wait ends as soon as the hub exits.
#[cfg(unix)]
const GRACEFUL_EXIT: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(unix)]
const FORCED_EXIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether `pid` has exited (gone, or a zombie awaiting its reaper) within
/// `deadline`. Linux waits on a pidfd, which becomes readable exactly when the
/// process terminates; elsewhere, or without pidfd support, it polls.
#[cfg(unix)]
fn wait_for_hub_exit(pid: u32, deadline: std::time::Duration) -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } as libc::c_int;
        if fd >= 0 {
            let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let started = std::time::Instant::now();
            let exited = loop {
                let left = deadline.saturating_sub(started.elapsed());
                // SAFETY: one valid pollfd for the fd opened above.
                let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().min(i32::MAX as u128) as i32) };
                if ready > 0 {
                    break true;
                }
                if ready == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break hub_exited(pid);
                }
            };
            // SAFETY: closes the pidfd this function opened.
            unsafe { libc::close(fd) };
            return exited;
        }
        // ESRCH: already gone. Any other error (an old kernel) polls below.
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return true;
        }
    }
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        if hub_exited(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    hub_exited(pid)
}

/// Gone, or a zombie: it no longer runs, whatever its reaper is doing.
#[cfg(unix)]
fn hub_exited(pid: u32) -> bool {
    if !hub_pid_exists(pid) {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next()) == Some("Z")
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim_start().starts_with('Z'))
    }
}

#[cfg(not(unix))]
fn cleanup_hub_test_home(_home: &Path, _root: &Path) {}

#[cfg(unix)]
fn hub_pid_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(target_os = "linux")]
fn holds_hub_lock(pid: u32, lock: &Path) -> bool {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|path| path == lock))
}

#[cfg(target_os = "macos")]
fn holds_hub_lock(pid: u32, lock: &Path) -> bool {
    ["/usr/sbin/lsof", "/usr/bin/lsof", "lsof"]
        .into_iter()
        .find_map(|binary| {
            std::process::Command::new(binary)
                .args(["-t", "-n", "-P"])
                .arg(lock)
                .output()
                .ok()
        })
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == pid.to_string())
        })
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn holds_hub_lock(_pid: u32, _lock: &Path) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn hub_process_command(pid: u32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(target_os = "macos")]
fn hub_process_command(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn hub_process_command(_pid: u32) -> Option<String> {
    None
}
