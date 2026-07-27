//! Detect live external symlinks that resolve into a worktree about to be
//! removed (cas-df97).
//!
//! Real incident this guards against: a factory worker ran a dotfile
//! stow/install step *inside* its isolated worktree, which repointed ~21
//! live `$HOME` symlinks (`.gitconfig`, `.ssh/config`, `~/bin/*`, systemd
//! user units, Konsole profile, starship, ...) into the worktree. The
//! worktree was later removed as routine cleanup, leaving every one of
//! those symlinks dangling. Nothing visibly broke until the next reboot,
//! when every app re-read its config and found it gone.
//!
//! The failure is silent by construction: the symlinks stay valid right up
//! until the worktree directory is actually removed, and the breakage only
//! surfaces later, arbitrarily far from the cause. Detecting it at removal
//! time — the one point where it's cheap to catch — requires knowing
//! whether anything *outside* the worktree still points *into* it.
//!
//! This is deliberately the "adequate" scan named in the bug report, not
//! the thorough alternative (a repo-wide symlink registry maintained at
//! link-creation time): it walks a handful of well-known roots where a
//! stow/install step is likely to have dropped a symlink, bounded in depth
//! and entry count so a large `~/.config` tree can't make worktree removal
//! hang.

use std::path::{Path, PathBuf};

/// Maximum directory depth to descend under each scanned root.
const MAX_SCAN_DEPTH: usize = 6;

/// Hard cap on filesystem entries visited across all roots combined — a
/// second, independent bound alongside depth against pathological trees.
const MAX_SCAN_ENTRIES: usize = 50_000;

/// A live external symlink found pointing into a worktree slated for
/// removal. Removing the worktree would leave `link` dangling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSymlink {
    /// The symlink's own path (outside the worktree).
    pub link: PathBuf,
    /// The symlink's resolved (canonical) target, inside the worktree.
    pub target: PathBuf,
}

/// Scan well-known external roots for symlinks resolving into
/// `worktree_path`, returning every one found.
///
/// Roots scanned: `$HOME` (direct entries only — traditional dotfiles like
/// `.gitconfig`, `.vimrc`, `.bash_profile` live here), `~/.config`
/// (recursively — covers app configs and systemd user units under
/// `~/.config/systemd/user`), `~/.local/share` (recursively — e.g. Konsole
/// profiles), `~/bin`, and `~/.ssh`.
///
/// Best-effort: unreadable directories are skipped rather than failing the
/// whole scan, and `$HOME` unset (no sane environment) returns empty.
pub fn scan_external_symlinks_into(worktree_path: &Path) -> Vec<ExternalSymlink> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let Ok(worktree_canon) = worktree_path.canonicalize() else {
        return Vec::new();
    };

    let roots: [(PathBuf, usize); 5] = [
        (home.clone(), 1),
        (home.join(".config"), MAX_SCAN_DEPTH),
        (home.join(".local/share"), MAX_SCAN_DEPTH),
        (home.join("bin"), MAX_SCAN_DEPTH),
        (home.join(".ssh"), MAX_SCAN_DEPTH),
    ];

    let mut found = Vec::new();
    let mut budget = MAX_SCAN_ENTRIES;
    for (root, max_depth) in roots {
        scan_dir(&root, 0, max_depth, &worktree_canon, &mut found, &mut budget);
    }
    found
}

fn scan_dir(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    worktree_canon: &Path,
    found: &mut Vec<ExternalSymlink>,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;

        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };

        if meta.file_type().is_symlink() {
            // Resolve fully (not just read the immediate link target) so a
            // chain of symlinks that eventually lands inside the worktree
            // is still caught. Skip anything that doesn't resolve — a
            // dangling link can't reference a live worktree.
            if let Ok(resolved) = path.canonicalize() {
                if resolved == *worktree_canon || resolved.starts_with(worktree_canon) {
                    found.push(ExternalSymlink {
                        link: path.clone(),
                        target: resolved,
                    });
                }
            }
            // Never descend into a symlinked directory: it isn't part of
            // this filesystem subtree and could itself point somewhere
            // that loops back (e.g. into the worktree indirectly).
            continue;
        }

        if meta.is_dir() && depth < max_depth {
            scan_dir(&path, depth + 1, max_depth, worktree_canon, found, budget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Point `$HOME` at a scratch dir for the duration of `f`, restoring the
    /// previous value afterward. Tests in this module must not run
    /// concurrently with each other (they mutate process-global env) —
    /// `cargo test` runs tests in a module single-file-ish but across
    /// threads by default, so each test creates and clears its own $HOME
    /// scoped to a serial guard via a per-test mutex.
    fn with_scoped_home<F: FnOnce(&Path)>(home: &Path, f: F) {
        static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = HOME_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home);
        }
        f(home);
        unsafe {
            match prior {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn no_symlinks_returns_empty() {
        let home = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();
        std::fs::write(home.path().join(".gitconfig"), "[user]\n").unwrap();

        with_scoped_home(home.path(), |_| {
            let found = scan_external_symlinks_into(worktree.path());
            assert!(found.is_empty());
        });
    }

    #[test]
    fn direct_home_symlink_into_worktree_is_detected() {
        let home = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();
        let real_file = worktree.path().join("gitconfig");
        std::fs::write(&real_file, "[user]\nname = test\n").unwrap();

        let link = home.path().join(".gitconfig");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        with_scoped_home(home.path(), |_| {
            let found = scan_external_symlinks_into(worktree.path());
            assert_eq!(found.len(), 1, "found: {found:?}");
            assert_eq!(found[0].link, link);
        });
    }

    #[test]
    fn nested_config_symlink_into_worktree_is_detected() {
        let home = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();
        let real_dir = worktree.path().join("systemd-units");
        std::fs::create_dir_all(&real_dir).unwrap();
        let real_file = real_dir.join("my-service.service");
        std::fs::write(&real_file, "[Unit]\n").unwrap();

        let unit_dir = home.path().join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir).unwrap();
        let link = unit_dir.join("my-service.service");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        with_scoped_home(home.path(), |_| {
            let found = scan_external_symlinks_into(worktree.path());
            assert_eq!(found.len(), 1, "found: {found:?}");
            assert_eq!(found[0].link, link);
        });
    }

    #[test]
    fn symlink_pointing_elsewhere_is_not_flagged() {
        let home = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let real_file = elsewhere.path().join("unrelated.txt");
        std::fs::write(&real_file, "unrelated").unwrap();

        let link = home.path().join(".unrelated-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        with_scoped_home(home.path(), |_| {
            let found = scan_external_symlinks_into(worktree.path());
            assert!(found.is_empty(), "found: {found:?}");
        });
    }

    #[test]
    fn dangling_symlink_is_not_flagged() {
        let home = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();

        let link = home.path().join(".dangling");
        #[cfg(unix)]
        std::os::unix::fs::symlink(worktree.path().join("gone"), &link).unwrap();

        with_scoped_home(home.path(), |_| {
            let found = scan_external_symlinks_into(worktree.path());
            assert!(found.is_empty(), "a dangling link can't reference a live worktree: {found:?}");
        });
    }
}
