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
        scan_dir(
            &root,
            0,
            max_depth,
            &worktree_canon,
            &mut found,
            &mut budget,
        );
    }
    found
}

/// Scan the primary checkout's dependency tree for live links into a worker
/// worktree. pnpm's default virtual store is per-project, so its ordinary
/// install path does not cross-link worktrees. This guard instead protects the
/// non-default `virtualStoreDir`/global-store configuration or a manual
/// relink that makes package entries resolve through a disposable worktree.
/// Unlike the `$HOME` scan above, this root is deliberately JS-only: a
/// repository without a root `package.json` is untouched.
///
/// This is a guard, not a repair. A package-manager reinstall is the only
/// reliable way to reconstruct its chosen virtual-store layout; removing the
/// worktree first turns that recoverable mistake into a silent broken install.
pub fn scan_project_node_modules_symlinks_into(
    worktree_path: &Path,
    project_root: &Path,
) -> Vec<ExternalSymlink> {
    if !project_root.join("package.json").is_file() {
        return Vec::new();
    }
    let Ok(worktree_canon) = worktree_path.canonicalize() else {
        return Vec::new();
    };

    let mut found = Vec::new();
    let mut budget = MAX_SCAN_ENTRIES;
    scan_dir(
        &project_root.join("node_modules"),
        0,
        MAX_SCAN_DEPTH,
        &worktree_canon,
        &mut found,
        &mut budget,
    );
    found
}

/// A dangling link in the primary checkout's JavaScript dependency tree.
/// `target` is the resolved lexical destination (which intentionally need not
/// exist) so remediation can name the deleted worktree rather than merely the
/// link itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingNodeModulesSymlink {
    pub link: PathBuf,
    pub target: PathBuf,
}

/// Find broken symlinks beneath a primary checkout's `node_modules` tree.
///
/// The scan is bounded and skips non-JS repositories. It is intentionally a
/// report-only detector: `pnpm install --frozen-lockfile` (or the repository's
/// chosen package-manager equivalent) owns repair of its virtual store.
pub fn scan_dangling_node_modules_symlinks(project_root: &Path) -> Vec<DanglingNodeModulesSymlink> {
    if !project_root.join("package.json").is_file() {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut budget = MAX_SCAN_ENTRIES;
    scan_dangling_dir(
        &project_root.join("node_modules"),
        0,
        MAX_SCAN_DEPTH,
        &mut found,
        &mut budget,
    );
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

fn scan_dangling_dir(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    found: &mut Vec<DanglingNodeModulesSymlink>,
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
            if path.canonicalize().is_err() {
                let target = std::fs::read_link(&path)
                    .map(|target| {
                        if target.is_absolute() {
                            target
                        } else {
                            path.parent().unwrap_or(dir).join(target)
                        }
                    })
                    .unwrap_or_else(|_| PathBuf::from("<unreadable target>"));
                found.push(DanglingNodeModulesSymlink { link: path, target });
            }
            continue;
        }
        if meta.is_dir() && depth < max_depth {
            scan_dangling_dir(&path, depth + 1, max_depth, found, budget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use tempfile::TempDir;

    // `$HOME` is process-global, so every test here goes through the
    // crate-wide `test_support::with_temp_home` helper (not a module-local
    // mutex) — it's the single serialization point shared by every
    // HOME-mutating test in the crate (see lib.rs). A second, uncoordinated
    // lock here would race against e.g. worktree::discovery's HOME tests.

    #[test]
    fn no_symlinks_returns_empty() {
        TestEnvGuard::run_with_temp_home(|home| {
            let worktree = TempDir::new().unwrap();
            std::fs::write(home.join(".gitconfig"), "[user]\n").unwrap();

            let found = scan_external_symlinks_into(worktree.path());
            assert!(found.is_empty());
        });
    }

    #[test]
    fn direct_home_symlink_into_worktree_is_detected() {
        TestEnvGuard::run_with_temp_home(|home| {
            let worktree = TempDir::new().unwrap();
            let real_file = worktree.path().join("gitconfig");
            std::fs::write(&real_file, "[user]\nname = test\n").unwrap();

            let link = home.join(".gitconfig");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&real_file, &link).unwrap();

            let found = scan_external_symlinks_into(worktree.path());
            assert_eq!(found.len(), 1, "found: {found:?}");
            assert_eq!(found[0].link, link);
        });
    }

    #[test]
    fn nested_config_symlink_into_worktree_is_detected() {
        TestEnvGuard::run_with_temp_home(|home| {
            let worktree = TempDir::new().unwrap();
            let real_dir = worktree.path().join("systemd-units");
            std::fs::create_dir_all(&real_dir).unwrap();
            let real_file = real_dir.join("my-service.service");
            std::fs::write(&real_file, "[Unit]\n").unwrap();

            let unit_dir = home.join(".config/systemd/user");
            std::fs::create_dir_all(&unit_dir).unwrap();
            let link = unit_dir.join("my-service.service");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&real_file, &link).unwrap();

            let found = scan_external_symlinks_into(worktree.path());
            assert_eq!(found.len(), 1, "found: {found:?}");
            assert_eq!(found[0].link, link);
        });
    }

    #[test]
    fn symlink_pointing_elsewhere_is_not_flagged() {
        TestEnvGuard::run_with_temp_home(|home| {
            let worktree = TempDir::new().unwrap();
            let elsewhere = TempDir::new().unwrap();
            let real_file = elsewhere.path().join("unrelated.txt");
            std::fs::write(&real_file, "unrelated").unwrap();

            let link = home.join(".unrelated-link");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&real_file, &link).unwrap();

            let found = scan_external_symlinks_into(worktree.path());
            assert!(found.is_empty(), "found: {found:?}");
        });
    }

    #[test]
    fn dangling_symlink_is_not_flagged() {
        TestEnvGuard::run_with_temp_home(|home| {
            let worktree = TempDir::new().unwrap();

            let link = home.join(".dangling");
            #[cfg(unix)]
            std::os::unix::fs::symlink(worktree.path().join("gone"), &link).unwrap();

            let found = scan_external_symlinks_into(worktree.path());
            assert!(
                found.is_empty(),
                "a dangling link can't reference a live worktree: {found:?}"
            );
        });
    }

    #[test]
    fn main_checkout_node_modules_link_into_worktree_is_detected() {
        let project = TempDir::new().unwrap();
        let worktree = TempDir::new().unwrap();
        std::fs::write(project.path().join("package.json"), "{}").unwrap();
        let target = worktree
            .path()
            .join("node_modules/.pnpm/pkg@1/node_modules/pkg");
        std::fs::create_dir_all(&target).unwrap();
        let link = project.path().join("node_modules/pkg");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let found = scan_project_node_modules_symlinks_into(worktree.path(), project.path());
        assert_eq!(found.len(), 1, "found: {found:?}");
        assert_eq!(found[0].link, link);
    }

    #[test]
    fn dangling_node_modules_link_is_reported_but_non_js_project_is_ignored() {
        let project = TempDir::new().unwrap();
        let link = project.path().join("node_modules/pkg");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/deleted/worktree/node_modules/pkg", &link).unwrap();

        assert!(scan_dangling_node_modules_symlinks(project.path()).is_empty());
        std::fs::write(project.path().join("package.json"), "{}").unwrap();
        let found = scan_dangling_node_modules_symlinks(project.path());
        assert_eq!(found.len(), 1, "found: {found:?}");
        assert_eq!(found[0].link, link);
        assert!(
            found[0]
                .target
                .ends_with("deleted/worktree/node_modules/pkg")
        );
    }
}
