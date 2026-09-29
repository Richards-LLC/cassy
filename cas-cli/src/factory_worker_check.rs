//! Compile-only worker evidence. Admission is serialized and each Cargo child
//! holds an OS slot lock, including when its supervising wrapper is killed.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize)]
struct CheckReceipt {
    head: String,
    repo: PathBuf,
    packages: Vec<String>,
}

pub(crate) fn valid_package(package: &str) -> bool {
    package
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Deliberately excludes glob/package-ID expressions, aliases, flags that
/// broaden the scope, and arbitrary Cargo configuration or toolchains.
pub(crate) fn check_packages(args: &[String]) -> Option<Vec<String>> {
    let mut packages = Vec::new();
    let mut tests = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-p" if args.get(index + 1).is_some_and(|arg| valid_package(arg)) => {
                packages.push(args[index + 1].clone());
                index += 2;
            }
            "--tests" if !tests => {
                tests = true;
                index += 1;
            }
            _ => return None,
        }
    }
    (tests && !packages.is_empty()).then_some(packages)
}

fn lock_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
}

fn acquire_slot(root: &Path, cap: usize) -> Result<File> {
    std::fs::create_dir_all(root)?;
    for index in 0..cap {
        let slot = lock_file(&root.join(format!("slot-{index}.lock")))?;
        match slot.try_lock_exclusive() {
            Ok(()) => return Ok(slot),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Worker check refused: max_concurrent_builders={cap} slots are occupied; retry later")
}

#[cfg(unix)]
fn inherit_slot(slot: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    // Keep the slot live in Cargo/rustc descendants if this wrapper is killed.
    // SAFETY: the file is live and fcntl only changes this owned descriptor.
    let result = unsafe {
        let flags = libc::fcntl(slot.as_raw_fd(), libc::F_GETFD);
        if flags < 0 {
            -1
        } else {
            libc::fcntl(slot.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC)
        }
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn inherit_slot(_slot: &File) -> Result<()> {
    bail!("Worker checks require inherited Unix slot locks on this platform")
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(repo).output()?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn clean_head(repo: &Path) -> Result<String> {
    if !git(repo, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        bail!("Commit the worker change before checking; check: PASS must name a clean commit");
    }
    git(repo, &["rev-parse", "HEAD"])
}

fn receipt_path(cas_root: &Path, repo: &Path, head: &str) -> PathBuf {
    let key = hex::encode(Sha256::digest(repo.as_os_str().as_encoded_bytes()));
    cas_root
        .join("worker-checks")
        .join(key)
        .join(format!("{head}.json"))
}

pub(crate) fn passing_receipt(cas_root: &Path, repo: &Path, head: &str) -> Option<String> {
    let repo = repo.canonicalize().ok()?;
    let receipt: CheckReceipt =
        serde_json::from_slice(&std::fs::read(receipt_path(cas_root, &repo, head)).ok()?).ok()?;
    (receipt.repo == repo
        && receipt.head == head
        && !receipt.packages.is_empty()
        && receipt
            .packages
            .iter()
            .all(|package| valid_package(package)))
    .then(|| format!("check: PASS {head} packages={}", receipt.packages.join(",")))
}

pub(crate) fn execute(cas_root: &Path, args: &[String]) -> Result<()> {
    execute_at(
        cas_root,
        args,
        &std::env::current_dir()?,
        Path::new("cargo"),
    )
}

fn execute_at(cas_root: &Path, args: &[String], cwd: &Path, cargo: &Path) -> Result<()> {
    let packages = check_packages(args)
        .context("Only cargo check -p <crate> [-p <crate> ...] --tests is allowed")?;
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let cas_root = cas_root.canonicalize()?;
    // Check only isolated, seeded worker caches; never use a shared target.
    if !repo.starts_with(cas_root.join("worktrees")) {
        bail!(
            "Worker check requires an isolated worktree under {}",
            cas_root.join("worktrees").display()
        );
    }
    let head = clean_head(&repo)?;
    let receipt = receipt_path(&cas_root, &repo, &head);
    // A failed retry must not leave an earlier PASS at this SHA.
    match std::fs::remove_file(&receipt) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let config = crate::config::Config::load(&cas_root)?.factory();
    let slots = cas_root.join("worker-check-slots");
    std::fs::create_dir_all(&slots)?;
    let admission = lock_file(&slots.join("admission.lock"))?;
    admission.lock_exclusive()?;
    let snapshot = crate::factory_build_guard::inspect(&cas_root, &config, 1);
    if !snapshot.violations().is_empty() {
        bail!(
            "Worker check refused: {}; retry later",
            snapshot.violations().join("; ")
        );
    }
    // OS locks close the snapshot-to-spawn race, even when the soft guard's
    // environment override is set. No force/disable option bypasses the cap.
    let slot = acquire_slot(&slots, config.max_concurrent_builders)?;
    inherit_slot(&slot)?;
    let mut child = Command::new(cargo)
        .arg("check")
        .args(args)
        .current_dir(cwd)
        .env("CARGO_TARGET_DIR", repo.join("target"))
        .spawn()
        .context("start package-scoped cargo check")?;
    FileExt::unlock(&admission)?;
    drop(admission);
    let status = child.wait()?;
    drop(slot);
    if !status.success() {
        bail!("check: FAIL {head} ({status})");
    }
    if clean_head(&repo)? != head {
        bail!("Worker tree changed during check; no PASS receipt recorded");
    }
    let record = CheckReceipt {
        head: head.clone(),
        repo,
        packages,
    };
    std::fs::create_dir_all(receipt.parent().context("receipt directory")?)?;
    std::fs::write(&receipt, serde_json::to_vec(&record)?)?;
    println!("check: PASS {head} packages={}", record.packages.join(","));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_package_scoped_tests_checks_are_accepted() {
        let args = |text: &str| {
            text.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            check_packages(&args("-p cas -p cas-pty --tests")).unwrap(),
            ["cas", "cas-pty"]
        );
        assert!(check_packages(&args("--tests -p cas")).is_some());
        for invalid in [
            "--tests",
            "-p cas",
            "-p '*' --tests",
            "-p cas --tests --lib",
            "-p cas --tests --workspace",
            "-p cas --tests --config x=y",
            "-p cas --tests --tests",
            "-p --tests",
            "-p ../cas --tests",
        ] {
            assert!(check_packages(&args(invalid)).is_none(), "{invalid}");
        }
    }

    #[test]
    fn slots_enforce_cap_and_release_when_closed() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire_slot(dir.path(), 2).unwrap();
        let second = acquire_slot(dir.path(), 2).unwrap();
        assert!(acquire_slot(dir.path(), 2).is_err());
        drop(first);
        assert!(acquire_slot(dir.path(), 2).is_ok());
        drop(second);
        assert!(acquire_slot(dir.path(), 0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn child_keeps_slot_after_wrapper_drops_its_handle() {
        let dir = tempfile::tempdir().unwrap();
        let slot = acquire_slot(dir.path(), 1).unwrap();
        inherit_slot(&slot).unwrap();
        let mut child = Command::new("sleep").arg("1").spawn().unwrap();
        drop(slot);
        assert!(acquire_slot(dir.path(), 1).is_err());
        assert!(child.wait().unwrap().success());
        assert!(acquire_slot(dir.path(), 1).is_ok());
    }

    #[test]
    fn receipt_is_bound_to_worktree_and_exact_commit() {
        let root = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path().canonicalize().unwrap();
        let head = "a".repeat(40);
        let path = receipt_path(root.path(), &repo, &head);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let record = CheckReceipt {
            head: head.clone(),
            repo: repo.clone(),
            packages: vec!["cas".into()],
        };
        std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            passing_receipt(root.path(), &repo, &head)
                .unwrap()
                .starts_with(&format!("check: PASS {head}"))
        );
        assert!(passing_receipt(root.path(), &repo, &"b".repeat(40)).is_none());
        assert!(passing_receipt(root.path(), root.path(), &head).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn runner_checks_clean_commit_and_invalidates_failed_retry() {
        use std::os::unix::fs::PermissionsExt;
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]).unwrap();
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        let fake = dir.path().join("fake-cargo");
        std::fs::write(&fake, "#!/bin/sh\n[ \"$*\" = 'check -p cas --tests' ] || exit 2\n[ \"$CARGO_TARGET_DIR\" = \"$PWD/target\" ] || exit 3\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = vec!["-p".into(), "cas".into(), "--tests".into()];
        execute_at(&root, &args, &repo, &fake).unwrap();
        let head = clean_head(&repo).unwrap();
        assert!(passing_receipt(&root, &repo, &head).is_some());
        std::fs::write(&fake, "#!/bin/sh\nexit 12\n").unwrap();
        assert!(
            execute_at(&root, &args, &repo, &fake)
                .unwrap_err()
                .to_string()
                .contains("check: FAIL")
        );
        assert!(passing_receipt(&root, &repo, &head).is_none());
        std::fs::write(repo.join("dirty.rs"), "// dirty").unwrap();
        assert!(
            execute_at(&root, &args, &repo, &fake)
                .unwrap_err()
                .to_string()
                .contains("Commit the worker change")
        );
        assert!(execute_at(&root, &args, dir.path(), &fake).is_err());
    }
}
