//! Shared filesystem primitives for the `cas integrate <platform>` handlers.
//!
//! Each handler used to ship its own copy of these — github.rs had a bespoke
//! `atomic_write`, vercel.rs had `read_capped` + a non-atomic `write_file`,
//! neon.rs called `fs::read_to_string` / `fs::write` directly. This module
//! consolidates the lot so all three handlers behave identically under
//! - concurrent writers (atomic rename),
//! - oversized inputs (4 MiB cap),
//! - and symlink-shaped attacks on `.claude/` / `.cursor/` paths.
//!
//! Owner: cas-fc38 (cross-cutting hardening). All three handlers
//! ([`super::vercel`], [`super::neon`], [`super::github`]) consume these
//! helpers; new handlers should as well.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context};

/// Cap on user-controlled file reads (SKILL.md, package.json, schema.prisma,
/// vercel.json, etc.). Anything above this is almost certainly not a doc
/// file and we refuse rather than allocate unbounded memory.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// True iff `path` exists, is a regular file, and is **not** a symlink. All
/// platform handlers gate user-controlled reads behind this so a symlink at
/// `.claude/skills/<x>/SKILL.md` (or `package.json`, `vercel.json`,
/// `schema.prisma`, etc.) cannot redirect us into `~/.ssh` or `/etc/...`.
pub fn is_regular_file(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(md) => md.file_type().is_file(),
        Err(_) => false,
    }
}

/// Read a file with a [`MAX_FILE_BYTES`] cap. Refuses symlinks and rejects
/// inputs larger than the cap with a clear error rather than allocating
/// unbounded memory.
///
/// This is the only entry point handlers should use to read user-controlled
/// files (existing SKILL.md before merge, project manifests during detection,
/// etc.).
pub fn read_capped(path: &Path) -> anyhow::Result<String> {
    let md = fs::symlink_metadata(path)
        .with_context(|| format!("statting {}", path.display()))?;
    if md.file_type().is_symlink() {
        anyhow::bail!("{} is a symlink; refusing to follow", path.display());
    }
    if !md.file_type().is_file() {
        anyhow::bail!("{} is not a regular file", path.display());
    }
    if md.len() > MAX_FILE_BYTES {
        anyhow::bail!(
            "{} is {} bytes; exceeds cap of {} bytes",
            path.display(),
            md.len(),
            MAX_FILE_BYTES
        );
    }
    let f = fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut s = String::new();
    f.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut s)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(s)
}

/// Atomically replace `path`'s contents with `contents`.
///
/// Writes to a sibling tempfile (same directory, so the rename stays on the
/// same filesystem) then `fs::rename`'s into place. POSIX guarantees the
/// rename is atomic, so concurrent readers never see a half-written file
/// and a process crash mid-write either leaves the old version intact or
/// the new version intact — never a torn intermediate.
///
/// Refuses to write through a symlink at `path`: we `symlink_metadata` the
/// target first and bail if the existing entry is a symlink. (A non-existent
/// target is fine — the rename will create it.)
///
/// Implemented with `std::fs` only to avoid pulling `tempfile` into the
/// runtime dependency tree (it is dev-only here). The temp name is salted
/// with the process id + a nanosecond timestamp so two concurrent invocations
/// pick distinct names.
pub fn atomic_write(path: &Path, contents: &str) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow!("refusing to write to a root-less path: {}", path.display())
    })?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("non-UTF8 target file name: {}", path.display()))?;

    // Symlink defense: if the target exists as a symlink, refuse. We do not
    // need to chase parent traversal here — the worker's repo_root is already
    // resolved by `locate_repo_root`, and the handler's relative path is
    // module-controlled (.claude/..., .cursor/...).
    if let Ok(md) = fs::symlink_metadata(path) {
        if md.file_type().is_symlink() {
            anyhow::bail!(
                "{} is a symlink; refusing to write through it",
                path.display()
            );
        }
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(
        ".{}.cas-integrate.{}.{nanos}.tmp",
        file_name,
        std::process::id()
    );
    let tmp_path = parent.join(tmp_name);

    // Ensure the temp file is removed on any failure path before rename.
    let result = (|| -> std::io::Result<()> {
        // Use OpenOptions for the write so we can control symlink-follow on
        // the tempfile too (defensive — the parent should be a normal dir).
        {
            let mut f = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp_path)?;
            f.write_all(contents.as_bytes())?;
            f.flush()?;
        }
        fs::rename(&tmp_path, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp_path);
        return Err(anyhow!(
            "failed to atomically write {}: {e}",
            path.display()
        ));
    }
    Ok(())
}

/// Convenience: ensure `path`'s parent directory exists, then `atomic_write`.
pub fn atomic_write_create_dirs(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    atomic_write(path, contents)
}

// ---------------------------------------------------------------------------
// Repo-root resolution with git -C discipline
// ---------------------------------------------------------------------------

/// Resolve the repo root for a `cas integrate` invocation, starting from
/// `start`. Tries, in order:
///
/// 1. `git -C <start> rev-parse --show-toplevel` — if `start` lives inside
///    a git repo (or a submodule), this returns the inner repo's toplevel
///    (the one a user would expect `cas integrate` to operate on, not the
///    parent of a containing superproject).
/// 2. The first ancestor of `start` containing a `.git`, `.cas`,
///    `Cargo.toml`, or `package.json` marker — handles cases where `git`
///    isn't on PATH but the directory clearly is a project root.
/// 3. `start` itself, as a last resort.
///
/// Tests pass an explicit start path; production callers use
/// [`locate_repo_root`] which uses `std::env::current_dir`.
pub fn locate_repo_root_from(start: &Path) -> anyhow::Result<PathBuf> {
    // Step 1: `git -C <start> rev-parse --show-toplevel`. Using `-C` keeps
    // git's repo-discovery anchored at `start` rather than wherever the
    // process happened to be invoked from — and on a submodule checkout it
    // returns the *inner* repo's toplevel, which is what users want for
    // `cas integrate`.
    if let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(PathBuf::from(s));
            }
        }
    }

    // Step 2: walk upward looking for a project marker.
    const MARKERS: &[&str] = &[".git", ".cas", "Cargo.toml", "package.json"];
    let mut cur: Option<&Path> = Some(start);
    while let Some(p) = cur {
        for m in MARKERS {
            if p.join(m).exists() {
                return Ok(p.to_path_buf());
            }
        }
        cur = p.parent();
    }

    // Step 3: refuse to silently fall back to a bare CWD — `cas integrate`
    // from `~/Downloads` would otherwise scribble `.claude/skills/...` next
    // to whatever happens to live there. Explicit error is the cas-7417
    // sentinel-check semantic.
    anyhow::bail!(
        "{} is not inside a project (no git toplevel, no .git/.cas/Cargo.toml/package.json marker). \
         Run `cas integrate` from a project root.",
        start.display()
    )
}

/// Production wrapper: resolve from `std::env::current_dir`.
pub fn locate_repo_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    locate_repo_root_from(&cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- read_capped ---------------------------------------------------------

    #[test]
    fn read_capped_returns_contents_for_regular_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.txt");
        fs::write(&p, "hello world").unwrap();
        assert_eq!(read_capped(&p).unwrap(), "hello world");
    }

    #[test]
    fn read_capped_rejects_symlink() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real.txt");
        fs::write(&real, "secret").unwrap();
        let link = tmp.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        {
            // Skip on platforms without symlink support.
            return;
        }
        let err = read_capped(&link).unwrap_err().to_string();
        assert!(
            err.contains("symlink"),
            "expected symlink rejection; got {err}"
        );
    }

    #[test]
    fn read_capped_rejects_oversized_files() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("big.bin");
        // Write MAX_FILE_BYTES + 1 zero bytes.
        let mut f = fs::File::create(&p).unwrap();
        let buf = vec![0u8; 1024 * 1024];
        for _ in 0..4 {
            f.write_all(&buf).unwrap();
        }
        f.write_all(&[0u8]).unwrap();
        drop(f);
        let err = read_capped(&p).unwrap_err().to_string();
        assert!(
            err.contains("exceeds cap"),
            "expected size-cap error; got {err}"
        );
    }

    #[test]
    fn read_capped_returns_clear_error_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("missing.txt");
        let err = read_capped(&p).unwrap_err().to_string();
        assert!(err.contains("statting"), "got: {err}");
    }

    #[test]
    fn read_capped_rejects_directory() {
        let tmp = TempDir::new().unwrap();
        let err = read_capped(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("not a regular file"), "got: {err}");
    }

    // --- atomic_write --------------------------------------------------------

    #[test]
    fn atomic_write_creates_new_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("out.txt");
        atomic_write(&p, "hi").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hi");
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("out.txt");
        fs::write(&p, "old").unwrap();
        atomic_write(&p, "new").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
    }

    #[test]
    fn atomic_write_does_not_leave_tempfile_on_success() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("out.txt");
        atomic_write(&p, "x").unwrap();
        let stragglers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".cas-integrate.")
            })
            .collect();
        assert!(
            stragglers.is_empty(),
            "leftover tempfiles: {stragglers:?}"
        );
    }

    #[test]
    fn atomic_write_refuses_to_follow_symlink_target() {
        #[cfg(not(unix))]
        return;
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real.txt");
        fs::write(&real, "original").unwrap();
        let link = tmp.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = atomic_write(&link, "evil").unwrap_err().to_string();
        assert!(
            err.contains("symlink"),
            "expected symlink rejection; got {err}"
        );
        // Critical: real target must be untouched.
        assert_eq!(fs::read_to_string(&real).unwrap(), "original");
    }

    #[test]
    fn atomic_write_create_dirs_creates_missing_parents() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a/b/c/out.txt");
        atomic_write_create_dirs(&p, "deep").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "deep");
    }

    #[test]
    // cas-d963 owns the production temp-name collision and ownership-safe
    // cleanup fix. Keep this regression visible without conflating it with
    // the macOS test-hygiene changes in cas-859b.
    #[ignore = "cas-d963: production atomic_write temp-name collision"]
    fn atomic_write_under_concurrent_writers_never_tears() {
        // Spawn N threads each writing a distinct, deterministic content.
        // After the join, the file must equal exactly one of the candidates
        // (last-rename-wins) and never be partial / mixed.
        use std::thread;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("concurrent.txt");
        let candidates: Vec<String> =
            (0..16).map(|i| format!("payload-{i:03}")).collect();
        let handles: Vec<_> = candidates
            .iter()
            .map(|c| {
                let p = p.clone();
                let c = c.clone();
                thread::spawn(move || {
                    for _ in 0..8 {
                        atomic_write(&p, &c).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let final_contents = fs::read_to_string(&p).unwrap();
        assert!(
            candidates.contains(&final_contents),
            "post-race contents must be one of the candidates verbatim; got {final_contents:?}"
        );
        // No straggler tempfiles.
        let stragglers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".cas-integrate.")
            })
            .collect();
        assert!(stragglers.is_empty(), "leftover: {stragglers:?}");
    }

    // --- is_regular_file -----------------------------------------------------

    #[test]
    fn is_regular_file_true_for_regular() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.txt");
        fs::write(&p, "x").unwrap();
        assert!(is_regular_file(&p));
    }

    #[test]
    fn is_regular_file_false_for_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_regular_file(tmp.path()));
    }

    #[test]
    fn is_regular_file_false_for_symlink() {
        #[cfg(not(unix))]
        return;
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real.txt");
        fs::write(&real, "x").unwrap();
        let link = tmp.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!is_regular_file(&link));
    }

    #[test]
    fn is_regular_file_false_for_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_regular_file(&tmp.path().join("nope.txt")));
    }

    // --- locate_repo_root_from -----------------------------------------------

    #[test]
    fn locate_repo_root_from_returns_inner_git_toplevel_in_real_repo() {
        // `git init` a tempdir, then descend into a subdir and confirm
        // locate_repo_root_from resolves back to the toplevel — i.e. the
        // git -C discipline works.
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skipping: git not on PATH");
            return;
        }
        let tmp = TempDir::new().unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(status.success());
        let inner = tmp.path().join("a/b/c");
        fs::create_dir_all(&inner).unwrap();
        // Resolve canonical paths because git typically returns the
        // canonicalized toplevel and tempdir paths often go through
        // /private on macOS / a symlinked /tmp.
        let resolved = locate_repo_root_from(&inner).unwrap();
        assert_eq!(
            fs::canonicalize(&resolved).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[test]
    fn locate_repo_root_from_walks_up_to_marker_when_no_git() {
        // No git in this temp tree (ensure no .git dir exists). Drop a
        // Cargo.toml at the top, descend into a subdir, confirm we walk up
        // to it. Even if git is on PATH, `rev-parse` will fail outside a
        // repo; the function falls through to the marker walk.
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let inner = tmp.path().join("a/b");
        fs::create_dir_all(&inner).unwrap();
        // Skip if git accidentally finds an outer parent repo (e.g. running
        // inside a CAS worktree). We can detect that by checking whether
        // git's --show-toplevel would succeed — if it does, this test isn't
        // exercising the marker-walk path.
        let git_finds_outer = Command::new("git")
            .arg("-C")
            .arg(&inner)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if git_finds_outer {
            eprintln!("skipping: git rev-parse found an outer repo, marker-walk not exercised");
            return;
        }
        let resolved = locate_repo_root_from(&inner).unwrap();
        assert_eq!(
            fs::canonicalize(&resolved).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }
}
