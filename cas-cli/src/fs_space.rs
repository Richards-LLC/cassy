//! Portable filesystem-space queries.
//!
//! statvfs(3) field widths vary by platform: Linux exposes the block counts
//! as `u64`, while macOS exposes them as `u32` and keeps `f_frsize` as `u64`.
//! This module is the sole raw syscall boundary and widens every field before
//! arithmetic. Callers receive byte-denominated `u64` values while retaining
//! the important distinction between unprivileged-available and total-free
//! blocks.

use std::io;
use std::path::Path;

/// Byte-denominated space figures for the filesystem backing a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsSpace {
    /// Total filesystem size (`f_blocks`).
    pub total_bytes: u64,
    /// Space usable by an unprivileged process (`f_bavail`), excluding blocks
    /// reserved for privileged users.
    pub available_bytes: u64,
    /// All free space (`f_bfree`), including privileged reserved blocks.
    pub free_bytes: u64,
}

impl FsSpace {
    /// Bytes in use, derived from total blocks minus all free blocks.
    pub fn used_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }
}

/// Query the filesystem backing `path` and widen raw fields before arithmetic.
#[cfg(unix)]
pub fn fs_space(path: &Path) -> io::Result<FsSpace> {
    use libc::statvfs;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut stat = std::mem::MaybeUninit::<statvfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated and `stat` points to writable storage.
    if unsafe { statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs returned success and initialized the structure.
    let stat = unsafe { stat.assume_init() };

    // These casts are load-bearing on macOS and no-ops on Linux. Always widen
    // before multiplying so platform-specific field widths cannot control the
    // arithmetic type or truncate the result.
    let fragment_size = stat.f_frsize as u64;
    Ok(FsSpace {
        total_bytes: (stat.f_blocks as u64).saturating_mul(fragment_size),
        available_bytes: (stat.f_bavail as u64).saturating_mul(fragment_size),
        free_bytes: (stat.f_bfree as u64).saturating_mul(fragment_size),
    })
}

/// Non-Unix platforms do not expose statvfs(3).
#[cfg(not(unix))]
pub fn fs_space(_path: &Path) -> io::Result<FsSpace> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "filesystem space reporting requires statvfs",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_bytes_uses_free_not_unprivileged_available() {
        let space = FsSpace {
            total_bytes: 1_000,
            available_bytes: 200,
            free_bytes: 300,
        };

        assert_eq!(space.used_bytes(), 700);
        assert_ne!(
            space.used_bytes(),
            space.total_bytes - space.available_bytes,
            "reserved blocks make f_bavail the wrong source for used bytes"
        );
    }

    #[test]
    fn used_bytes_saturates_when_free_exceeds_total() {
        let space = FsSpace {
            total_bytes: 100,
            available_bytes: 0,
            free_bytes: 500,
        };
        assert_eq!(space.used_bytes(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn fs_space_reports_plausible_figures_for_a_real_path() {
        let temp = tempfile::tempdir().unwrap();
        let space = fs_space(temp.path()).expect("statvfs should inspect a temp directory");

        assert!(space.total_bytes > 0);
        assert!(space.free_bytes <= space.total_bytes);
        assert!(space.available_bytes <= space.free_bytes);
        assert_eq!(space.used_bytes(), space.total_bytes - space.free_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn fs_space_rejects_nul_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/tmp/bad\0path"));
        let error = fs_space(path).expect_err("NUL path must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
