//! Guarded Cargo target-cache pressure reporting and reclamation.
//!
//! Factory worktrees intentionally keep independent Cargo target directories:
//! sharing one target directory across concurrently-mutating branches is not a
//! correctness boundary CAS can currently prove. This module therefore
//! reclaims only regenerable `target/` trees belonging to known factory
//! worktrees, and only after conservative liveness, recency, containment, and
//! filesystem-watermark checks.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::config::FactoryConfig;

const QUARANTINE_PREFIX: &str = ".cas-target-gc-";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetCachePolicy {
    pub high_watermark_percent: u8,
    pub low_watermark_percent: u8,
    pub min_idle_secs: u64,
    pub retention_count: usize,
}

impl From<&FactoryConfig> for TargetCachePolicy {
    fn from(config: &FactoryConfig) -> Self {
        let high = config.target_cache_high_watermark_percent.clamp(1, 100);
        let low = config
            .target_cache_low_watermark_percent
            .min(high.saturating_sub(1));
        Self {
            high_watermark_percent: high,
            low_watermark_percent: low,
            min_idle_secs: config.target_cache_min_idle_secs,
            retention_count: config.target_cache_retention_count,
        }
    }
}

impl From<FactoryConfig> for TargetCachePolicy {
    fn from(config: FactoryConfig) -> Self {
        Self::from(&config)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheDisposition {
    LiveProcess,
    RecentWrite,
    Retained,
    Eligible,
    Selected,
    UnsafePath,
    Reclaimed,
    CleanupError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetCacheRecord {
    pub path: PathBuf,
    pub worktree: PathBuf,
    pub bytes: u64,
    pub newest_write_unix_secs: Option<u64>,
    pub disposition: CacheDisposition,
    pub reason: String,
    pub interrupted_cleanup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilesystemCapacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: u8,
    pub high_watermark_percent: u8,
    pub low_watermark_percent: u8,
    pub pressure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetCacheReport {
    pub schema_version: u32,
    pub filesystem: FilesystemCapacity,
    pub candidate_bytes: u64,
    pub selected_bytes: u64,
    pub reclaimed_bytes: u64,
    pub dry_run: bool,
    pub remediation: String,
    pub caches: Vec<TargetCacheRecord>,
}

impl TargetCacheReport {
    pub fn machine_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|error| format!(r#"{{"schema_version":1,"error":{error:?}}}"#))
    }
}

#[derive(Debug, Clone)]
struct ScannedCache {
    path: PathBuf,
    worktree: PathBuf,
    bytes: u64,
    newest_write: Option<SystemTime>,
    interrupted_cleanup: bool,
    unsafe_reason: Option<String>,
}

/// Fast filesystem-only pressure probe used before a factory starts workers.
pub fn capacity_status(
    repo_root: &Path,
    policy: TargetCachePolicy,
) -> io::Result<FilesystemCapacity> {
    let (total_bytes, available_bytes) = filesystem_capacity(repo_root)?;
    Ok(capacity_from_bytes(total_bytes, available_bytes, policy))
}

/// Inspect every root/factory-worktree Cargo cache without mutating it.
pub fn inspect(
    cas_root: &Path,
    policy: TargetCachePolicy,
    known_worktree_roots: &[PathBuf],
    live_worktree_roots: &[PathBuf],
    dry_run: bool,
) -> io::Result<TargetCacheReport> {
    let repo_root = cas_root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CAS root has no repository parent",
        )
    })?;
    let capacity = capacity_status(repo_root, policy)?;
    let now = SystemTime::now();
    let mut scanned = discover_caches(repo_root, cas_root, known_worktree_roots);
    let canonical_live_roots: HashSet<PathBuf> = live_worktree_roots
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect();

    let mut records = Vec::with_capacity(scanned.len());
    for cache in scanned.drain(..) {
        let newest_write_unix_secs = cache
            .newest_write
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|age| age.as_secs());
        let (disposition, reason) = if let Some(reason) = cache.unsafe_reason {
            (CacheDisposition::UnsafePath, reason)
        } else if canonical_live_roots.contains(&cache.worktree)
            || live_process_uses(&cache.worktree, &cache.path)
        {
            (
                CacheDisposition::LiveProcess,
                "registered or OS-visible live process uses this worktree".to_string(),
            )
        } else if cache.newest_write.is_some_and(|modified| {
            now.duration_since(modified).unwrap_or_default()
                < Duration::from_secs(policy.min_idle_secs)
        }) {
            (
                CacheDisposition::RecentWrite,
                format!("newest write is younger than {}s", policy.min_idle_secs),
            )
        } else {
            (
                CacheDisposition::Eligible,
                "stale regenerable Cargo target cache".to_string(),
            )
        };
        records.push(TargetCacheRecord {
            path: cache.path,
            worktree: cache.worktree,
            bytes: cache.bytes,
            newest_write_unix_secs,
            disposition,
            reason,
            interrupted_cleanup: cache.interrupted_cleanup,
        });
    }

    plan_records(&mut records, &capacity, policy);
    let candidate_bytes = records.iter().map(|record| record.bytes).sum();
    let selected_bytes = records
        .iter()
        .filter(|record| record.disposition == CacheDisposition::Selected)
        .map(|record| record.bytes)
        .sum();
    Ok(TargetCacheReport {
        schema_version: 1,
        filesystem: capacity,
        candidate_bytes,
        selected_bytes,
        reclaimed_bytes: 0,
        dry_run,
        remediation: "Review gc_report, then run gc_cleanup force=true dry_run=false; CAS revalidates liveness, recency, and path containment immediately before each rename.".to_string(),
        caches: records,
    })
}

/// Reclaim the report's selected caches. Each normal `target/` is first
/// atomically renamed to a sibling quarantine directory. An interrupted
/// recursive delete therefore cannot expose a half-deleted target tree to a
/// later Cargo invocation, and the next report rediscovers the quarantine.
pub fn cleanup_selected(
    cas_root: &Path,
    report: &mut TargetCacheReport,
    policy: TargetCachePolicy,
    live_worktree_roots: &[PathBuf],
) -> io::Result<()> {
    if report.dry_run {
        return Ok(());
    }
    let lock_path = cas_root.join("target-cache-gc.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive().map_err(|error| {
        io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("another target-cache cleanup owns the GC lock: {error}"),
        )
    })?;

    let canonical_live_roots: HashSet<PathBuf> = live_worktree_roots
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect();
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(policy.min_idle_secs))
        .unwrap_or(UNIX_EPOCH);
    let mut ordinal = 0u64;

    for record in report
        .caches
        .iter_mut()
        .filter(|record| record.disposition == CacheDisposition::Selected)
    {
        let rescanned = scan_cache(&record.worktree, &record.path, record.interrupted_cleanup);
        let Ok(rescanned) = rescanned else {
            record.disposition = CacheDisposition::CleanupError;
            record.reason = "cache vanished or became unreadable during cleanup".to_string();
            continue;
        };
        if let Some(reason) = rescanned.unsafe_reason {
            record.disposition = CacheDisposition::UnsafePath;
            record.reason = reason;
            continue;
        }
        if canonical_live_roots.contains(&rescanned.worktree)
            || live_process_uses(&rescanned.worktree, &rescanned.path)
        {
            record.disposition = CacheDisposition::LiveProcess;
            record.reason = "liveness appeared during destructive revalidation".to_string();
            continue;
        }
        if rescanned
            .newest_write
            .is_some_and(|modified| modified > cutoff)
        {
            record.disposition = CacheDisposition::RecentWrite;
            record.reason = "cache received a write during destructive revalidation".to_string();
            continue;
        }

        ordinal += 1;
        let quarantine = if record.interrupted_cleanup {
            record.path.clone()
        } else {
            let path = record.worktree.join(format!(
                "{QUARANTINE_PREFIX}{}-{ordinal}",
                std::process::id()
            ));
            if let Err(error) = fs::rename(&record.path, &path) {
                record.disposition = CacheDisposition::CleanupError;
                record.reason = format!("atomic quarantine rename failed: {error}");
                continue;
            }
            path
        };

        // Close the check→rename window once more. Open file descriptors now
        // resolve through the quarantine path, while a Cargo process that
        // started against the worktree root is visible by cwd/cmdline. If a
        // live user appeared, restore the original name when possible and
        // leave the cache intact either way.
        if live_process_uses(&record.worktree, &quarantine) {
            if !record.interrupted_cleanup && !record.path.exists() {
                let _ = fs::rename(&quarantine, &record.path);
            }
            record.disposition = CacheDisposition::LiveProcess;
            record.reason = "liveness appeared after atomic quarantine rename".to_string();
            continue;
        }

        match fs::remove_dir_all(&quarantine) {
            Ok(()) => {
                record.disposition = CacheDisposition::Reclaimed;
                record.reason = "reclaimed regenerable Cargo artifacts".to_string();
                report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(record.bytes);
            }
            Err(error) => {
                record.path = quarantine;
                record.interrupted_cleanup = true;
                record.disposition = CacheDisposition::CleanupError;
                record.reason = format!("quarantined; recursive cleanup interrupted: {error}");
            }
        }
    }
    Ok(())
}

fn plan_records(
    records: &mut [TargetCacheRecord],
    capacity: &FilesystemCapacity,
    policy: TargetCachePolicy,
) {
    let mut eligible: Vec<usize> = records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            (record.disposition == CacheDisposition::Eligible).then_some(index)
        })
        .collect();
    eligible.sort_by_key(|index| std::cmp::Reverse(records[*index].newest_write_unix_secs));
    let retained: Vec<usize> = eligible
        .iter()
        .filter(|index| !records[**index].interrupted_cleanup)
        .take(policy.retention_count)
        .copied()
        .collect();
    for index in retained {
        records[index].disposition = CacheDisposition::Retained;
        records[index].reason = "retained as one of the newest warm caches".to_string();
    }
    if !capacity.pressure {
        return;
    }

    let target_available = capacity
        .total_bytes
        .saturating_mul(100u64.saturating_sub(policy.low_watermark_percent as u64))
        / 100;
    let mut bytes_needed = target_available.saturating_sub(capacity.available_bytes);
    eligible.sort_by_key(|index| records[*index].newest_write_unix_secs);
    for index in eligible {
        if bytes_needed == 0 || records[index].disposition != CacheDisposition::Eligible {
            continue;
        }
        records[index].disposition = CacheDisposition::Selected;
        records[index].reason = "selected oldest-first to reach the low watermark".to_string();
        bytes_needed = bytes_needed.saturating_sub(records[index].bytes);
    }
}

fn discover_caches(
    repo_root: &Path,
    cas_root: &Path,
    known_worktree_roots: &[PathBuf],
) -> Vec<ScannedCache> {
    let mut roots = vec![repo_root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(cas_root.join("worktrees")) {
        roots.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir()),
        );
    }
    roots.extend(
        known_worktree_roots
            .iter()
            .filter(|path| path.exists())
            .cloned(),
    );
    roots.sort();
    roots.dedup();
    let mut caches = Vec::new();
    for root in roots {
        let target = root.join("target");
        if fs::symlink_metadata(&target).is_ok() {
            caches.push(
                scan_cache(&root, &target, false)
                    .unwrap_or_else(|error| scan_error_record(root.clone(), target, false, error)),
            );
        }
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(QUARANTINE_PREFIX)
                {
                    let path = entry.path();
                    caches.push(scan_cache(&root, &path, true).unwrap_or_else(|error| {
                        scan_error_record(root.clone(), path, true, error)
                    }));
                }
            }
        }
    }
    caches
}

fn scan_error_record(
    worktree: PathBuf,
    path: PathBuf,
    interrupted_cleanup: bool,
    error: io::Error,
) -> ScannedCache {
    ScannedCache {
        path,
        worktree,
        bytes: 0,
        newest_write: None,
        interrupted_cleanup,
        unsafe_reason: Some(format!("scan failed closed: {error}")),
    }
}

fn scan_cache(worktree: &Path, path: &Path, interrupted_cleanup: bool) -> io::Result<ScannedCache> {
    let worktree_metadata = fs::symlink_metadata(worktree)?;
    if worktree_metadata.file_type().is_symlink() || !worktree_metadata.is_dir() {
        return Ok(ScannedCache {
            path: path.to_path_buf(),
            worktree: worktree.to_path_buf(),
            bytes: 0,
            newest_write: None,
            interrupted_cleanup,
            unsafe_reason: Some("worktree root is a symlink or not a directory".to_string()),
        });
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(ScannedCache {
            path: path.to_path_buf(),
            worktree: worktree.to_path_buf(),
            bytes: 0,
            newest_write: None,
            interrupted_cleanup,
            unsafe_reason: Some("cache root is a symlink or not a directory".to_string()),
        });
    }
    let canonical_worktree = worktree.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    let allowed_name = path.file_name().is_some_and(|name| {
        name == "target" || name.to_string_lossy().starts_with(QUARANTINE_PREFIX)
    });
    if !allowed_name || canonical_path.parent() != Some(canonical_worktree.as_path()) {
        return Ok(ScannedCache {
            path: path.to_path_buf(),
            worktree: canonical_worktree,
            bytes: 0,
            newest_write: None,
            interrupted_cleanup,
            unsafe_reason: Some("cache path escaped its exact worktree parent".to_string()),
        });
    }

    let mut bytes = 0u64;
    let mut newest_write = metadata.modified().ok();
    for entry in WalkDir::new(&canonical_path).follow_links(false) {
        let entry = entry.map_err(io::Error::other)?;
        let metadata = entry.metadata().map_err(io::Error::other)?;
        if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
        }
        if let Ok(modified) = metadata.modified() {
            newest_write = Some(newest_write.map_or(modified, |current| current.max(modified)));
        }
    }
    Ok(ScannedCache {
        path: canonical_path,
        worktree: canonical_worktree,
        bytes,
        newest_write,
        interrupted_cleanup,
        unsafe_reason: None,
    })
}

fn live_process_uses(worktree: &Path, cache: &Path) -> bool {
    let Ok(processes) = fs::read_dir("/proc") else {
        // On platforms without a process table, fail closed: target-cache GC
        // remains report-only instead of guessing that a cache is idle.
        return true;
    };
    let worktree_text = worktree.as_os_str().to_string_lossy();
    for process in processes.flatten().filter(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    }) {
        let proc_path = process.path();
        if fs::read_link(proc_path.join("cwd"))
            .ok()
            .is_some_and(|cwd| cwd.starts_with(worktree))
        {
            return true;
        }
        if let Ok(cmdline) = fs::read(proc_path.join("cmdline")) {
            if cmdline
                .split(|byte| *byte == 0)
                .filter_map(|arg| std::str::from_utf8(arg).ok())
                .any(|arg| arg.contains(worktree_text.as_ref()))
            {
                return true;
            }
        }
        if let Ok(fds) = fs::read_dir(proc_path.join("fd")) {
            if fds
                .flatten()
                .filter_map(|fd| fs::read_link(fd.path()).ok())
                .any(|path| path.starts_with(cache))
            {
                return true;
            }
        }
    }
    false
}

fn capacity_from_bytes(
    total_bytes: u64,
    available_bytes: u64,
    policy: TargetCachePolicy,
) -> FilesystemCapacity {
    let used = total_bytes.saturating_sub(available_bytes);
    let used_percent = if total_bytes == 0 {
        100
    } else {
        (((used as u128) * 100 + total_bytes as u128 - 1) / total_bytes as u128).min(100) as u8
    };
    FilesystemCapacity {
        total_bytes,
        available_bytes,
        used_percent,
        high_watermark_percent: policy.high_watermark_percent,
        low_watermark_percent: policy.low_watermark_percent,
        pressure: used_percent >= policy.high_watermark_percent,
    }
}

#[cfg(unix)]
fn filesystem_capacity(path: &Path) -> io::Result<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stat` points to writable storage.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs returned success and initialized the structure.
    let stat = unsafe { stat.assume_init() };
    // Widen before multiplying: statvfs field widths are platform-specific.
    // Linux types f_blocks/f_bavail as u64, macOS as u32 (__darwin_fsblkcnt_t)
    // while f_frsize stays u64, so the unwidened form only compiles on Linux.
    // These casts are load-bearing on macOS and no-ops on Linux.
    let frsize = stat.f_frsize as u64;
    Ok((
        (stat.f_blocks as u64).saturating_mul(frsize),
        (stat.f_bavail as u64).saturating_mul(frsize),
    ))
}

#[cfg(not(unix))]
fn filesystem_capacity(_path: &Path) -> io::Result<(u64, u64)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "target-cache capacity reporting requires statvfs",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> TargetCachePolicy {
        TargetCachePolicy {
            high_watermark_percent: 80,
            low_watermark_percent: 60,
            min_idle_secs: 60,
            retention_count: 1,
        }
    }

    fn record(name: &str, bytes: u64, newest: u64) -> TargetCacheRecord {
        TargetCacheRecord {
            path: PathBuf::from(format!("/repo/.cas/worktrees/{name}/target")),
            worktree: PathBuf::from(format!("/repo/.cas/worktrees/{name}")),
            bytes,
            newest_write_unix_secs: Some(newest),
            disposition: CacheDisposition::Eligible,
            reason: String::new(),
            interrupted_cleanup: false,
        }
    }

    #[test]
    fn watermark_and_retention_select_oldest_until_low_watermark() {
        let capacity = capacity_from_bytes(1_000, 100, policy());
        let mut records = vec![record("old", 250, 10), record("new", 300, 20)];
        plan_records(&mut records, &capacity, policy());
        assert_eq!(records[0].disposition, CacheDisposition::Selected);
        assert_eq!(records[1].disposition, CacheDisposition::Retained);
    }

    #[test]
    fn no_pressure_never_selects_an_eligible_cache() {
        let capacity = capacity_from_bytes(1_000, 500, policy());
        let mut records = vec![record("old", 400, 10)];
        plan_records(
            &mut records,
            &capacity,
            TargetCachePolicy {
                retention_count: 0,
                ..policy()
            },
        );
        assert_eq!(records[0].disposition, CacheDisposition::Eligible);
    }

    #[test]
    fn capacity_probe_crosses_high_watermark_before_enospc() {
        let threshold_policy = TargetCachePolicy {
            high_watermark_percent: 85,
            ..policy()
        };
        let below = capacity_from_bytes(1_000, 160, threshold_policy);
        assert!(!below.pressure);
        let at_watermark = capacity_from_bytes(1_000, 150, threshold_policy);
        assert!(at_watermark.pressure);
        assert_eq!(at_watermark.used_percent, 85);
        assert!(at_watermark.available_bytes > 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_escape_is_reported_and_never_removed() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worker");
        let outside = temp.path().join("source");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.rs"), "source").unwrap();
        symlink(&outside, worktree.join("target")).unwrap();

        let scanned = scan_cache(&worktree, &worktree.join("target"), false).unwrap();
        assert!(scanned.unsafe_reason.is_some());
        assert_eq!(
            fs::read_to_string(outside.join("keep.rs")).unwrap(),
            "source"
        );

        let linked_worktree = temp.path().join("linked-worker");
        fs::create_dir_all(outside.join("target")).unwrap();
        symlink(&outside, &linked_worktree).unwrap();
        let linked = scan_cache(&linked_worktree, &linked_worktree.join("target"), false).unwrap();
        assert!(linked.unsafe_reason.is_some());
        assert!(outside.join("target").exists());
    }

    #[test]
    fn interrupted_quarantine_is_discovered_without_touching_source() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worker = cas_root.join("worktrees/dead-worker");
        let quarantine = worker.join(format!("{QUARANTINE_PREFIX}123-1"));
        fs::create_dir_all(&quarantine).unwrap();
        fs::write(quarantine.join("artifact.rlib"), b"artifact").unwrap();
        fs::write(worker.join("source.rs"), b"source").unwrap();

        let caches = discover_caches(temp.path(), &cas_root, &[]);
        assert_eq!(caches.len(), 1);
        assert!(caches[0].interrupted_cleanup);
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), b"source");
    }

    #[test]
    fn configured_or_store_known_worktree_outside_default_root_is_discovered() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join("repo/.cas");
        let custom = temp.path().join("scratch-worktrees/custom-worker");
        fs::create_dir_all(&cas_root).unwrap();
        fs::create_dir_all(custom.join("target")).unwrap();
        fs::write(custom.join("target/artifact"), b"artifact").unwrap();

        let report = inspect(
            &cas_root,
            TargetCachePolicy {
                high_watermark_percent: 1,
                low_watermark_percent: 0,
                min_idle_secs: 0,
                retention_count: 0,
            },
            std::slice::from_ref(&custom),
            &[],
            true,
        )
        .unwrap();

        assert_eq!(report.caches.len(), 1);
        assert_eq!(report.caches[0].worktree, custom.canonicalize().unwrap());
        assert_eq!(report.caches[0].bytes, 8);
    }

    #[test]
    fn live_and_recent_caches_are_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worker = cas_root.join("worktrees/live-worker");
        fs::create_dir_all(worker.join("target")).unwrap();
        fs::write(worker.join("target/artifact"), b"artifact").unwrap();

        let live_report = inspect(
            &cas_root,
            TargetCachePolicy {
                high_watermark_percent: 1,
                low_watermark_percent: 0,
                min_idle_secs: 0,
                retention_count: 0,
            },
            &[],
            std::slice::from_ref(&worker),
            true,
        )
        .unwrap();
        assert_eq!(
            live_report.caches[0].disposition,
            CacheDisposition::LiveProcess
        );

        let recent_report = inspect(
            &cas_root,
            TargetCachePolicy {
                high_watermark_percent: 1,
                low_watermark_percent: 0,
                min_idle_secs: u64::MAX,
                retention_count: 0,
            },
            &[],
            &[],
            true,
        )
        .unwrap();
        assert_eq!(
            recent_report.caches[0].disposition,
            CacheDisposition::RecentWrite
        );
        assert!(worker.join("target/artifact").exists());
    }

    #[cfg(unix)]
    #[test]
    fn os_process_cwd_marks_cache_live_without_registry_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worker = cas_root.join("worktrees/process-worker");
        fs::create_dir_all(worker.join("target")).unwrap();
        fs::write(worker.join("target/artifact"), b"artifact").unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&worker)
            .spawn()
            .unwrap();

        let report = inspect(
            &cas_root,
            TargetCachePolicy {
                high_watermark_percent: 1,
                low_watermark_percent: 0,
                min_idle_secs: 0,
                retention_count: 0,
            },
            &[],
            &[],
            true,
        )
        .unwrap();
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(report.caches[0].disposition, CacheDisposition::LiveProcess);
        assert!(worker.join("target/artifact").exists());
    }

    #[test]
    fn cleanup_removes_only_target_and_resumes_interrupted_quarantine() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        let worker = cas_root.join("worktrees/dead-worker");
        fs::create_dir_all(worker.join("target/deps")).unwrap();
        fs::write(worker.join("target/deps/artifact.rlib"), vec![0u8; 32]).unwrap();
        fs::write(worker.join("source.rs"), b"source").unwrap();

        let policy = TargetCachePolicy {
            high_watermark_percent: 1,
            low_watermark_percent: 0,
            min_idle_secs: 0,
            retention_count: 0,
        };
        let mut report = inspect(&cas_root, policy, &[], &[], false).unwrap();
        assert_eq!(report.caches[0].bytes, 32);
        assert_eq!(report.caches[0].disposition, CacheDisposition::Selected);
        cleanup_selected(&cas_root, &mut report, policy, &[]).unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::Reclaimed);
        assert!(!worker.join("target").exists());
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), b"source");

        let quarantine = worker.join(format!("{QUARANTINE_PREFIX}interrupted"));
        fs::create_dir_all(&quarantine).unwrap();
        fs::write(quarantine.join("partial"), b"partial").unwrap();
        let resume_policy = TargetCachePolicy {
            retention_count: 1,
            ..policy
        };
        let mut resumed = inspect(&cas_root, resume_policy, &[], &[], false).unwrap();
        assert!(resumed.caches[0].interrupted_cleanup);
        assert_eq!(resumed.caches[0].disposition, CacheDisposition::Selected);
        cleanup_selected(&cas_root, &mut resumed, resume_policy, &[]).unwrap();
        assert!(!quarantine.exists());
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), b"source");
    }
}
