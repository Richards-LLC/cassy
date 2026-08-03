//! cas-0a21: serialize transactional delivery merges per repository + target
//! ref.
//!
//! `worktree_merge` reads the target tip in preflight and then runs a Git
//! merge. Without a lock, two supervisors merging into the same target
//! interleave: both observe the reviewed `target_sha`, both authorize, and the
//! second merge is rooted at the first one's merge commit rather than at the
//! tip its receipt was reviewed against.
//!
//! The lock is keyed on the *canonical repository identity* (the resolved git
//! common dir, shared by a checkout and all of its linked worktrees) combined
//! with the target ref. Distinct repositories, and distinct target refs inside
//! one repository, hash to distinct keys and therefore never contend — merges
//! that cannot possibly interfere are never serialized against each other.
//!
//! This is an advisory OS file lock, so it only orders *CAS-mediated* merges.
//! It deliberately does not attempt to stop a human running `git commit`
//! directly; that case is caught by the compare-and-swap of `receipt.target_sha`
//! and by the post-merge first-parent check in the caller.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use sha2::{Digest, Sha256};

/// Domain separator so this hash can never collide with another CAS digest
/// that happens to run over the same bytes.
const DOMAIN: &str = "cas-0a21/delivery-target-lock/v1";

/// Held for the duration of a transactional delivery's critical section.
/// Releases the OS lock on drop, including on panic or early return.
pub struct DeliveryTargetLock {
    _file: File,
    key: String,
}

impl Drop for DeliveryTargetLock {
    fn drop(&mut self) {
        // `flock` is inherited across fork. Closing only the parent's file
        // descriptor does not release the lock while a forked child still
        // holds its inherited reference, even when FD_CLOEXEC ensures a later
        // exec will close it. Explicit LOCK_UN releases the shared lock at the
        // operation boundary instead of extending it to the child's lifetime.
        if let Err(error) = FileExt::unlock(&self._file) {
            // Drop must never panic (especially during unwinding), but a
            // failed unlock can strand every delivery targeting this ref.
            tracing::error!(
                lock_key = %self.key,
                error = %error,
                "failed to release delivery-target lock"
            );
        }
    }
}

impl DeliveryTargetLock {
    /// Stable, opaque identity of the (repository, target ref) pair this lock
    /// guards. Contains no raw path or branch text, so it is safe to log.
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// Compute the opaque lock key for a canonical repo identity + target ref.
///
/// Pure and total: unit-testable without touching the filesystem.
pub fn delivery_target_key(canonical_repo: &Path, target_ref: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN.as_bytes());
    hasher.update([0u8]);
    hasher.update(canonical_repo.as_os_str().as_encoded_bytes());
    hasher.update([0u8]);
    hasher.update(target_ref.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Acquire the exclusive delivery lock for `canonical_repo` + `target_ref`,
/// blocking until it is available.
///
/// `cas_root` only decides where the lock file lives — the *identity* comes
/// entirely from `canonical_repo` + `target_ref`, so two CAS roots pointed at
/// one repository still serialize correctly as long as they share a root.
pub fn lock_delivery_target(
    cas_root: &Path,
    canonical_repo: &Path,
    target_ref: &str,
) -> std::io::Result<DeliveryTargetLock> {
    let key = delivery_target_key(canonical_repo, target_ref);
    let dir = cas_root.join("locks").join("delivery-target");
    std::fs::create_dir_all(&dir)?;
    let path: PathBuf = dir.join(format!("{key}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    file.lock_exclusive()?;
    Ok(DeliveryTargetLock { _file: file, key })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn key_is_stable_and_separates_repositories_and_targets() {
        let repo_a = PathBuf::from("/repos/alpha/.git");
        let repo_b = PathBuf::from("/repos/beta/.git");

        // Stable across calls — the lock must be re-findable.
        assert_eq!(
            delivery_target_key(&repo_a, "main"),
            delivery_target_key(&repo_a, "main")
        );
        // Same repository, different target refs: independent.
        assert_ne!(
            delivery_target_key(&repo_a, "main"),
            delivery_target_key(&repo_a, "release")
        );
        // Different repositories, same target ref: independent.
        assert_ne!(
            delivery_target_key(&repo_a, "main"),
            delivery_target_key(&repo_b, "main")
        );
        // Field boundaries are unambiguous: no concatenation collision
        // between (repo "…/a", ref "bc") and (repo "…/ab", ref "c").
        assert_ne!(
            delivery_target_key(Path::new("/r/a"), "bc"),
            delivery_target_key(Path::new("/r/ab"), "c")
        );
    }

    #[test]
    fn independent_targets_do_not_block_each_other() {
        let temp = TempDir::new().unwrap();
        let repo = PathBuf::from("/repos/alpha/.git");
        // Holding `main` must not prevent acquiring `release` in the same
        // repository, nor the same ref in a different repository.
        let _main = lock_delivery_target(temp.path(), &repo, "main").unwrap();
        let _release = lock_delivery_target(temp.path(), &repo, "release").unwrap();
        let _other_repo =
            lock_delivery_target(temp.path(), Path::new("/repos/beta/.git"), "main").unwrap();
    }

    #[test]
    fn same_repo_and_target_is_mutually_exclusive_across_handles() {
        let temp = TempDir::new().unwrap();
        let repo = PathBuf::from("/repos/alpha/.git");
        let held = lock_delivery_target(temp.path(), &repo, "main").unwrap();
        let key = held.key().to_string();

        // A second *process-level* acquisition of the same key must not be
        // grantable while the first is held. fs2 locks are per-file-handle,
        // so assert via try_lock on an independent handle.
        let path = temp
            .path()
            .join("locks")
            .join("delivery-target")
            .join(format!("{key}.lock"));
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(
            contender.try_lock_exclusive().is_err(),
            "a held delivery-target lock must exclude a concurrent acquirer"
        );

        drop(held);
        // Released on drop, so the contender can now take it.
        contender
            .try_lock_exclusive()
            .expect("released delivery-target lock must be immediately reacquirable");
    }

    #[cfg(unix)]
    #[test]
    fn delivery_target_lock_descriptor_is_close_on_exec() {
        use std::os::fd::AsRawFd;

        let temp = TempDir::new().unwrap();
        let held =
            lock_delivery_target(temp.path(), Path::new("/repos/alpha/.git"), "main").unwrap();
        let flags = unsafe { libc::fcntl(held._file.as_raw_fd(), libc::F_GETFD) };

        assert!(flags >= 0, "F_GETFD must succeed for the lock descriptor");
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "delivery-target locks must not leak across exec"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawned_process_does_not_retain_delivery_target_lock() {
        let temp = TempDir::new().unwrap();
        let repo = PathBuf::from("/repos/alpha/.git");
        let held = lock_delivery_target(temp.path(), &repo, "main").unwrap();
        let key = held.key().to_string();
        let path = temp
            .path()
            .join("locks")
            .join("delivery-target")
            .join(format!("{key}.lock"));
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 5"])
            .spawn()
            .expect("spawn a child that stays alive after exec");
        drop(held);
        let reacquire = contender.try_lock_exclusive();
        let _ = child.kill();
        let _ = child.wait();

        reacquire.expect("an exec'd child must not inherit the delivery-target lock");
    }

    #[cfg(unix)]
    #[test]
    fn dropping_guard_unlocks_even_when_forked_child_inherits_descriptor() {
        let temp = TempDir::new().unwrap();
        let repo = PathBuf::from("/repos/alpha/.git");
        let held = lock_delivery_target(temp.path(), &repo, "main").unwrap();
        let key = held.key().to_string();
        let path = temp
            .path()
            .join("locks")
            .join("delivery-target")
            .join(format!("{key}.lock"));
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        let mut pipe_fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
        let child = unsafe { libc::fork() };
        assert!(
            child >= 0,
            "fork failed: {}",
            std::io::Error::last_os_error()
        );
        if child == 0 {
            unsafe {
                libc::close(pipe_fds[1]);
                let mut byte = 0u8;
                let _ = libc::read(
                    pipe_fds[0],
                    (&mut byte as *mut u8).cast::<libc::c_void>(),
                    1,
                );
                libc::close(pipe_fds[0]);
                libc::_exit(0);
            }
        }

        unsafe { libc::close(pipe_fds[0]) };
        drop(held);
        let reacquire = contender.try_lock_exclusive();

        let byte = 1u8;
        unsafe {
            let _ = libc::write(pipe_fds[1], (&byte as *const u8).cast::<libc::c_void>(), 1);
            libc::close(pipe_fds[1]);
            let mut status = 0;
            let _ = libc::waitpid(child, &mut status, 0);
        }

        reacquire.expect(
            "dropping the guard must unlock even while a forked child retains the descriptor",
        );
    }
}
