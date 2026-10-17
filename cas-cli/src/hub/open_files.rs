//! The hub's open-file ceiling (cas-cb94). prowl's launchd hub inherited a
//! soft RLIMIT_NOFILE of 256, ran out of descriptors, and every auth-store
//! read failed until a restart. The hub raises its own soft limit at start,
//! records it, and `cas hub status` shows usage against it.

use serde::Serialize;

/// The soft limit the hub asks for at start; its hard limit caps it.
pub const HUB_NOFILE_TARGET: u64 = 8192;
/// Usage at or above this share of the limit is a warning.
const WARN_PERCENT: u64 = 80;

/// The soft open-file limit before and after [`raise_nofile_limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NofileLimit {
    pub before: u64,
    pub soft: u64,
    pub hard: u64,
}

/// Raise this process's soft open-file limit to `min(hard, target)`. It
/// never lowers a limit that is already higher.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // rlim_t is u64 on most, not all, targets
pub fn raise_nofile_limit(target: u64) -> std::io::Result<NofileLimit> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes into a valid, initialised `rlimit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let before = limit.rlim_cur as u64;
    let hard = limit.rlim_max as u64;
    let wanted = target.min(hard);
    if wanted <= before {
        return Ok(NofileLimit { before, soft: before, hard });
    }
    limit.rlim_cur = wanted as libc::rlim_t;
    // SAFETY: setrlimit reads a valid `rlimit` whose soft value is <= hard.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(NofileLimit { before, soft: wanted, hard })
}

#[cfg(not(unix))]
pub fn raise_nofile_limit(_target: u64) -> std::io::Result<NofileLimit> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// How many descriptors `pid` holds open, when this platform can tell.
#[cfg(target_os = "linux")]
pub fn open_file_count(pid: u32) -> Option<u64> {
    Some(std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count() as u64)
}

/// How many descriptors `pid` holds open, when this platform can tell.
#[cfg(target_os = "macos")]
pub fn open_file_count(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let entry = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: a null buffer asks only for the size the fd list needs.
    let needed =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    let needed = usize::try_from(needed).ok().filter(|bytes| *bytes > 0)?;
    let mut entries = vec![libc::proc_fdinfo { proc_fd: 0, proc_fdtype: 0 }; needed / entry + 1];
    let capacity = libc::c_int::try_from(entries.len() * entry).ok()?;
    // SAFETY: the buffer is valid for `capacity` bytes of proc_fdinfo rows.
    let filled = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, entries.as_mut_ptr().cast(), capacity)
    };
    usize::try_from(filled).ok().filter(|bytes| *bytes > 0).map(|bytes| (bytes / entry) as u64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn open_file_count(_pid: u32) -> Option<u64> {
    None
}

/// `cas hub status`'s open-file line: the hub's descriptors against the
/// limit it recorded at start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenFilesReport {
    pub status: &'static str,
    pub open: Option<u64>,
    pub limit: Option<u64>,
    pub message: String,
}

impl OpenFilesReport {
    pub fn new(open: Option<u64>, limit: Option<u64>) -> Self {
        let (status, message) = match (open, limit) {
            (Some(open), Some(limit)) if limit > 0 => {
                let percent = open.saturating_mul(100) / limit;
                if percent >= WARN_PERCENT {
                    ("warning", format!("{open} of {limit} open ({percent}%); near the limit, auth will fail with 503 when it is reached"))
                } else {
                    ("ok", format!("{open} of {limit} open ({percent}%)"))
                }
            }
            (Some(open), _) => ("unknown", format!("{open} open; limit not recorded (restart the hub to record it)")),
            (None, _) => ("unknown", "open-file count unavailable on this platform".to_owned()),
        };
        Self { status, open, limit, message }
    }

    pub fn for_process(pid: u32, limit: Option<u64>) -> Self {
        Self::new(open_file_count(pid), limit)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// cas-cb94: the hub lifts its soft limit to min(hard, 8192), and status
    /// reads this process's real descriptor count against it.
    #[test]
    fn hub_raises_its_open_file_limit_and_reports_usage_against_it() {
        let raised = raise_nofile_limit(HUB_NOFILE_TARGET).unwrap();
        assert_eq!(raised.soft, raised.before.max(HUB_NOFILE_TARGET.min(raised.hard)));
        assert_eq!(raise_nofile_limit(HUB_NOFILE_TARGET).unwrap().before, raised.soft);

        let report = OpenFilesReport::for_process(std::process::id(), Some(raised.soft));
        assert_eq!(report.status, "ok", "{report:?}");
        assert!(report.open.is_some_and(|open| open > 0 && open < raised.soft));
        // prowl at the edge of launchd's 256.
        assert_eq!(OpenFilesReport::new(Some(230), Some(256)).status, "warning");
    }
}
