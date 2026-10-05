//! Linux process evidence. Missing/inaccessible evidence is never proof of idle.
use super::*;

#[cfg(all(test, unix))]
pub(super) fn linux_uses(
    proc_root: &Path,
    worktree: &Path,
    cache: &Path,
    own_lock_fd: Option<i32>,
) -> bool {
    let descriptors: Vec<_> = own_lock_fd.into_iter().collect();
    linux_uses_many(proc_root, worktree, cache, &descriptors)
}

pub(super) fn linux_uses_many(
    proc_root: &Path,
    worktree: &Path,
    cache: &Path,
    own_lock_fds: &[i32],
) -> bool {
    if worktree.as_os_str().is_empty() || cache.as_os_str().is_empty() {
        return true;
    }
    let Ok(processes) = fs::read_dir(proc_root) else {
        return true;
    };
    for entry in processes {
        let Ok(entry) = entry else {
            return true;
        };
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let process = entry.path();
        match process_uses(&process, pid, worktree, cache, own_lock_fds) {
            Ok(true) => return true,
            Ok(false) => {}
            // A process can exit between directory enumeration and probes.
            Err(error) if error.kind() == io::ErrorKind::NotFound && vanished(&process) => {}
            Err(_) => return true,
        }
    }
    false
}

fn vanished(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn process_uses(
    process: &Path,
    pid: u32,
    worktree: &Path,
    cache: &Path,
    own_lock_fds: &[i32],
) -> io::Result<bool> {
    let stat = fs::read_to_string(process.join("stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("malformed proc stat"))?
        .1
        .split_whitespace()
        .collect();
    let flags = fields
        .get(6)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| io::Error::other("missing proc flags"))?;
    // Zombies and kernel threads cannot hold userspace Cargo artifacts. Their
    // cwd/exe links may legitimately be absent; all live userspace probes below
    // must succeed or the result stays unknown/live.
    if matches!(fields.first().copied(), Some("Z" | "X")) || flags & 0x0020_0000 != 0 {
        return Ok(false);
    }
    for link in ["cwd", "exe"] {
        let path = fs::read_link(process.join(link))?;
        if path.starts_with(worktree) || path.starts_with(cache) {
            return Ok(true);
        }
    }
    let cmdline = fs::read(process.join("cmdline"))?;
    let worktree_bytes = worktree.as_os_str().as_encoded_bytes();
    let cache_bytes = cache.as_os_str().as_encoded_bytes();
    if cmdline.split(|byte| *byte == 0).any(|argument| {
        argument
            .windows(worktree_bytes.len())
            .any(|window| window == worktree_bytes)
            || argument
                .windows(cache_bytes.len())
                .any(|window| window == cache_bytes)
    }) {
        return Ok(true);
    }
    if maps_use(&fs::read(process.join("maps"))?, worktree, cache) {
        return Ok(true);
    }
    for fd in fs::read_dir(process.join("fd"))? {
        let fd = fd?;
        if pid == std::process::id()
            && own_lock_fds
                .iter()
                .any(|own| fd.file_name() == own.to_string().as_str())
        {
            continue; // Exempt only the eviction lock descriptor, never the PID.
        }
        match fs::read_link(fd.path()) {
            Ok(path) if path.starts_with(worktree) || path.starts_with(cache) => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && vanished(&fd.path()) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn maps_use(maps: &[u8], worktree: &Path, cache: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    for line in maps.split(|byte| *byte == b'\n') {
        let Some(start) = line.iter().position(|byte| *byte == b'/') else {
            continue;
        };
        let raw = &line[start..];
        let mut decoded = Vec::with_capacity(raw.len());
        let mut index = 0;
        while index < raw.len() {
            if raw[index] == b'\\'
                && raw
                    .get(index + 1..index + 4)
                    .is_some_and(|digits| digits.iter().all(|byte| (b'0'..=b'7').contains(byte)))
            {
                let digits = &raw[index + 1..index + 4];
                let value = u16::from(digits[0] - b'0') * 64
                    + u16::from(digits[1] - b'0') * 8
                    + u16::from(digits[2] - b'0');
                if let Ok(byte) = u8::try_from(value) {
                    decoded.push(byte);
                    index += 4;
                } else {
                    decoded.push(raw[index]);
                    index += 1;
                }
            } else {
                decoded.push(raw[index]);
                index += 1;
            }
        }
        // Match both kernel-escaped and literal names conservatively.
        if [raw, decoded.as_slice()].into_iter().any(|bytes| {
            let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
            path.starts_with(worktree) || path.starts_with(cache)
        }) {
            return true;
        }
    }
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("proc");
        let process = root.join(std::process::id().to_string());
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::write(process.join("stat"), "1 (fixture process) S 0 0 0 0 0 0").unwrap();
        symlink("/worker", process.join("cwd")).unwrap();
        symlink("/usr/bin/program", process.join("exe")).unwrap();
        fs::write(process.join("cmdline"), b"program\0").unwrap();
        fs::write(process.join("maps"), b"").unwrap();
        let cache = PathBuf::from("/worker/target/debug");
        (temp, root, process, cache)
    }

    #[test]
    fn bare_argv_executable_and_mapped_artifact_preserve_cache_cas_29b0() {
        let (_temp, root, process, cache) = fixture();
        assert!(!linux_uses(&root, &cache, &cache, None));
        fs::remove_file(process.join("exe")).unwrap();
        symlink(cache.join("binary"), process.join("exe")).unwrap();
        assert!(linux_uses(&root, &cache, &cache, None));
        fs::remove_file(process.join("exe")).unwrap();
        symlink("/usr/bin/program", process.join("exe")).unwrap();
        fs::write(
            process.join("maps"),
            b"1-2 r-xp 0 00:00 1 /worker/target/debug/library.so (deleted)\n",
        )
        .unwrap();
        assert!(linux_uses(&root, &cache, &cache, None));
    }

    #[test]
    fn retirement_exempts_exact_profile_locks_not_other_handles_cas_72f4() {
        let (_temp, root, process, cache) = fixture();
        symlink(cache.join("debug/.cargo-lock"), process.join("fd/17")).unwrap();
        symlink(cache.join("release/.cargo-lock"), process.join("fd/18")).unwrap();
        assert!(!linux_uses_many(&root, &cache, &cache, &[17, 18]));
        assert!(linux_uses_many(&root, &cache, &cache, &[17]));
        symlink(cache.join("test-output"), process.join("fd/19")).unwrap();
        assert!(linux_uses_many(&root, &cache, &cache, &[17, 18]));
    }

    #[test]
    fn own_lock_descriptor_alone_is_exempt_and_other_own_handles_are_live_cas_29b0() {
        let (_temp, root, process, cache) = fixture();
        symlink(cache.join(".cargo-lock"), process.join("fd/17")).unwrap();
        assert!(!linux_uses(&root, &cache, &cache, Some(17)));
        symlink(cache.join("artifact"), process.join("fd/18")).unwrap();
        assert!(linux_uses(&root, &cache, &cache, Some(17)));
        assert!(linux_uses(&root, &cache, &cache, None));
    }

    #[test]
    fn inaccessible_or_missing_live_process_evidence_preserves_cache_cas_29b0() {
        let (_temp, root, process, cache) = fixture();
        fs::remove_file(process.join("maps")).unwrap();
        fs::create_dir(process.join("maps")).unwrap(); // Reproducible read failure even as root.
        assert!(linux_uses(&root, &cache, &cache, None));
        fs::remove_dir(process.join("maps")).unwrap();
        assert!(linux_uses(&root, &cache, &cache, None));
        assert!(linux_uses(&root.join("unavailable"), &cache, &cache, None));
    }
}
