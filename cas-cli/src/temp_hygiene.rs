//! Scratch-root and `$TMPDIR` hygiene (GH #704).
//!
//! Two related jobs live here:
//!
//! 1. **Isolated-root placement.** A disposable Cassy root that lands on a
//!    memory-backed filesystem (`/tmp` is a 32 GB tmpfs on the operator host)
//!    spends RAM, not disk. A root that carries `worktrees/` or `build-cache/`
//!    is multi-GB by construction, so that combination is refused outright;
//!    any other memory-backed root warns. Disk-backed roots say nothing.
//! 2. **Stale temp-root inventory.** `gc_report` needs to name the leaked
//!    `cas-probe-comm-*` / `custom-wt-*` / `cas-*` directories under
//!    `temp_dir()` with age and size *before* the filesystem fills. The
//!    inventory never deletes anything.
//!
//! The mount lookup is injected through [`MountProbe`] so the guard is
//! unit-testable on hosts that have no tmpfs at all (macOS).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// One mount as reported by the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountInfo {
    pub mount_point: PathBuf,
    pub fstype: String,
}

impl MountInfo {
    pub fn new(mount_point: impl Into<PathBuf>, fstype: impl Into<String>) -> Self {
        Self {
            mount_point: mount_point.into(),
            fstype: fstype.into(),
        }
    }

    /// RAM-backed filesystems: bytes written here are resident memory.
    pub fn is_memory_backed(&self) -> bool {
        matches!(self.fstype.as_str(), "tmpfs" | "ramfs")
    }
}

/// Injected mount lookup. Implementations answer "which mount backs this
/// path", including for paths that do not exist yet.
pub trait MountProbe {
    fn mount_for(&self, path: &Path) -> Option<MountInfo>;
}

/// Real host lookup: Linux reads `/proc/mounts`; other platforms (macOS has
/// no tmpfs) report nothing, which downgrades every verdict to `Ok`.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostMountProbe;

impl MountProbe for HostMountProbe {
    fn mount_for(&self, path: &Path) -> Option<MountInfo> {
        #[cfg(target_os = "linux")]
        {
            let contents = std::fs::read_to_string("/proc/mounts").ok()?;
            mount_for_path(&parse_proc_mounts(&contents), path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            None
        }
    }
}

/// Parse `/proc/mounts`: `device mountpoint fstype options dump pass`, with
/// octal escapes (`\040` for space) in the mount point.
pub fn parse_proc_mounts(contents: &str) -> Vec<MountInfo> {
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let mount_point = fields.next()?;
            let fstype = fields.next()?;
            Some(MountInfo::new(unescape_mount_point(mount_point), fstype))
        })
        .collect()
}

fn unescape_mount_point(raw: &str) -> PathBuf {
    if !raw.contains('\\') {
        return PathBuf::from(raw);
    }
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'\\' && idx + 3 < bytes.len() {
            let octal = &raw[idx + 1..idx + 4];
            if let Ok(value) = u8::from_str_radix(octal, 8) {
                out.push(value as char);
                idx += 4;
                continue;
            }
        }
        out.push(bytes[idx] as char);
        idx += 1;
    }
    PathBuf::from(out)
}

/// Longest-prefix match of `path` against known mount points. Works on paths
/// that do not exist yet, which is the case the placement guard cares about.
pub fn mount_for_path(mounts: &[MountInfo], path: &Path) -> Option<MountInfo> {
    let candidate = normalize_for_prefix_match(path);
    mounts
        .iter()
        .filter(|mount| candidate.starts_with(&mount.mount_point))
        .max_by_key(|mount| mount.mount_point.components().count())
        .cloned()
}

fn normalize_for_prefix_match(path: &Path) -> PathBuf {
    // Canonicalize the deepest existing ancestor so symlinked temp dirs
    // (macOS `/tmp` -> `/private/tmp`) resolve to their real mount, then
    // re-attach the not-yet-created tail.
    let mut existing = path;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if existing.exists() {
            break;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
    let mut resolved = existing
        .canonicalize()
        .unwrap_or_else(|_| existing.to_path_buf());
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// Directories that make a Cassy root multi-GB by construction.
pub const BULK_ROOT_DIRS: &[&str] = &["worktrees", "build-cache"];

/// True when the root already carries (or is being pointed at as) a full
/// factory root: `worktrees/` and `build-cache/` are the 5 GB and 10 GB
/// offenders from the incident.
pub fn root_holds_bulk_dirs(root: &Path) -> bool {
    BULK_ROOT_DIRS.iter().any(|name| root.join(name).exists())
}

/// Placement verdict for an isolated Cassy root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScratchRootVerdict {
    /// Disk-backed, or the mount could not be identified: nothing to say.
    Allowed,
    /// Memory-backed but not a bulk root: proceed, loudly.
    Warn(String),
    /// Memory-backed *and* holding worktrees/ or build-cache/: refuse.
    Refuse(String),
}

impl ScratchRootVerdict {
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Allowed => None,
            Self::Warn(message) | Self::Refuse(message) => Some(message),
        }
    }
}

/// Decide whether an isolated root may live at `root`. `holds_bulk` is the
/// caller's declaration that this root does (or will) carry `worktrees/` or
/// `build-cache/`.
pub fn inspect_isolated_root(
    root: &Path,
    holds_bulk: bool,
    probe: &dyn MountProbe,
) -> ScratchRootVerdict {
    let Some(mount) = probe.mount_for(root) else {
        return ScratchRootVerdict::Allowed;
    };
    if !mount.is_memory_backed() {
        return ScratchRootVerdict::Allowed;
    }
    let where_ = format!(
        "{} is a memory-backed {} mount",
        mount.mount_point.display(),
        mount.fstype
    );
    if holds_bulk {
        ScratchRootVerdict::Refuse(format!(
            "refusing to place an isolated Cassy root at {root}: {where_}, and this root carries {bulk} (multi-GB). Use {scratch}/<name> or another disk-backed path.",
            root = root.display(),
            bulk = BULK_ROOT_DIRS.join("/ or "),
            scratch = scratch_root_base().display(),
        ))
    } else {
        ScratchRootVerdict::Warn(format!(
            "warning: isolated Cassy root {root} is on RAM ({where_}); it competes with process memory and every byte written is lost on reboot. Prefer {scratch}/<name>.",
            root = root.display(),
            scratch = scratch_root_base().display(),
        ))
    }
}

/// Apply [`inspect_isolated_root`] as a side-effecting guard: refusals become
/// errors, warnings go to stderr, and the warning text (if any) is returned so
/// callers can also surface it in structured output.
pub fn guard_isolated_root(
    root: &Path,
    probe: &dyn MountProbe,
) -> anyhow::Result<Option<String>> {
    match inspect_isolated_root(root, root_holds_bulk_dirs(root), probe) {
        ScratchRootVerdict::Allowed => Ok(None),
        ScratchRootVerdict::Warn(message) => {
            eprintln!("{message}");
            Ok(Some(message))
        }
        ScratchRootVerdict::Refuse(message) => anyhow::bail!(message),
    }
}

/// `~/.cas/scratch` — the disk-backed home for disposable roots.
pub fn scratch_root_base() -> PathBuf {
    crate::store::known_repos::host_cas_dir().join("scratch")
}

/// Default location for a named disposable root: `~/.cas/scratch/<name>`.
pub fn default_scratch_root(name: &str) -> PathBuf {
    scratch_root_base().join(name)
}

/// Directory-name prefixes that Cassy (or its tests) create under `temp_dir()`.
/// Order matters: the most specific prefix wins when labelling an entry.
pub const TEMP_ROOT_PREFIXES: &[&str] = &["cas-probe-comm-", "custom-wt-", "cas-"];

/// Default staleness threshold for the temp inventory.
pub const DEFAULT_TEMP_ROOT_STALE_SECS: u64 = 3_600;

/// Bound on directory entries walked while sizing, so a single 20 GB stray
/// root cannot make `gc_report` slow.
const SIZE_ENTRY_BUDGET: u64 = 200_000;
const PER_ROOT_ENTRY_BUDGET: u64 = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleTempRoot {
    pub path: PathBuf,
    pub prefix: &'static str,
    pub age: Duration,
    /// `None` when the global sizing budget ran out before this root.
    pub bytes: Option<u64>,
    /// True when the per-root entry budget stopped the walk early, making
    /// `bytes` a lower bound.
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TempRootInventory {
    pub dir: PathBuf,
    pub threshold: Duration,
    pub roots: Vec<StaleTempRoot>,
    pub total_bytes: u64,
    pub unsized_roots: usize,
    pub error: Option<String>,
}

impl TempRootInventory {
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.error.is_none()
    }

    /// Read-only rendering for `gc_report`. Never deletes, never suggests an
    /// automatic delete.
    pub fn render(&self) -> String {
        let mut out = format!(
            "\nStale Cassy temp roots under {} (older than {}s): {}\n",
            self.dir.display(),
            self.threshold.as_secs(),
            self.roots.len(),
        );
        if let Some(error) = &self.error {
            out.push_str(&format!("  - temp inventory unavailable: {error}\n"));
            return out;
        }
        if self.roots.is_empty() {
            return out;
        }
        out.push_str(&format!(
            "  Measured bytes: {}{}\n",
            self.total_bytes,
            if self.unsized_roots > 0 {
                format!(" (+{} root(s) not sized; walk budget)", self.unsized_roots)
            } else {
                String::new()
            }
        ));
        for root in self.roots.iter().take(10) {
            out.push_str(&format!(
                "  - {} (prefix {}, age {}s, {})\n",
                root.path.display(),
                root.prefix,
                root.age.as_secs(),
                match root.bytes {
                    Some(bytes) if root.truncated => format!(">={bytes} bytes"),
                    Some(bytes) => format!("{bytes} bytes"),
                    None => "size not measured".to_string(),
                },
            ));
        }
        if self.roots.len() > 10 {
            out.push_str(&format!(
                "  - ... and {} more\n",
                self.roots.len() - 10
            ));
        }
        out.push_str(
            "  Never auto-deleted. Inspect, then remove manually; a filled $TMPDIR breaks every live session's shell output.\n",
        );
        out
    }
}

/// Inventory stale Cassy-shaped directories under `dir`. `now` is injected so
/// aged fixtures need no mtime surgery.
pub fn scan_stale_temp_roots(dir: &Path, older_than: Duration, now: SystemTime) -> TempRootInventory {
    let mut inventory = TempRootInventory {
        dir: dir.to_path_buf(),
        threshold: older_than,
        ..Default::default()
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return inventory,
        Err(error) => {
            inventory.error = Some(error.to_string());
            return inventory;
        }
    };

    let mut stale: Vec<(PathBuf, &'static str, Duration)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(prefix) = TEMP_ROOT_PREFIXES
            .iter()
            .find(|prefix| name.starts_with(**prefix))
        else {
            continue;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(age) = entry_age(&path, now) else {
            continue;
        };
        if age < older_than {
            continue;
        }
        stale.push((path, prefix, age));
    }

    // Oldest first: those are the ones an operator should look at, and they
    // get first claim on the sizing budget.
    stale.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));

    let mut budget = SIZE_ENTRY_BUDGET;
    for (path, prefix, age) in stale {
        let (bytes, truncated) = if budget == 0 {
            (None, false)
        } else {
            let (bytes, entries_walked, truncated) =
                bounded_dir_size(&path, PER_ROOT_ENTRY_BUDGET.min(budget));
            budget = budget.saturating_sub(entries_walked);
            (Some(bytes), truncated)
        };
        if let Some(bytes) = bytes {
            inventory.total_bytes = inventory.total_bytes.saturating_add(bytes);
        } else {
            inventory.unsized_roots += 1;
        }
        inventory.roots.push(StaleTempRoot {
            path,
            prefix,
            age,
            bytes,
            truncated,
        });
    }

    // Biggest first for the rendered head; the operator wants the offender.
    inventory.roots.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| right.age.cmp(&left.age))
            .then_with(|| left.path.cmp(&right.path))
    });
    inventory
}

fn entry_age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    Some(now.duration_since(modified).unwrap_or_default())
}

/// Recursive size with an entry budget. Returns `(bytes, entries_walked,
/// truncated)`; `truncated` means the budget stopped the walk, so `bytes` is a
/// lower bound. Symlinks are counted by their own size, never followed.
fn bounded_dir_size(root: &Path, budget: u64) -> (u64, u64, bool) {
    let mut bytes = 0u64;
    let mut walked = 0u64;
    let mut truncated = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if walked >= budget {
                truncated = true;
                return (bytes, walked, truncated);
            }
            walked += 1;
            let Ok(metadata) = entry.metadata().or_else(|_| entry.path().symlink_metadata())
            else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    (bytes, walked, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeMounts(Vec<MountInfo>);

    impl MountProbe for FakeMounts {
        fn mount_for(&self, path: &Path) -> Option<MountInfo> {
            mount_for_path(&self.0, path)
        }
    }

    fn tmpfs_host() -> FakeMounts {
        FakeMounts(vec![
            MountInfo::new("/", "ext4"),
            MountInfo::new("/tmp", "tmpfs"),
            MountInfo::new("/home", "ext4"),
        ])
    }

    #[test]
    fn proc_mounts_parse_yields_mount_point_and_fstype() {
        let mounts = parse_proc_mounts(
            "/dev/nvme0n1p2 / ext4 rw,relatime 0 0\ntmpfs /tmp tmpfs rw,nosuid,nodev 0 0\ntmpfs /run/user/1000 tmpfs rw 0 0\nbad-line\n",
        );
        assert_eq!(mounts.len(), 3);
        assert_eq!(mounts[1], MountInfo::new("/tmp", "tmpfs"));
        assert!(mounts[1].is_memory_backed());
        assert!(!mounts[0].is_memory_backed());
    }

    #[test]
    fn proc_mounts_unescape_spaces_in_mount_points() {
        let mounts = parse_proc_mounts("tmpfs /mnt/my\\040disk tmpfs rw 0 0\n");
        assert_eq!(mounts[0].mount_point, PathBuf::from("/mnt/my disk"));
    }

    #[test]
    fn longest_prefix_wins_for_nested_mounts() {
        let mounts = vec![
            MountInfo::new("/", "ext4"),
            MountInfo::new("/run/user/1000", "tmpfs"),
        ];
        let mount = mount_for_path(&mounts, Path::new("/run/user/1000/cas-scratch/.cas"))
            .expect("nested mount should match");
        assert_eq!(mount.fstype, "tmpfs");
        assert_eq!(
            mount_for_path(&mounts, Path::new("/home/x/.cas"))
                .expect("root mount")
                .fstype,
            "ext4"
        );
    }

    #[test]
    fn full_root_on_tmpfs_is_refused_and_names_the_mount() {
        let verdict = inspect_isolated_root(
            Path::new("/tmp/cas700-store"),
            true, // carries worktrees/ + build-cache/
            &tmpfs_host(),
        );
        let ScratchRootVerdict::Refuse(message) = &verdict else {
            panic!("a bulk root on tmpfs must be refused, got {verdict:?}");
        };
        assert!(message.contains("/tmp"), "must name the mount: {message}");
        assert!(message.contains("tmpfs"), "must name the fstype: {message}");
        assert!(
            message.contains("worktrees"),
            "must say why it is refused: {message}"
        );
    }

    #[test]
    fn small_root_on_tmpfs_warns_instead_of_refusing() {
        let verdict = inspect_isolated_root(
            Path::new("/tmp/cas-probe-comm-abc/.cas"),
            false,
            &tmpfs_host(),
        );
        let ScratchRootVerdict::Warn(message) = &verdict else {
            panic!("a small tmpfs root must warn, got {verdict:?}");
        };
        assert!(message.contains("/tmp"), "must name the mount: {message}");
        assert!(
            message.contains("scratch"),
            "must point at the disk-backed default: {message}"
        );
    }

    #[test]
    fn disk_backed_root_is_allowed_even_when_it_holds_bulk_dirs() {
        assert_eq!(
            inspect_isolated_root(Path::new("/home/x/.cas"), true, &tmpfs_host()),
            ScratchRootVerdict::Allowed
        );
    }

    #[test]
    fn unknown_mount_is_allowed_so_macos_never_blocks() {
        struct NoMounts;
        impl MountProbe for NoMounts {
            fn mount_for(&self, _path: &Path) -> Option<MountInfo> {
                None
            }
        }
        assert_eq!(
            inspect_isolated_root(Path::new("/private/tmp/whatever"), true, &NoMounts),
            ScratchRootVerdict::Allowed
        );
    }

    #[test]
    fn bulk_dir_detection_reads_the_root_contents() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!root_holds_bulk_dirs(temp.path()));
        std::fs::create_dir_all(temp.path().join("build-cache")).unwrap();
        assert!(root_holds_bulk_dirs(temp.path()));
    }

    #[test]
    fn default_scratch_root_is_under_home_cas_scratch() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            assert_eq!(
                default_scratch_root("cas-probe-comm-1"),
                home.join(".cas").join("scratch").join("cas-probe-comm-1")
            );
        });
    }

    #[test]
    fn stale_scan_lists_aged_cas_roots_with_age_and_size_and_deletes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let probe = temp.path().join("cas-probe-comm-1234");
        std::fs::create_dir_all(probe.join(".cas")).unwrap();
        std::fs::write(probe.join(".cas").join("cas.db"), vec![7u8; 512]).unwrap();
        let worktree_fixture = temp.path().join("custom-wt-99");
        std::fs::create_dir_all(&worktree_fixture).unwrap();
        std::fs::create_dir_all(temp.path().join("unrelated-thing")).unwrap();

        let now = SystemTime::now() + Duration::from_secs(7_200);
        let inventory = scan_stale_temp_roots(temp.path(), Duration::from_secs(3_600), now);

        assert_eq!(inventory.roots.len(), 2, "{inventory:?}");
        let probe_entry = inventory
            .roots
            .iter()
            .find(|root| root.path == probe)
            .expect("probe root should be listed");
        assert_eq!(probe_entry.prefix, "cas-probe-comm-");
        assert_eq!(probe_entry.bytes, Some(512));
        assert!(probe_entry.age >= Duration::from_secs(7_200));
        assert!(
            inventory
                .roots
                .iter()
                .any(|root| root.path == worktree_fixture && root.prefix == "custom-wt-")
        );
        assert!(
            probe.exists() && worktree_fixture.exists(),
            "the inventory must never delete"
        );

        let rendered = inventory.render();
        assert!(rendered.contains("Stale Cassy temp roots"), "{rendered}");
        assert!(rendered.contains("cas-probe-comm-1234"), "{rendered}");
        assert!(rendered.contains("512 bytes"), "{rendered}");
        assert!(rendered.contains("Never auto-deleted"), "{rendered}");
    }

    #[test]
    fn fresh_roots_are_not_reported_as_stale() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("cas-probe-comm-new")).unwrap();

        let inventory =
            scan_stale_temp_roots(temp.path(), Duration::from_secs(3_600), SystemTime::now());
        assert!(inventory.roots.is_empty(), "{inventory:?}");
        assert!(inventory.render().contains(": 0"), "{inventory:?}");
    }

    #[test]
    fn missing_temp_dir_is_an_empty_inventory_not_an_error() {
        let inventory = scan_stale_temp_roots(
            Path::new("/nonexistent-temp-root-cas-cb5e"),
            Duration::from_secs(60),
            SystemTime::now(),
        );
        assert!(inventory.error.is_none());
        assert!(inventory.roots.is_empty());
    }
}
