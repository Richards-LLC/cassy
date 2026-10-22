//! Guarded Cargo target-cache pressure reporting and reclamation.
//!
//! Factory worktrees intentionally keep independent Cargo target directories:
//! sharing one target directory across concurrently-mutating branches is not a
//! correctness boundary Cassy can currently prove. This module therefore
//! reclaims only regenerable `target/` trees belonging to known factory
//! worktrees, and only after conservative liveness, recency, containment, and
//! filesystem-watermark checks.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::config::FactoryConfig;

mod lane;
mod unknown;
pub(crate) mod legacy;
#[cfg(unix)]
pub(crate) mod owner;
pub(crate) mod parked;
#[cfg(any(target_os = "linux", all(test, unix)))]
mod process_probe;
pub(crate) mod retirement;
pub(crate) mod scratch;
pub use lane::LanePreviewRecord;

const QUARANTINE_PREFIX: &str = ".cas-target-gc-";
const MANAGED_BRANCH_PREFIXES: [&str; 2] = ["epic/", "release/"];
const MANAGED_WORKTREE_PREFIXES: [&str; 2] = ["epic-", "release-"];

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
    Indeterminate,
    RecentWrite,
    Retained,
    Eligible,
    Selected,
    OwnershipUncertain,
    OwnershipChanged,
    UnsafePath,
    Reclaimed,
    CleanupError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetCacheRecord {
    pub path: PathBuf,
    pub worktree: PathBuf,
    pub bytes: u64,
    #[serde(default)]
    pub scan_elapsed_ms: u64,
    pub newest_write_unix_secs: Option<u64>,
    pub disposition: CacheDisposition,
    pub reason: String,
    pub interrupted_cleanup: bool,
    #[serde(skip)]
    ownership: CacheOwnership,
    #[serde(skip)]
    release_warm_on_pressure: bool,
    #[serde(skip)]
    git_admin_dir: Option<PathBuf>,
    #[serde(skip)]
    git_branch: Option<String>,
    #[serde(skip)]
    git_commit: Option<String>,
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
    #[serde(default)]
    pub filesystem_path: PathBuf,
    #[serde(default)]
    pub reserved_builders: usize,
    pub candidate_bytes: u64,
    pub selected_bytes: u64,
    pub reclaimed_bytes: u64,
    pub dry_run: bool,
    pub remediation: String,
    pub caches: Vec<TargetCacheRecord>,
    #[serde(default)]
    pub lane_previews: Vec<LanePreviewRecord>,
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
    scan_elapsed_ms: u64,
    snapshot: Option<legacy::Snapshot>,
    newest_write: Option<SystemTime>,
    interrupted_cleanup: bool,
    unsafe_reason: Option<String>,
    ownership: CacheOwnership,
    git_admin_dir: Option<PathBuf>,
    git_branch: Option<String>,
    git_commit: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum CacheOwnership {
    #[default]
    Unknown,
    Durable,
    GitConventionOnly,
}

#[derive(Debug, Clone)]
struct GitWorktreeCandidate {
    path: PathBuf,
    branch: Option<String>,
    commit: Option<String>,
    git_admin_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct GitWorktreeIdentity {
    branch: Option<String>,
    commit: Option<String>,
    git_admin_dir: Option<PathBuf>,
}

/// Fast filesystem-only pressure probe used before a factory starts workers.
pub fn capacity_status(
    repo_root: &Path,
    policy: TargetCachePolicy,
) -> io::Result<FilesystemCapacity> {
    capacity_status_with(repo_root, policy, crate::fs_space::fs_space)
}

pub(crate) fn capacity_status_with(
    root: &Path,
    policy: TargetCachePolicy,
    probe: impl FnOnce(&Path) -> io::Result<crate::fs_space::FsSpace>,
) -> io::Result<FilesystemCapacity> {
    let (total_bytes, available_bytes) = filesystem_capacity_from_space(probe(root)?);
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
    inspect_with_probe(
        cas_root,
        policy,
        known_worktree_roots,
        live_worktree_roots,
        dry_run,
        capacity_status,
    )
}

fn inspect_with_probe(
    cas_root: &Path,
    policy: TargetCachePolicy,
    known_worktree_roots: &[PathBuf],
    live_worktree_roots: &[PathBuf],
    dry_run: bool,
    probe: impl Fn(&Path, TargetCachePolicy) -> io::Result<FilesystemCapacity>,
) -> io::Result<TargetCacheReport> {
    inspect_scoped_with_probe(
        cas_root, policy, known_worktree_roots, live_worktree_roots, dry_run, false, probe,
    )
}

/// Admission reclaims verified relocated worker and resolved review targets. Keep full factory
/// GC (previews, in-tree caches, traces and artifacts) an explicit operation.
pub(crate) fn reclaim_scratch_for_admission(
    cas_root: &Path,
    policy: TargetCachePolicy,
    probe: impl Fn(&Path, TargetCachePolicy) -> io::Result<FilesystemCapacity>,
) -> io::Result<TargetCacheReport> {
    let review_bytes = crate::factory_review_scratch::reclaim_targets(cas_root, None);
    let mut report = inspect_scoped_with_probe(cas_root, policy, &[], &[], false, true, probe)?;
    cleanup_selected(cas_root, &mut report, policy, &[])?;
    report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(review_bytes);
    Ok(report)
}

fn inspect_scoped_with_probe(
    cas_root: &Path,
    policy: TargetCachePolicy,
    known_worktree_roots: &[PathBuf],
    live_worktree_roots: &[PathBuf],
    dry_run: bool,
    scratch_only: bool,
    probe: impl Fn(&Path, TargetCachePolicy) -> io::Result<FilesystemCapacity>,
) -> io::Result<TargetCacheReport> {
    let repo_root = cas_root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cassy root has no repository parent",
        )
    })?;
    let config = crate::config::Config::load(cas_root)
        .map_err(io::Error::other)?
        .factory();
    let filesystem_path = crate::factory_scratch::build_filesystem(cas_root, &config);
    let mut capacity = probe(&filesystem_path, policy)?;
    let reserved_builders = config.max_concurrent_builders;
    capacity.pressure = crate::factory_build_guard::disk_pressure(&capacity, reserved_builders);
    let now = SystemTime::now();
    let mut scanned = discover_caches(repo_root, cas_root, known_worktree_roots);
    if scratch_only {
        scanned.retain(|cache| relocated_target(&cache.worktree, &cache.path));
    }
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
        } else if cache.ownership == CacheOwnership::Unknown {
            (
                CacheDisposition::OwnershipUncertain,
                if relocated_target(&cache.worktree, &cache.path) {
                    "scratch target has no verifiable checkout owner; review-only, no automatic adoption or named reclaim".into()
                } else {
                    "unknown owner; review-only. Remediation: manual review, then gc_cleanup reclaim_unknown_target=<exact absolute cache path> force=true dry_run=false; positive quiet window and native/build guards remain mandatory; no ownership adoption".into()
                },
            )
        } else if let Err((disposition, reason)) = cache_admission(
            cas_root,
            &cache.worktree,
            &cache.path,
            &canonical_live_roots,
            cache.snapshot.as_ref(),
            false,
        ) {
            (disposition, reason)
        } else if cache.newest_write.is_some_and(|modified| {
            now.duration_since(modified).unwrap_or_default()
                < Duration::from_secs(policy.min_idle_secs)
        }) {
            (
                CacheDisposition::RecentWrite,
                format!("newest write is younger than {}s", policy.min_idle_secs),
            )
        } else if cache.ownership == CacheOwnership::GitConventionOnly {
            (
                CacheDisposition::OwnershipUncertain,
                "Git convention proves repository membership but not Cassy ownership; durable provenance is required for cleanup".to_string(),
            )
        } else {
            (
                CacheDisposition::Eligible,
                if legacy::lane_target(cas_root, &cache.worktree, &cache.path) {
                    "stale regenerable Cargo target cache; cleanup will revalidate lane/Cargo/lifetime locks".to_string()
                } else {
                    "stale regenerable Cargo target cache".to_string()
                },
            )
        };
        let release_warm_on_pressure = capacity.pressure
            && disposition == CacheDisposition::Eligible
            && legacy::lane_target(cas_root, &cache.worktree, &cache.path)
            && !relocated_target(&cache.worktree, &cache.path)
            && legacy::terminal_dead_owner(cas_root, &cache.worktree);
        records.push(TargetCacheRecord {
            path: cache.path,
            worktree: cache.worktree,
            bytes: cache.bytes,
            scan_elapsed_ms: cache.scan_elapsed_ms,
            newest_write_unix_secs,
            disposition,
            reason,
            interrupted_cleanup: cache.interrupted_cleanup,
            release_warm_on_pressure,
            ownership: cache.ownership,
            git_admin_dir: cache.git_admin_dir,
            git_branch: cache.git_branch,
            git_commit: cache.git_commit,
        });
    }

    // Never credit deletion on /home against pressure on a scratch mount.
    // Each device gets its own watermark and warm-cache retention decision.
    let main_device = crate::factory_scratch::filesystem_id(&filesystem_path)?;
    let mut devices: HashMap<u64, Vec<TargetCacheRecord>> = HashMap::new();
    for mut record in records {
        match crate::factory_scratch::filesystem_id(&record.path) {
            Ok(device) => devices.entry(device).or_default().push(record),
            Err(error) => {
                record.disposition = CacheDisposition::Indeterminate;
                record.reason = format!("filesystem identity unavailable: {error}");
                devices.entry(main_device).or_default().push(record);
            }
        }
    }
    let mut records = Vec::new();
    for (device, mut group) in devices {
        let status = if device == main_device {
            Ok(capacity.clone())
        } else {
            probe(&group[0].path, policy)
        };
        match status {
            Ok(status) => plan_records(&mut group, &status, policy),
            Err(error) => {
                for record in &mut group {
                    record.disposition = CacheDisposition::Indeterminate;
                    record.reason = format!("filesystem capacity unavailable: {error}");
                }
            }
        }
        records.extend(group);
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    let candidate_bytes = records.iter().map(|record| record.bytes).sum();
    let lane_previews = if scratch_only {
        Vec::new()
    } else {
        lane::inspect(cas_root, policy, live_worktree_roots)
    };
    let selected_bytes = records
        .iter()
        .filter(|record| record.disposition == CacheDisposition::Selected)
        .map(|record| record.bytes)
        .sum::<u64>()
        .saturating_add(
            lane_previews
                .iter()
                .filter(|preview| preview.disposition == CacheDisposition::Selected)
                .map(|preview| preview.bytes)
                .sum::<u64>(),
        );
    Ok(TargetCacheReport {
        schema_version: 1,
        filesystem: capacity,
        filesystem_path,
        reserved_builders,
        candidate_bytes,
        selected_bytes,
        reclaimed_bytes: 0,
        dry_run,
        remediation: "Review gc_report, then run gc_cleanup force=true dry_run=false; Cassy revalidates ownership, liveness, recency, and path containment immediately before each rename.".to_string(),
        caches: records,
        lane_previews,
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
    cleanup_selected_with_opt_in(cas_root, report, policy, live_worktree_roots, None)
}

/// An exact path is an explicit reclaim authorization, never ownership proof.
/// Unknown-owner caches otherwise remain review-only.
pub fn cleanup_selected_with_opt_in(
    cas_root: &Path,
    report: &mut TargetCacheReport,
    policy: TargetCachePolicy,
    live_worktree_roots: &[PathBuf],
    reclaim_unknown_target: Option<&Path>,
) -> io::Result<()> {
    cleanup_selected_using(cas_root, report, policy, live_worktree_roots,
        reclaim_unknown_target, Path::new("/proc"))
}

fn cleanup_selected_using(
    cas_root: &Path,
    report: &mut TargetCacheReport,
    policy: TargetCachePolicy,
    live_worktree_roots: &[PathBuf],
    reclaim_unknown_target: Option<&Path>,
    proc_root: &Path,
) -> io::Result<()> {
    if let Some(path) = reclaim_unknown_target {
        // Named reclaim never widens into ordinary cache, preview or artifact GC.
        for record in &mut report.caches {
            if record.disposition == CacheDisposition::Selected {
                record.disposition = CacheDisposition::Eligible;
                record.reason = "not named by this exact-path reclaim".into();
            }
        }
        report.selected_bytes = 0;
        let named = report.caches.iter_mut().find(|record| record.path.as_os_str() == path.as_os_str());
        let Some(record) = named else {
            report.remediation = format!("Refused {}: opt-in must name one exact inventoried unknown-owner cache; no paths reclaimed", path.display());
            return Ok(());
        };
        if let Err((disposition, reason)) = unknown::validate(cas_root, record, path, policy) {
            record.disposition = disposition;
            record.reason = reason;
            return Ok(());
        }
        if report.dry_run {
            record.reason = "review-only: named reclaim requires force=true AND explicit dry_run=false; quiet/build guards are not waived".into();
            return Ok(());
        }
        record.disposition = CacheDisposition::Selected;
        record.reason = "explicit consent for this unknown-owner cache; destructive guards pending".into();
        report.selected_bytes = record.bytes;
    } else if report.dry_run {
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
        let explicit_unknown = reclaim_unknown_target.is_some() && record.ownership == CacheOwnership::Unknown;
        if (if let Some(path) = reclaim_unknown_target {
            unknown::validate(cas_root, record, path, policy).is_err()
        } else { !ownership_is_current(cas_root, record) })
            || record.release_warm_on_pressure
                && !legacy::terminal_dead_owner(cas_root, &record.worktree)
        {
            record.disposition = CacheDisposition::OwnershipChanged;
            record.reason =
                "cache ownership or terminal-task evidence changed after report; refusing destructive cleanup"
                    .to_string();
            continue;
        }
        let rescanned = scan_cache(&record.worktree, &record.path, record.interrupted_cleanup);
        let Ok(rescanned) = rescanned else {
            record.disposition = CacheDisposition::CleanupError;
            record.reason = "cache vanished or became unreadable during cleanup".to_string();
            continue;
        };
        if let Some(reason) = &rescanned.unsafe_reason {
            record.disposition = CacheDisposition::UnsafePath;
            record.reason = reason.clone();
            continue;
        }
        let mut guard = match cache_admission_at(
            cas_root,
            &rescanned.worktree,
            &rescanned.path,
            &canonical_live_roots,
            rescanned.snapshot.as_ref(),
            true,
            proc_root,
            explicit_unknown,
        ) {
            Ok(guard) => guard,
            Err((disposition, reason)) => {
                record.disposition = disposition;
                record.reason = reason;
                continue;
            }
        };
        // Pressure may revoke a terminal dead lane's warm reservation. Keep
        // the lane, every Cargo lock and any lifetime lease through deletion
        // on macOS too; readable process evidence is still mandatory.
        if record.release_warm_on_pressure && guard.is_none() {
            match rescanned
                .snapshot
                .as_ref()
                .ok_or_else(|| io::Error::other("missing cache snapshot"))
                .and_then(|snapshot| {
                    legacy::acquire(cas_root, &record.worktree, &record.path, snapshot)
                }) {
                Ok(held) => guard = Some(held),
                Err(error) => {
                    record.disposition = CacheDisposition::Indeterminate;
                    record.reason = format!("terminal lane pressure guard refused: {error}");
                    continue;
                }
            }
        }
        let under_guard = if explicit_unknown {
            match scan_cache(&record.worktree, &record.path, false) {
                Ok(scan) => Some(scan),
                Err(error) => {
                    record.disposition = CacheDisposition::Indeterminate;
                    record.reason = format!("quiet-window recheck failed under build guards: {error}");
                    continue;
                }
            }
        } else { None };
        let newest = under_guard.as_ref().unwrap_or(&rescanned);
        if newest
            .newest_write
            .is_some_and(|modified| modified > cutoff)
        {
            record.disposition = CacheDisposition::RecentWrite;
            record.reason = "cache received a write during destructive revalidation".to_string();
            continue;
        }

        if record.release_warm_on_pressure
            && !legacy::terminal_dead_owner(cas_root, &record.worktree)
            || guard
                .as_ref()
                .is_some_and(|guard| !guard.unchanged(&record.path))
            || guard.is_some()
                && ((!explicit_unknown && !legacy::lane_target(cas_root, &record.worktree, &record.path))
                    || !legacy::registry_allows(cas_root, &record.worktree))
        {
            record.disposition = CacheDisposition::Indeterminate;
            record.reason = "legacy lane guards changed before rename".into();
            continue;
        }
        if let Some(path) = reclaim_unknown_target
            && let Err((disposition, reason)) = unknown::validate(cas_root, record, path, policy) {
                record.disposition = disposition;
                record.reason = reason;
                continue;
        }
        let descriptors = guard
            .as_ref()
            .map_or_else(Vec::new, legacy::Guard::descriptors);
        if let Some(disposition) = process_block_at(
            &record.worktree,
            &record.path,
            &descriptors,
            guard.is_some() && !explicit_unknown,
            proc_root,
        ) {
            record.disposition = disposition;
            record.reason = "process evidence changed before rename".into();
            continue;
        }
        ordinal += 1;
        let quarantine = if record.interrupted_cleanup {
            record.path.clone()
        } else {
            let path = if relocated_target(&record.worktree, &record.path) {
                record.path.with_file_name(format!(
                    "{QUARANTINE_PREFIX}{}--{}",
                    record
                        .worktree
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    uuid::Uuid::new_v4()
                ))
            } else {
                record.worktree.join(format!(
                    "{QUARANTINE_PREFIX}{}-{ordinal}",
                    std::process::id()
                ))
            };
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
        // With unknown ownership, recheck file writes after the atomic rename
        // too: an in-place artifact rewrite need not change a directory mtime.
        if explicit_unknown {
            let quiet = scan_cache(&record.worktree, &quarantine, true).ok().is_some_and(|scan| {
                scan.unsafe_reason.is_none()
                    && scan.snapshot.as_ref().is_some_and(legacy::Snapshot::admissible)
                    && scan.newest_write.is_some_and(|modified| modified <= cutoff)
            });
            if !quiet {
                if !record.path.exists() { let _ = fs::rename(&quarantine, &record.path); }
                record.disposition = CacheDisposition::RecentWrite;
                record.reason = format!("quiet-window or tree evidence changed after quarantine; cache preserved (quarantine {})", quarantine.display());
                continue;
            }
        }
        let process_block =
            process_block_at(&record.worktree, &quarantine, &descriptors, guard.is_some() && !explicit_unknown, proc_root);
        if guard
            .as_ref()
            .is_some_and(|guard| !guard.unchanged(&quarantine))
            || process_block.is_some()
        {
            if !record.interrupted_cleanup && !record.path.exists() {
                let _ = fs::rename(&quarantine, &record.path);
            }
            record.disposition = process_block.unwrap_or(CacheDisposition::Indeterminate);
            record.reason = "guards changed after atomic quarantine rename".to_string();
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
    // cas-cef2: File close alone cannot release a flock inherited by a
    // concurrently forked child; make the operation boundary explicit.
    let previously_reclaimed = report
        .lane_previews
        .iter()
        .filter(|preview| preview.disposition == CacheDisposition::Reclaimed)
        .map(|preview| preview.bytes)
        .sum::<u64>();
    if reclaim_unknown_target.is_none() {
        lane::cleanup(cas_root, &mut report.lane_previews, policy, live_worktree_roots);
    }
    report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(
        report
            .lane_previews
            .iter()
            .filter(|preview| preview.disposition == CacheDisposition::Reclaimed)
            .map(|preview| preview.bytes)
            .sum::<u64>()
            .saturating_sub(previously_reclaimed),
    );
    FileExt::unlock(&lock)?;
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
        .filter(|index| {
            !records[**index].interrupted_cleanup && !records[**index].release_warm_on_pressure
        })
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
        records[index].reason = if records[index].release_warm_on_pressure {
            "selected terminal-owned dead lane under pressure; warm reservation released; cleanup revalidates owner, tasks and locks".into()
        } else {
            "selected oldest-first to reach the low watermark".into()
        };
        bytes_needed = bytes_needed.saturating_sub(records[index].bytes);
    }
}

fn discover_caches(
    repo_root: &Path,
    cas_root: &Path,
    known_worktree_roots: &[PathBuf],
) -> Vec<ScannedCache> {
    let git_worktrees = list_validated_git_worktrees(repo_root);
    let git_identities: HashMap<PathBuf, GitWorktreeIdentity> = git_worktrees
        .iter()
        .map(|worktree| {
            (
                worktree.path.clone(),
                GitWorktreeIdentity {
                    branch: worktree.branch.clone(),
                    commit: worktree.commit.clone(),
                    git_admin_dir: worktree.git_admin_dir.clone(),
                },
            )
        })
        .collect();
    let mut roots = HashMap::from([(repo_root.to_path_buf(), CacheOwnership::Durable)]);
    if let Ok(entries) = fs::read_dir(cas_root.join("worktrees")) {
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                insert_root(&mut roots, path, CacheOwnership::Durable);
            }
        }
    }
    for path in known_worktree_roots.iter().filter(|path| path.exists()) {
        insert_root(&mut roots, path.clone(), CacheOwnership::Durable);
    }
    for worktree in git_worktrees.into_iter().filter(|worktree| {
        worktree.branch.as_deref().is_some_and(|branch| {
            MANAGED_BRANCH_PREFIXES
                .iter()
                .any(|prefix| branch.starts_with(prefix))
        }) || managed_worktree_path(&worktree.path, cas_root)
            || lane::is_preview_path(cas_root, &worktree.path)
    }) {
        // Lane targets are reportable even without provenance, but their
        // checkout lifecycle belongs to the independently locked preview GC.
        let ownership = if managed_worktree_path(&worktree.path, cas_root) {
            CacheOwnership::Durable
        } else {
            CacheOwnership::GitConventionOnly
        };
        insert_root(&mut roots, worktree.path, ownership);
    }
    // A missing admin directory removes Git's ownership authority, even if
    // a stale registry/config row still lists this checkout. Measure it only.
    for (checkout, _) in missing_gitdir_checkouts(cas_root) {
        roots.insert(checkout, CacheOwnership::Unknown);
    }
    let scratch_owners = roots.clone();
    let mut roots: Vec<_> = roots.into_iter().collect();
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    let mut caches = Vec::new();
    for (root, ownership) in roots {
        // Preview ownership, recency and lifetime locks are enforced by lane
        // cleanup. Never select the same bytes through ordinary target GC.
        let ownership = if lane::is_preview_path(cas_root, &root) {
            CacheOwnership::GitConventionOnly
        } else {
            ownership
        };
        let git_identity = git_identities.get(&root);
        let target = root.join("target");
        if fs::symlink_metadata(&target).is_ok() {
            let mut cache = scan_cache(&root, &target, false)
                .unwrap_or_else(|error| scan_error_record(root.clone(), target, false, error));
            cache.ownership = ownership;
            cache.git_admin_dir = git_identity.and_then(|identity| identity.git_admin_dir.clone());
            cache.git_branch = git_identity.and_then(|identity| identity.branch.clone());
            cache.git_commit = git_identity.and_then(|identity| identity.commit.clone());
            caches.push(cache);
        }
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(QUARANTINE_PREFIX)
                {
                    let path = entry.path();
                    let mut cache = scan_cache(&root, &path, true)
                        .unwrap_or_else(|error| scan_error_record(root.clone(), path, true, error));
                    cache.ownership = ownership;
                    cache.git_admin_dir =
                        git_identity.and_then(|identity| identity.git_admin_dir.clone());
                    cache.git_branch = git_identity.and_then(|identity| identity.branch.clone());
                    cache.git_commit = git_identity.and_then(|identity| identity.commit.clone());
                    caches.push(cache);
                }
            }
        }
    }
    if let Some(scratch) = crate::factory_scratch::configured_scratch_root(cas_root) {
        let targets =
            cas_types::factory_scratch::project_scratch_dir(&scratch, cas_root).join("targets");
        for entry in fs::read_dir(&targets).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let interrupted = name.starts_with(QUARANTINE_PREFIX);
            let worker = if interrupted {
                let Some((worker, suffix)) = name
                    .strip_prefix(QUARANTINE_PREFIX)
                    .and_then(|name| name.rsplit_once("--"))
                else {
                    continue;
                };
                if uuid::Uuid::parse_str(suffix).is_err() {
                    continue;
                }
                worker
            } else {
                &name
            };
            let worktree = cas_root.join("worktrees").join(worker);
            let ownership = scratch_owners
                .get(&worktree)
                .copied()
                .unwrap_or(CacheOwnership::Unknown);
            let mut cache = scan_cache(&worktree, &path, interrupted).unwrap_or_else(|error| {
                scan_error_record(worktree.clone(), path, interrupted, error)
            });
            cache.ownership = ownership;
            if let Some(identity) = git_identities.get(&worktree) {
                cache.git_admin_dir = identity.git_admin_dir.clone();
                cache.git_branch = identity.branch.clone();
                cache.git_commit = identity.commit.clone();
            }
            caches.push(cache);
        }
    }
    let retired = cas_root.join("merge-sweeps/assembly-target");
    if fs::symlink_metadata(&retired).is_ok() {
        let worktree = cas_root.join("merge-sweeps");
        let mut cache = scan_cache(&worktree, &retired, false)
            .unwrap_or_else(|error| scan_error_record(worktree, retired, false, error));
        cache.ownership = CacheOwnership::Unknown;
        caches.push(cache);
    }
    caches
}

fn insert_root(
    roots: &mut HashMap<PathBuf, CacheOwnership>,
    path: PathBuf,
    ownership: CacheOwnership,
) {
    roots
        .entry(path)
        .and_modify(|current| {
            if *current == CacheOwnership::GitConventionOnly && ownership == CacheOwnership::Durable
            {
                *current = ownership;
            }
        })
        .or_insert(ownership);
}

/// Enumerate linked checkouts that may contain factory target caches. Git is
/// the source of truth for detached release worktrees; branch/path filters
/// identify candidates, while durable provenance decides cleanup eligibility.
fn list_validated_git_worktrees(repo_root: &Path) -> Vec<GitWorktreeCandidate> {
    let Some(common_git_dir) = git_common_dir(repo_root) else {
        return Vec::new();
    };
    let worktrees = crate::worktree::GitOperations::new(repo_root.to_path_buf())
        .list_worktrees()
        .unwrap_or_default();
    worktrees
        .into_iter()
        .filter(|worktree| !worktree.is_bare)
        .filter_map(|worktree| {
            let path = if worktree.path.is_absolute() {
                worktree.path
            } else {
                repo_root.join(worktree.path)
            };
            let symlink = fs::symlink_metadata(&path)
                .ok()
                .is_some_and(|metadata| metadata.file_type().is_symlink());
            let git_admin_dir = linked_worktree_admin_dir(&path, &common_git_dir);
            if git_admin_dir.is_none() && !symlink {
                return None;
            }
            Some(GitWorktreeCandidate {
                path,
                branch: worktree.branch,
                commit: worktree.commit,
                git_admin_dir,
            })
        })
        .collect()
}

/// Older factory checkouts can sit directly under .cas as well as worktrees/.
/// A deleted Git common directory makes them invisible to `git worktree list`.
/// Inventory only: missing Git metadata does not prove the checkout has no WIP.
pub(crate) fn missing_gitdir_checkouts(
    cas_root: &std::path::Path,
) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    let mut found = Vec::new();
    for root in [cas_root.to_path_buf(), cas_root.join("worktrees")] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let checkout = entry.path();
            let marker = checkout.join(".git");
            if !std::fs::symlink_metadata(&marker).is_ok_and(|md| md.is_file()) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(marker) else {
                continue;
            };
            let Some(target) = contents.trim().strip_prefix("gitdir: ") else {
                continue;
            };
            if target.is_empty() {
                continue;
            }
            let target = checkout.join(target);
            if matches!(target.try_exists(), Ok(false)) {
                found.push((checkout, target));
            }
        }
    }
    found.sort();
    found
}

fn git_common_dir(repo_root: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = std::str::from_utf8(&output.stdout).ok()?.trim();
    let path = Path::new(raw);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    path.canonicalize().ok()
}

/// Return the validated Git admin directory for a linked checkout.
/// A path replaced by a symlink is left for `scan_cache` to report as unsafe,
/// while a regular path without Git's linked-worktree marker is omitted.
fn linked_worktree_admin_dir(path: &Path, common_git_dir: &Path) -> Option<PathBuf> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return None;
    };
    if metadata.file_type().is_symlink() {
        return None;
    }
    if !metadata.is_dir() {
        return None;
    }
    let git_file = path.join(".git");
    let Ok(git_metadata) = fs::symlink_metadata(&git_file) else {
        return None;
    };
    if git_metadata.file_type().is_symlink() || !git_metadata.is_file() {
        return None;
    }
    let Ok(contents) = fs::read_to_string(git_file) else {
        return None;
    };
    let Some(raw_git_dir) = contents
        .lines()
        .find_map(|line| line.strip_prefix("gitdir: "))
    else {
        return None;
    };
    let git_dir = Path::new(raw_git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir.to_path_buf()
    } else {
        path.join(git_dir)
    };
    let Ok(git_dir) = git_dir.canonicalize() else {
        return None;
    };
    let Ok(worktrees_dir) = common_git_dir.join("worktrees").canonicalize() else {
        return None;
    };
    if git_dir.parent() != Some(worktrees_dir.as_path()) {
        return None;
    }
    let Ok(admin_gitdir) = fs::read_to_string(git_dir.join("gitdir")) else {
        return None;
    };
    let admin_gitdir = admin_gitdir.trim();
    let admin_gitdir = Path::new(admin_gitdir);
    let admin_gitdir = if admin_gitdir.is_absolute() {
        admin_gitdir.to_path_buf()
    } else {
        git_dir.join(admin_gitdir)
    };
    let Ok(admin_gitdir) = admin_gitdir.canonicalize() else {
        return None;
    };
    let Ok(checkout_gitdir) = path.join(".git").canonicalize() else {
        return None;
    };
    (admin_gitdir == checkout_gitdir).then_some(git_dir)
}

fn managed_worktree_path(path: &Path, cas_root: &Path) -> bool {
    let Some(parent) = path.parent().and_then(|parent| parent.canonicalize().ok()) else {
        return false;
    };
    let Ok(cas_root) = cas_root.canonicalize() else {
        return false;
    };
    parent == cas_root
        && path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            MANAGED_WORKTREE_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
}

fn ownership_is_current(cas_root: &Path, record: &TargetCacheRecord) -> bool {
    if matches!(
        record.ownership,
        CacheOwnership::Unknown | CacheOwnership::GitConventionOnly
    ) {
        return false;
    }
    let Some(expected_git_admin_dir) = record.git_admin_dir.as_ref() else {
        return true;
    };
    let Some(repo_root) = cas_root.parent() else {
        return false;
    };
    let Ok(expected_worktree) = record.worktree.canonicalize() else {
        return false;
    };
    list_validated_git_worktrees(repo_root)
        .into_iter()
        .any(|worktree| {
            let Some(git_admin_dir) = worktree.git_admin_dir else {
                return false;
            };
            let Ok(path) = worktree.path.canonicalize() else {
                return false;
            };
            path == expected_worktree
                && git_admin_dir == *expected_git_admin_dir
                && worktree.branch.as_ref() == record.git_branch.as_ref()
                && worktree.commit.as_ref() == record.git_commit.as_ref()
        })
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
        scan_elapsed_ms: 0,
        snapshot: None,
        newest_write: None,
        interrupted_cleanup,
        unsafe_reason: Some(format!("scan failed closed: {error}")),
        ownership: CacheOwnership::Durable,
        git_admin_dir: None,
        git_branch: None,
        git_commit: None,
    }
}

/// Only the configured project namespace and exact worker name are managed.
/// Quarantines encode their worker and a UUID and stay beside the target.
fn relocated_target(worktree: &Path, path: &Path) -> bool {
    if cas_types::factory_scratch::managed_worktree_store(worktree).is_none() {
        return false;
    }
    let target = crate::factory_scratch::worker_target_dir(worktree);
    if target == worktree.join("target") || path.parent() != target.parent() {
        return false;
    }
    if path == target {
        return true;
    }
    let Some(worker) = worktree.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(&format!("{QUARANTINE_PREFIX}{worker}--")))
        .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok())
}

fn scan_cache(worktree: &Path, path: &Path, interrupted_cleanup: bool) -> io::Result<ScannedCache> {
    let started = Instant::now();
    let relocated = relocated_target(worktree, path);
    let worktree_metadata = fs::symlink_metadata(worktree);
    let missing_scratch_owner = relocated
        && worktree_metadata
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
    if !missing_scratch_owner && worktree_metadata.as_ref().is_err() {
        return Err(worktree_metadata.unwrap_err());
    }
    if worktree_metadata
        .as_ref()
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Ok(ScannedCache {
            path: path.to_path_buf(),
            worktree: worktree.to_path_buf(),
            bytes: 0,
            scan_elapsed_ms: 0,
            snapshot: None,
            newest_write: None,
            interrupted_cleanup,
            unsafe_reason: Some("worktree root is a symlink or not a directory".to_string()),
            ownership: CacheOwnership::Durable,
            git_admin_dir: None,
            git_branch: None,
            git_commit: None,
        });
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(ScannedCache {
            path: path.to_path_buf(),
            worktree: worktree.to_path_buf(),
            bytes: 0,
            scan_elapsed_ms: 0,
            snapshot: None,
            newest_write: None,
            interrupted_cleanup,
            unsafe_reason: Some("cache root is a symlink or not a directory".to_string()),
            ownership: CacheOwnership::Durable,
            git_admin_dir: None,
            git_branch: None,
            git_commit: None,
        });
    }
    let canonical_worktree = if missing_scratch_owner {
        worktree.to_path_buf()
    } else {
        worktree.canonicalize()?
    };
    let canonical_path = path.canonicalize()?;
    let allowed_name = path.file_name().is_some_and(|name| {
        name == "target" || name == "assembly-target" || name.to_string_lossy().starts_with(QUARANTINE_PREFIX)
    });
    if !(relocated && canonical_path == path)
        && (!allowed_name || canonical_path.parent() != Some(canonical_worktree.as_path()))
    {
        return Ok(ScannedCache {
            path: path.to_path_buf(),
            worktree: canonical_worktree,
            bytes: 0,
            scan_elapsed_ms: 0,
            snapshot: None,
            newest_write: None,
            interrupted_cleanup,
            unsafe_reason: Some("cache path escaped its exact worktree parent".to_string()),
            ownership: CacheOwnership::Durable,
            git_admin_dir: None,
            git_branch: None,
            git_commit: None,
        });
    }

    if relocated && canonical_path != path {
        return Err(io::Error::other("scratch target has a symlinked ancestor"));
    }
    let mut snapshot = legacy::Snapshot::new();
    let mut bytes = 0u64;
    let mut newest_write = metadata.modified().ok();
    for entry in WalkDir::new(&canonical_path).follow_links(false) {
        let entry = entry.map_err(io::Error::other)?;
        let metadata = entry.metadata().map_err(io::Error::other)?;
        snapshot.record(&canonical_path, &entry, &metadata)?;
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
        scan_elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        snapshot: Some(snapshot),
        newest_write,
        interrupted_cleanup,
        unsafe_reason: None,
        ownership: CacheOwnership::Durable,
        git_admin_dir: None,
        git_branch: None,
        git_commit: None,
    })
}

fn cache_admission(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    live: &HashSet<PathBuf>,
    snapshot: Option<&legacy::Snapshot>,
    lock: bool,
) -> Result<Option<legacy::Guard>, (CacheDisposition, String)> {
    cache_admission_at(
        cas_root,
        worktree,
        cache,
        live,
        snapshot,
        lock,
        Path::new("/proc"),
        false,
    )
}

fn cache_admission_at(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    live: &HashSet<PathBuf>,
    snapshot: Option<&legacy::Snapshot>,
    lock: bool,
    proc_root: &Path,
    explicit_unknown: bool,
) -> Result<Option<legacy::Guard>, (CacheDisposition, String)> {
    if live.contains(worktree) {
        return Err((
            CacheDisposition::LiveProcess,
            "fresh registered owner with matching live PID".into(),
        ));
    }
    if explicit_unknown {
        // Unknown owners get no opaque-process exception, even under locks.
        if let Some(disposition) = process_block_at(worktree, cache, &[], false, proc_root) {
            return Err((disposition, "native process/handle evidence does not prove the unknown cache idle".into()));
        }
        let snapshot = snapshot.filter(|snapshot| snapshot.admissible())
            .ok_or_else(|| (CacheDisposition::UnsafePath, "unsafe unknown-cache snapshot".into()))?;
        if !lock { return Ok(None); }
        let guard = legacy::acquire_explicit(cas_root, worktree, cache, snapshot)
            .map_err(|error| (CacheDisposition::Indeterminate, format!("unknown-cache build guard refused: {error}")))?;
        if let Some(disposition) = process_block_at(worktree, cache, &guard.descriptors(), false, proc_root) {
            return Err((disposition, "process evidence changed under unknown-cache build guard".into()));
        }
        return Ok(Some(guard));
    }
    // Relocated output uses the same lane/Cargo guards on every Unix host.
    // A marked target must also prove its durable generation/lifetime lease.
    #[cfg(unix)]
    if relocated_target(worktree, cache) {
        legacy::owner_gate(cas_root, worktree)?;
        let snapshot = snapshot
            .filter(|snapshot| snapshot.admissible())
            .ok_or_else(|| {
                (
                    CacheDisposition::UnsafePath,
                    "unsafe scratch target snapshot".into(),
                )
            })?;
        if !lock && cache.join(owner::MARKER).exists() {
            let _lease = owner::for_gc(cas_root, worktree, cache)
                .and_then(|lease| {
                    lease.ok_or_else(|| io::Error::other("missing scratch target provenance"))
                })
                .map_err(|error| {
                    (
                        if error.kind() == io::ErrorKind::WouldBlock {
                            CacheDisposition::LiveProcess
                        } else {
                            CacheDisposition::Indeterminate
                        },
                        error.to_string(),
                    )
                })?;
        }
        if let Some(disposition) =
            process_block_at(worktree, cache, &[], cfg!(target_os = "linux"), proc_root)
        {
            return Err((
                disposition,
                "scratch target process evidence does not establish idle".into(),
            ));
        }
        if !lock {
            return Ok(None);
        }
        let guard = legacy::acquire(cas_root, worktree, cache, snapshot).map_err(|error| {
            (
                CacheDisposition::Indeterminate,
                format!("scratch target build guard refused: {error}"),
            )
        })?;
        if let Some(disposition) = process_block_at(
            worktree,
            cache,
            &guard.descriptors(),
            cfg!(target_os = "linux"),
            proc_root,
        ) {
            return Err((
                disposition,
                "process evidence changed under scratch target build guards".into(),
            ));
        }
        return Ok(Some(guard));
    }
    #[cfg(target_os = "linux")]
    {
        linux_cache_admission_scanned(cas_root, worktree, cache, proc_root, snapshot, lock)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cas_root, snapshot, lock, proc_root);
        if live_process_uses(worktree, cache) {
            Err((
                CacheDisposition::Indeterminate,
                "process evidence does not establish idle".into(),
            ))
        } else {
            Ok(None)
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
fn linux_cache_admission(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    proc_root: &Path,
) -> Result<Option<legacy::Guard>, (CacheDisposition, String)> {
    let scanned = scan_cache(worktree, cache, false)
        .map_err(|error| (CacheDisposition::Indeterminate, error.to_string()))?;
    linux_cache_admission_scanned(
        cas_root,
        worktree,
        cache,
        proc_root,
        scanned.snapshot.as_ref(),
        true,
    )
}

#[cfg(target_os = "linux")]
fn linux_cache_admission_scanned(
    cas_root: &Path,
    worktree: &Path,
    cache: &Path,
    proc_root: &Path,
    snapshot: Option<&legacy::Snapshot>,
    lock: bool,
) -> Result<Option<legacy::Guard>, (CacheDisposition, String)> {
    use process_probe::Evidence;
    let evidence = process_probe::linux_evidence_many(proc_root, worktree, cache, &[]);
    if let Evidence::Live { pid, field } = evidence {
        return Err((
            CacheDisposition::LiveProcess,
            format!("positive process evidence: pid={pid:?} field={field}"),
        ));
    }
    if legacy::lane_target(cas_root, worktree, cache) {
        legacy::owner_gate(cas_root, worktree)?;
        let snapshot = snapshot
            .filter(|snapshot| snapshot.admissible())
            .ok_or_else(|| {
                (
                    CacheDisposition::Indeterminate,
                    "unsafe or missing cache snapshot".into(),
                )
            })?;
        if !lock {
            if matches!(evidence, Evidence::Opaque { pid: None }) {
                return Err((
                    CacheDisposition::Indeterminate,
                    "process table unavailable".into(),
                ));
            }
            return Ok(None); // Preview never creates or acquires build locks.
        }
        let guard = legacy::acquire(cas_root, worktree, cache, snapshot).map_err(|error| {
            (
                CacheDisposition::Indeterminate,
                format!("legacy lane guard refused: {error}"),
            )
        })?;
        if let Some(disposition) =
            linux_process_block(proc_root, worktree, cache, &guard.descriptors(), true)
        {
            return Err((
                disposition,
                "process evidence changed under legacy lane guard".into(),
            ));
        }
        return Ok(Some(guard));
    }
    if matches!(evidence, Evidence::Opaque { .. }) {
        return Err((
            CacheDisposition::Indeterminate,
            "opaque process evidence; target is outside guarded worker-lane policy".into(),
        ));
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
fn linux_process_block(
    proc_root: &Path,
    worktree: &Path,
    cache: &Path,
    descriptors: &[i32],
    guarded: bool,
) -> Option<CacheDisposition> {
    match process_probe::linux_evidence_many(proc_root, worktree, cache, descriptors) {
        process_probe::Evidence::Idle => None,
        process_probe::Evidence::Live { .. } => Some(CacheDisposition::LiveProcess),
        process_probe::Evidence::Opaque { pid: Some(_) } if guarded => None,
        process_probe::Evidence::Opaque { .. } => Some(CacheDisposition::Indeterminate),
    }
}

fn process_block_at(
    worktree: &Path,
    cache: &Path,
    descriptors: &[i32],
    guarded: bool,
    proc_root: &Path,
) -> Option<CacheDisposition> {
    #[cfg(target_os = "linux")]
    {
        linux_process_block(proc_root, worktree, cache, descriptors, guarded)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (guarded, proc_root);
        process_uses_many(worktree, cache, descriptors).then_some(CacheDisposition::Indeterminate)
    }
}

/// Private review directories use the same native process probe as retired
/// worker output. Positive evidence and unavailable process inventory retain it.
pub(crate) fn review_output_in_use(path: &Path, lease: &fs::File) -> bool {
    #[cfg(unix)]
    let descriptors = {
        use std::os::fd::AsRawFd;
        vec![lease.as_raw_fd()]
    };
    #[cfg(not(unix))]
    let descriptors = {
        let _ = lease;
        Vec::new()
    };
    #[cfg(target_os = "linux")]
    {
        process_probe::linux_uses_many_owned(Path::new("/proc"), path, path, &descriptors, true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        process_uses_many(path, path, &descriptors)
    }
}

fn live_process_uses(worktree: &Path, cache: &Path) -> bool {
    process_uses(worktree, cache, None)
}

fn output_in_use(cache: &Path, eviction_lock: &fs::File) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        process_uses(cache, cache, Some(eviction_lock.as_raw_fd()))
    }
    #[cfg(not(unix))]
    {
        let _ = (cache, eviction_lock);
        true
    }
}

fn process_uses(worktree: &Path, cache: &Path, own_lock_fd: Option<i32>) -> bool {
    let descriptors: Vec<_> = own_lock_fd.into_iter().collect();
    process_uses_many(worktree, cache, &descriptors)
}

#[cfg(target_os = "macos")]
fn process_uses_many(worktree: &Path, cache: &Path, own_lock_fds: &[i32]) -> bool {
    // Unknown/inaccessible evidence must preserve the cache. Include fd fields
    // to exempt precisely the eviction lock, while keeping this process's
    // executable, mappings and every other output handle visible.
    let Ok(output) = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-F0pfn"])
        .output()
    else {
        return true;
    };
    if !output.status.success() || !output.stderr.is_empty() {
        return true;
    }
    if lsof_uses_many(&output.stdout, worktree, cache, own_lock_fds) {
        return true;
    }
    // A build may name a target in argv before opening any file in it.
    // Keep this in the existing native probe, matching Linux cmdline checks.
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,command="])
        .output()
    else {
        return true;
    };
    !output.status.success()
        || !output.stderr.is_empty()
        || process_arguments_use(&output.stdout, worktree, cache)
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn process_arguments_use(output: &[u8], worktree: &Path, cache: &Path) -> bool {
    let mut seen = false;
    for line in output.split(|byte| *byte == b'\n').filter(|line| !line.iter().all(u8::is_ascii_whitespace)) {
        let line = line.trim_ascii_start();
        let split = line
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(line.len());
        if split == line.len() || line[split..].iter().all(u8::is_ascii_whitespace) {
            return true;
        }
        let Some(pid) = std::str::from_utf8(&line[..split])
            .ok()
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            return true;
        };
        seen = true;
        // The invoking GC's argv may carry the explicit path itself. Its
        // handles, mappings and cwd are still checked above, without waiver.
        if pid == std::process::id() { continue; }
        if [worktree, cache].iter().any(|path| {
            let path = path.as_os_str().as_encoded_bytes();
            !path.is_empty() && line[split..].windows(path.len()).any(|window| window == path)
        }) { return true; }
    }
    !seen
}

#[cfg(all(test, unix))]
fn lsof_uses(output: &[u8], worktree: &Path, cache: &Path, own_lock_fd: Option<i32>) -> bool {
    let descriptors: Vec<_> = own_lock_fd.into_iter().collect();
    lsof_uses_many(output, worktree, cache, &descriptors)
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn lsof_uses_many(output: &[u8], worktree: &Path, cache: &Path, own_lock_fds: &[i32]) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let mut pid = None;
    let mut fd = None;
    for field in output.split(|byte| *byte == 0) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        match field.first() {
            Some(b'p') => {
                pid = std::str::from_utf8(&field[1..])
                    .ok()
                    .and_then(|value| value.parse::<u32>().ok());
                fd = None;
                if pid.is_none() {
                    return true;
                }
            }
            Some(b'f') => {
                fd = std::str::from_utf8(&field[1..])
                    .ok()
                    .and_then(|value| value.parse::<i32>().ok());
            }
            Some(b'n') => {
                if pid.is_none() {
                    return true;
                }
                if pid == Some(std::process::id())
                    && fd.is_some_and(|fd| own_lock_fds.contains(&fd))
                {
                    continue;
                }
                let path = Path::new(std::ffi::OsStr::from_bytes(&field[1..]));
                if path.starts_with(worktree) || path.starts_with(cache) {
                    return true;
                }
            }
            _ => {}
        }
    }
    pid.is_none()
}

#[cfg(target_os = "linux")]
fn process_uses_many(worktree: &Path, cache: &Path, own_lock_fds: &[i32]) -> bool {
    process_probe::linux_uses_many(Path::new("/proc"), worktree, cache, own_lock_fds)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_uses_many(_worktree: &Path, _cache: &Path, _own_lock_fds: &[i32]) -> bool {
    true
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

#[cfg(all(test, unix))]
fn filesystem_capacity(path: &Path) -> io::Result<(u64, u64)> {
    crate::fs_space::fs_space(path).map(filesystem_capacity_from_space)
}

fn filesystem_capacity_from_space(space: crate::fs_space::FsSpace) -> (u64, u64) {
    (space.total_bytes, space.available_bytes)
}

#[cfg(all(test, not(unix)))]
fn filesystem_capacity(_path: &Path) -> io::Result<(u64, u64)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "target-cache capacity reporting requires statvfs",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn reclamation_available() -> bool {
        // No process can reference this fresh, nonexistent output path. A live
        // result means the native process table cannot establish idle safely.
        let probe = tempfile::tempdir().unwrap();
        let path = probe.path().join("nonexistent-output");
        let available = !live_process_uses(&path, &path);
        if !available {
            eprintln!(
                "RECLAIM PROOF UNAVAILABLE: native process evidence is unreadable; asserting preservation only"
            );
        }
        available
    }

    pub(super) fn git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git must be available for worktree discovery coverage");
        assert!(
            output.status.success(),
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

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
            scan_elapsed_ms: 0,
            newest_write_unix_secs: Some(newest),
            disposition: CacheDisposition::Eligible,
            reason: String::new(),
            interrupted_cleanup: false,
            ownership: CacheOwnership::Durable,
            release_warm_on_pressure: false,
            git_admin_dir: None,
            git_branch: None,
            git_commit: None,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scratch_root_gc_inventories_and_reclaims_dead_targets_cas_9725() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().join("repo");
        let cas = repo.join(".cas");
        let scratch = env.home().join("separate-scratch");
        fs::create_dir_all(cas.join("worktrees")).unwrap();
        fs::write(
            cas.join("config.toml"),
            format!(
                "[factory]\nmax_concurrent_builders = 4\nscratch_root = {:?}\n",
                scratch.display().to_string()
            ),
        )
        .unwrap();
        let mut targets = Vec::new();
        for (name, age) in [("old", 300), ("warm", 200), ("live", 100)] {
            let worker = cas.join("worktrees").join(name);
            fs::create_dir_all(&worker).unwrap();
            fs::write(worker.join("source.txt"), "keep source").unwrap();
            drop(owner::acquire(&cas, &worker).unwrap().unwrap());
            let target = crate::factory_scratch::worker_target_dir(&worker);
            fs::create_dir_all(target.join("debug")).unwrap();
            fs::write(target.join("debug/.cargo-lock"), "").unwrap();
            fs::write(target.join("debug/output"), vec![0u8; 32]).unwrap();
            let time = filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(age));
            for entry in WalkDir::new(&target).contents_first(true) {
                filetime::set_file_mtime(entry.unwrap().path(), time).unwrap();
            }
            targets.push((worker, target));
        }
        // A held lifetime lease protects output even without a registry row.
        let _live_lease = owner::acquire(&cas, &targets[2].0).unwrap().unwrap();
        let live = Vec::new();
        let orphan = targets[0].1.parent().unwrap().join("missing-checkout");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("output"), vec![0u8; 32]).unwrap();
        let unmanaged = scratch.join("verify-leftover");
        fs::create_dir_all(&unmanaged).unwrap();
        fs::write(unmanaged.join("evidence"), "review only").unwrap();
        let policy = TargetCachePolicy {
            high_watermark_percent: 85,
            low_watermark_percent: 75,
            min_idle_secs: 60,
            retention_count: 1,
        };
        // Below the watermark, but four builders cannot reserve 40 GiB.
        let probe = |path: &Path, policy| {
            assert_eq!(path, scratch, "GC must probe the admission filesystem");
            Ok(capacity_from_bytes(
                100 * 1024u64.pow(3),
                16 * 1024u64.pow(3),
                policy,
            ))
        };
        let mut report = inspect_with_probe(&cas, policy, &[], &live, false, probe).unwrap();
        assert_eq!(report.filesystem_path, scratch);
        assert_eq!(report.filesystem.used_percent, 84);
        assert!(
            report.filesystem.pressure,
            "projected admission pressure must trigger GC"
        );
        let dispositions: Vec<_> = targets
            .iter()
            .map(|(_, path)| {
                report
                    .caches
                    .iter()
                    .find(|record| record.path == *path)
                    .unwrap_or_else(|| {
                        panic!("scratch target missing from inventory: {}", path.display())
                    })
                    .disposition
            })
            .collect();
        assert_eq!(
            dispositions,
            [
                CacheDisposition::Selected,
                CacheDisposition::Retained,
                CacheDisposition::LiveProcess
            ]
        );
        assert!(report.caches.iter().all(|record| record.bytes >= 32));
        assert_eq!(report.caches.iter().find(|record| record.path == orphan).unwrap().disposition,
            CacheDisposition::OwnershipUncertain);
        let cargo = OpenOptions::new().read(true).write(true).open(targets[0].1.join("debug/.cargo-lock")).unwrap();
        cargo.lock_exclusive().unwrap();
        cleanup_selected(&cas, &mut report, policy, &live).unwrap();
        assert!(targets[0].1.exists(), "contended Cargo output must survive");
        drop(cargo);
        report = inspect_with_probe(&cas, policy, &[], &live, false, probe).unwrap();
        cleanup_selected(&cas, &mut report, policy, &live).unwrap();
        assert!(!targets[0].1.exists(), "dead scratch target must be reclaimed");
        assert!(report.reclaimed_bytes >= 32);
        assert!(targets[1].1.exists(), "warm cache retained");
        assert!(targets[2].1.exists(), "live cache retained");
        assert_eq!(fs::read_to_string(targets[0].0.join("source.txt")).unwrap(), "keep source");
        assert_eq!(fs::read_to_string(unmanaged.join("evidence")).unwrap(), "review only");
        assert!(orphan.exists(), "unknown checkout must remain review-only");
        // Resume an interrupted scratch quarantine without crossing devices.
        let quarantine = targets[1].1.with_file_name(format!("{QUARANTINE_PREFIX}warm--{}", uuid::Uuid::new_v4()));
        fs::rename(&targets[1].1, &quarantine).unwrap();
        report = inspect_with_probe(&cas, policy, &[], &live, false, probe).unwrap();
        assert_eq!(report.caches.iter().find(|record| record.path == quarantine).unwrap().disposition,
            CacheDisposition::Selected);
        cleanup_selected(&cas, &mut report, policy, &live).unwrap();
        assert!(!quarantine.exists(), "interrupted scratch quarantine must be reclaimed");
        let rendered = crate::factory_scratch::render_relocated(&cas);
        assert!(rendered.contains(&unmanaged.display().to_string()), "unmanaged scratch review item absent: {rendered}");
    }

    #[cfg(unix)]
    #[test]
    fn lsof_output_handles_paths_process_identity_and_unknown_evidence_cas_29b0() {
        let cache = Path::new("/repo with spaces/target/debug");
        let other = std::process::id() + 1;
        let artifact = format!("p{other}\0\nf18\0n/repo with spaces/target/debug/output\0");
        assert!(lsof_uses(artifact.as_bytes(), cache, cache, Some(17)));
        assert!(!lsof_uses(
            b"p42\0\nf18\0n/repo with spaces/target/debugger/unrelated\0",
            cache,
            cache,
            None
        ));
        let own_lock = format!(
            "p{}\0\nf17\0n/repo with spaces/target/debug/.cargo-lock\0",
            std::process::id()
        );
        assert!(!lsof_uses(own_lock.as_bytes(), cache, cache, Some(17)));
        assert!(lsof_uses(own_lock.as_bytes(), cache, cache, None));
        let own_output = format!("{own_lock}\nf18\0n/repo with spaces/target/debug/output\0");
        assert!(lsof_uses(own_output.as_bytes(), cache, cache, Some(17)));
        let own_exe = format!("{own_lock}\nftxt\0n/repo with spaces/target/debug/binary\0");
        assert!(lsof_uses(own_exe.as_bytes(), cache, cache, Some(17)));
        assert!(lsof_uses(b"", cache, cache, Some(17)));
        assert!(lsof_uses(b"pinvalid\0", cache, cache, Some(17)));
        assert!(process_arguments_use(format!("{other} cargo --target-dir /tmp/cache/debug\n").as_bytes(), Path::new("/tmp/worktree"), Path::new("/tmp/cache")));
        assert!(!process_arguments_use(format!("{other} sleep 30\n").as_bytes(), Path::new("/tmp/worktree"), Path::new("/tmp/cache")));
        assert!(process_arguments_use(b"", cache, cache));

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
    fn terminal_dead_worker_loses_warm_reservation_only_under_pressure_cas_04a9() {
        let temp = tempfile::tempdir().unwrap();
        let cas = temp.path().canonicalize().unwrap().join(".cas");
        let worker = cas.join("worktrees/dead-worker");
        let target = worker.join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/output"), b"rebuildable").unwrap();
        fs::write(target.join("debug/.cargo-lock"), b"").unwrap();
        let mut owner = cas_types::Agent::new("dead-owner".into(), "dead-worker".into());
        owner.pid = Some(i32::MAX as u32);
        owner.pid_starttime = Some(1);
        owner.status = cas_types::AgentStatus::Shutdown;
        owner
            .metadata
            .insert("clone_path".into(), worker.display().to_string());
        crate::store::open_agent_store(&cas)
            .unwrap()
            .register(&owner)
            .unwrap();
        let tasks = crate::store::open_task_store(&cas).unwrap();
        let mut task = cas_types::Task::new("cas-dead".into(), "finished lane".into());
        task.assignee = Some(owner.id.clone());
        tasks.add(&task).unwrap();
        let pressure = TargetCachePolicy {
            high_watermark_percent: 1,
            low_watermark_percent: 0,
            min_idle_secs: 0,
            retention_count: 1,
        };
        // cas-9725 counts projected builder growth (10 GiB per lane) against
        // free space, so the real host disk decides "normal". Pin a roomy
        // filesystem: 1% used, only the 1% watermark is under pressure.
        let gib = 1024u64.pow(3);
        let roomy = |_: &Path, policy| Ok(capacity_from_bytes(1000 * gib, 999 * gib, policy));
        let report = inspect_with_probe(&cas, pressure, &[], &[], true, roomy).unwrap();
        assert_eq!(
            report.caches[0].disposition,
            CacheDisposition::Retained,
            "nonterminal work keeps its warm reservation"
        );
        task.status = cas_types::TaskStatus::Closed;
        tasks.update(&task).unwrap();
        let normal = inspect_with_probe(
            &cas,
            TargetCachePolicy {
                high_watermark_percent: 100,
                low_watermark_percent: 99,
                ..pressure
            },
            &[],
            &[],
            true,
            roomy,
        )
        .unwrap();
        assert!(!normal.filesystem.pressure);
        assert_eq!(normal.caches[0].disposition, CacheDisposition::Retained);
        let mut report = inspect_with_probe(&cas, pressure, &[], &[], true, roomy).unwrap();
        assert!(report.filesystem.pressure);
        assert_eq!(report.caches[0].bytes, 11);
        assert_eq!(
            report.caches[0].disposition,
            CacheDisposition::Selected,
            "terminal-owned dead lane must yield under pressure"
        );
        assert_eq!(report.selected_bytes, 11);
        cleanup_selected(&cas, &mut report, pressure, &[]).unwrap();
        assert!(
            target.join("debug/output").is_file(),
            "dry run must preserve bytes"
        );
        task.status = cas_types::TaskStatus::InProgress;
        tasks.update(&task).unwrap();
        report.dry_run = false;
        cleanup_selected(&cas, &mut report, pressure, &[]).unwrap();
        assert_ne!(
            report.caches[0].disposition,
            CacheDisposition::Reclaimed,
            "task reopening invalidates pressure exemption"
        );
        assert!(target.exists());
        task.status = cas_types::TaskStatus::Closed;
        tasks.update(&task).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.join("debug/.cargo-lock"))
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        let mut report = inspect_with_probe(&cas, pressure, &[], &[], false, roomy).unwrap();
        cleanup_selected(&cas, &mut report, pressure, &[]).unwrap();
        assert_ne!(
            report.caches[0].disposition,
            CacheDisposition::Reclaimed,
            "held Cargo lock must protect even a terminal dead lane"
        );
        assert!(target.exists());
        FileExt::unlock(&lock).unwrap();
        drop(lock);
        let mut report = inspect_with_probe(&cas, pressure, &[], &[], false, roomy).unwrap();
        cleanup_selected(&cas, &mut report, pressure, &[]).unwrap();
        assert_eq!(report.caches[0].disposition, CacheDisposition::Reclaimed);
        assert!(!target.exists());
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
    fn filesystem_capacity_reports_total_and_unprivileged_available_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let (total_bytes, available_bytes) = filesystem_capacity(temp_root.as_path())
            .expect("statvfs should inspect a temp directory");

        assert!(total_bytes > 0);
        assert!(
            available_bytes <= total_bytes,
            "f_bavail-derived capacity cannot exceed total capacity"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_capacity_rejects_nul_path_as_invalid_input() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/tmp/bad\0path"));
        let error = filesystem_capacity(path).expect_err("NUL path must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn filesystem_capacity_uses_bavail_not_bfree() {
        let capacity = filesystem_capacity_from_space(crate::fs_space::FsSpace {
            total_bytes: 1_000,
            available_bytes: 200,
            free_bytes: 300,
        });

        assert_eq!(capacity, (1_000, 200));
        assert_ne!(
            capacity.1, 300,
            "factory pressure must use unprivileged-available bytes, not all free bytes"
        );
    }

    #[cfg(unix)]
    #[test]
    // pin: Read the sentinel fixture to prove cleanup never removed or rewrote source outside the target cache.
    fn symlink_target_escape_is_reported_and_never_removed() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let worktree = temp_root.as_path().join("worker");
        let outside = temp_root.as_path().join("source");
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

        let linked_worktree = temp_root.as_path().join("linked-worker");
        fs::create_dir_all(outside.join("target")).unwrap();
        symlink(&outside, &linked_worktree).unwrap();
        let linked = scan_cache(&linked_worktree, &linked_worktree.join("target"), false).unwrap();
        assert!(linked.unsafe_reason.is_some());
        assert!(outside.join("target").exists());
    }

    #[test]
    fn interrupted_quarantine_is_discovered_without_touching_source() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let cas_root = temp_root.as_path().join(".cas");
        let worker = cas_root.join("worktrees/dead-worker");
        let quarantine = worker.join(format!("{QUARANTINE_PREFIX}123-1"));
        fs::create_dir_all(&quarantine).unwrap();
        fs::write(quarantine.join("artifact.rlib"), b"artifact").unwrap();
        fs::write(worker.join("source.rs"), b"source").unwrap();

        let caches = discover_caches(temp_root.as_path(), &cas_root, &[]);
        assert_eq!(caches.len(), 1);
        assert!(caches[0].interrupted_cleanup);
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), b"source");
    }

    #[test]
    fn configured_or_store_known_worktree_outside_default_root_is_discovered() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let cas_root = temp_root.as_path().join("repo/.cas");
        let custom = temp_root.as_path().join("scratch-worktrees/custom-worker");
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
    fn git_managed_epic_and_release_worktree_targets_are_discovered_but_unrelated_are_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let repo = temp_root.as_path().join("repo");
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "cassy@example.invalid"]);
        git(&repo, &["config", "user.name", "Cassy"]);
        fs::write(repo.join("README"), b"fixture").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "--quiet", "-m", "fixture"]);
        fs::create_dir_all(&cas_root).unwrap();

        let epic = temp_root.as_path().join("external-epic");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "epic/cas-868-gc",
                epic.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(epic.join("target")).unwrap();
        fs::write(epic.join("target/epic-artifact"), b"epic").unwrap();

        let release = cas_root.join("release-v3.25.5");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                release.to_str().unwrap(),
                "HEAD",
            ],
        );
        fs::create_dir_all(release.join("target")).unwrap();
        fs::write(release.join("target/release-artifact"), b"release").unwrap();

        let user_release = temp_root.as_path().join("external-release");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "release/user-owned",
                user_release.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(user_release.join("target")).unwrap();
        fs::write(user_release.join("target/user-artifact"), b"user").unwrap();

        let unrelated = temp_root.as_path().join("feature-worktree");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feature/unrelated",
                unrelated.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(unrelated.join("target")).unwrap();
        fs::write(unrelated.join("target/unrelated-artifact"), b"noise").unwrap();

        let caches = discover_caches(&repo, &cas_root, &[]);
        let worktrees: HashSet<_> = caches.iter().map(|cache| cache.worktree.clone()).collect();
        assert!(worktrees.contains(&epic.canonicalize().unwrap()));
        assert!(worktrees.contains(&release.canonicalize().unwrap()));
        assert!(worktrees.contains(&user_release.canonicalize().unwrap()));
        assert!(!worktrees.contains(&unrelated.canonicalize().unwrap()));
        assert_eq!(caches.len(), 3);

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
        assert_eq!(report.caches.len(), 3);
        assert_eq!(report.candidate_bytes, 15);
        if reclamation_available() {
            assert!(report.caches.iter().any(|cache| {
                cache.worktree == epic.canonicalize().unwrap()
                    && cache.disposition == CacheDisposition::OwnershipUncertain
            }));
            assert!(report.caches.iter().any(|cache| {
                cache.worktree == user_release.canonicalize().unwrap()
                    && cache.disposition == CacheDisposition::OwnershipUncertain
            }));
            assert!(report.caches.iter().any(|cache| {
                cache.worktree == release.canonicalize().unwrap()
                    && cache.disposition == CacheDisposition::Selected
            }));
            assert_eq!(report.selected_bytes, 7);
        } else {
            // Without process liveness evidence, GC stays report-only.
            assert!(
                report
                    .caches
                    .iter()
                    .all(|cache| cache.disposition == CacheDisposition::Indeterminate)
            );
            assert_eq!(report.selected_bytes, 0);
        }
        for (path, artifact, expected) in [
            (&epic, "epic-artifact", b"epic".as_slice()),
            (&release, "release-artifact", b"release".as_slice()),
            (&user_release, "user-artifact", b"user".as_slice()),
        ] {
            assert_eq!(
                fs::read(path.join("target").join(artifact)).unwrap(),
                expected
            );
            assert_eq!(fs::read(path.join("README")).unwrap(), b"fixture");
        }
    }

    #[test]
    fn stale_git_worktree_path_reuse_is_not_treated_as_owned() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let repo = temp_root.as_path().join("repo");
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "cassy@example.invalid"]);
        git(&repo, &["config", "user.name", "Cassy"]);
        fs::write(repo.join("README"), b"fixture").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "--quiet", "-m", "fixture"]);
        fs::create_dir_all(&cas_root).unwrap();

        let reused = temp_root.as_path().join("reused-epic");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "epic/cas-868-reused",
                reused.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&reused).unwrap();
        fs::create_dir_all(reused.join("target")).unwrap();
        fs::write(reused.join("target/unrelated-artifact"), b"keep").unwrap();

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
        assert!(report.caches.is_empty());
        assert!(reused.join("target/unrelated-artifact").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cleanup_rechecks_git_ownership_after_report_and_deserialization_before_rename() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let repo = temp_root.as_path().join("repo");
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "cassy@example.invalid"]);
        git(&repo, &["config", "user.name", "Cassy"]);
        fs::write(repo.join("README"), b"fixture").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "--quiet", "-m", "fixture"]);
        fs::create_dir_all(&cas_root).unwrap();

        let owned = temp_root.as_path().join("owned-epic");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "epic/cas-868-owned",
                owned.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(owned.join("target")).unwrap();
        fs::write(owned.join("target/before-replacement"), b"old").unwrap();

        let policy = TargetCachePolicy {
            high_watermark_percent: 1,
            low_watermark_percent: 0,
            min_idle_secs: 0,
            retention_count: 0,
        };
        let mut report =
            inspect(&cas_root, policy, std::slice::from_ref(&owned), &[], false).unwrap();
        assert_eq!(report.caches.len(), 1);
        assert_eq!(report.caches[0].bytes, 3);
        // Model a selected report predating ownership replacement even when
        // this host cannot establish idle; ownership is checked before probes.
        report.caches[0].disposition = CacheDisposition::Selected;
        let serialized = serde_json::to_string(&report).unwrap();
        let mut deserialized: TargetCacheReport = serde_json::from_str(&serialized).unwrap();
        let mut trusted_report = report;
        git(
            &repo,
            &["worktree", "remove", "--force", owned.to_str().unwrap()],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                owned.to_str().unwrap(),
                "HEAD",
            ],
        );
        fs::create_dir_all(owned.join("target")).unwrap();
        fs::write(owned.join("target/after-replacement"), b"keep").unwrap();

        cleanup_selected(&cas_root, &mut trusted_report, policy, &[]).unwrap();
        assert_eq!(
            trusted_report.caches[0].disposition,
            CacheDisposition::OwnershipChanged
        );
        cleanup_selected(&cas_root, &mut deserialized, policy, &[]).unwrap();
        assert_eq!(
            deserialized.caches[0].disposition,
            CacheDisposition::OwnershipChanged
        );
        assert_eq!(
            fs::read(owned.join("target/after-replacement")).unwrap(),
            b"keep"
        );
        assert_eq!(trusted_report.reclaimed_bytes, 0);
        assert_eq!(deserialized.reclaimed_bytes, 0);
    }

    // Git-linked worktrees are task-owned fixtures. The explicit live root
    // models an active worker; the detached release checkout is stale and is
    // safe to reclaim after normal liveness/recency revalidation.
    #[cfg(target_os = "linux")]
    #[test]
    fn git_discovered_stale_release_target_is_reclaimed_while_active_epic_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let reclaim = reclamation_available();
        let repo = temp_root.as_path().join("repo");
        let cas_root = repo.join(".cas");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "cassy@example.invalid"]);
        git(&repo, &["config", "user.name", "Cassy"]);
        fs::write(repo.join("README"), b"fixture").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "--quiet", "-m", "fixture"]);
        fs::create_dir_all(&cas_root).unwrap();

        let epic = temp_root.as_path().join("external-epic");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "epic/cas-868-active",
                epic.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(epic.join("target")).unwrap();
        fs::write(epic.join("target/active-artifact"), b"keep").unwrap();

        let release = cas_root.join("release-v3.25.5");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                release.to_str().unwrap(),
                "HEAD",
            ],
        );
        fs::create_dir_all(release.join("target")).unwrap();
        fs::write(release.join("target/stale-artifact"), b"drop").unwrap();
        let stale_bytes = fs::read(release.join("target/stale-artifact")).unwrap();
        let sources: Vec<_> = [&epic, &release]
            .into_iter()
            .map(|path| (path.clone(), fs::read(path.join("README")).unwrap()))
            .collect();

        let policy = TargetCachePolicy {
            high_watermark_percent: 1,
            low_watermark_percent: 0,
            min_idle_secs: 0,
            retention_count: 0,
        };
        let mut report =
            inspect(&cas_root, policy, &[], std::slice::from_ref(&epic), false).unwrap();
        assert_eq!(report.caches.len(), 2);
        assert!(report.caches.iter().any(|cache| {
            cache.worktree == epic.canonicalize().unwrap()
                && cache.disposition == CacheDisposition::LiveProcess
        }));
        assert!(report.caches.iter().any(|cache| {
            cache.worktree == release.canonicalize().unwrap()
                && cache.disposition
                    == if reclaim {
                        CacheDisposition::Selected
                    } else {
                        CacheDisposition::Indeterminate
                    }
        }));
        assert_eq!(report.candidate_bytes, 8);
        assert_eq!(report.selected_bytes, if reclaim { 4 } else { 0 });

        cleanup_selected(&cas_root, &mut report, policy, std::slice::from_ref(&epic)).unwrap();
        assert_eq!(
            fs::read(epic.join("target/active-artifact")).unwrap(),
            b"keep"
        );
        assert_eq!(release.join("target").exists(), !reclaim);
        assert_eq!(report.reclaimed_bytes, if reclaim { 4 } else { 0 });
        if !reclaim {
            assert_eq!(
                fs::read(release.join("target/stale-artifact")).unwrap(),
                stale_bytes
            );
        }
        for (path, before) in sources {
            assert_eq!(fs::read(path.join("README")).unwrap(), before);
        }
    }

    // Unreadable Linux process evidence takes precedence over recency.
    #[cfg(target_os = "linux")]
    #[test]
    fn live_and_recent_caches_are_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let cas_root = temp_root.as_path().join(".cas");
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
        assert_eq!(recent_report.caches.len(), 1);
        assert_eq!(recent_report.candidate_bytes, 8);
        assert_eq!(recent_report.selected_bytes, 0);
        assert_eq!(
            fs::read(worker.join("target/artifact")).unwrap(),
            b"artifact"
        );
    }

    // The implementation discovers process cwd through Linux `/proc`.
    #[cfg(target_os = "linux")]
    #[test]
    fn os_process_cwd_marks_cache_live_without_registry_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let cas_root = temp_root.as_path().join(".cas");
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

    // Destructive cleanup requires readable native process evidence.
    #[cfg(target_os = "linux")]
    #[test]
    fn cleanup_removes_only_target_and_resumes_interrupted_quarantine() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let reclaim = reclamation_available();
        let cas_root = temp_root.as_path().join(".cas");
        let worker = cas_root.join("worktrees/dead-worker");
        fs::create_dir_all(worker.join("target/deps")).unwrap();
        fs::write(worker.join("target/deps/artifact.rlib"), vec![0u8; 32]).unwrap();
        fs::write(worker.join("source.rs"), b"source").unwrap();
        let source = fs::read(worker.join("source.rs")).unwrap();

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
        assert_eq!(report.reclaimed_bytes, 32);
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), source);

        let quarantine = worker.join(format!("{QUARANTINE_PREFIX}interrupted"));
        fs::create_dir_all(&quarantine).unwrap();
        fs::write(quarantine.join("partial"), b"partial").unwrap();
        let resume_policy = TargetCachePolicy {
            retention_count: 1,
            ..policy
        };
        let mut resumed = inspect(&cas_root, resume_policy, &[], &[], false).unwrap();
        let interrupted = resumed
            .caches
            .iter()
            .position(|cache| cache.interrupted_cleanup)
            .unwrap();
        assert_eq!(resumed.caches[interrupted].bytes, 7);
        assert_eq!(resumed.candidate_bytes, 7);
        assert_eq!(
            resumed.caches[interrupted].disposition,
            if reclaim {
                CacheDisposition::Selected
            } else {
                CacheDisposition::Indeterminate
            }
        );
        cleanup_selected(&cas_root, &mut resumed, resume_policy, &[]).unwrap();
        assert_eq!(quarantine.exists(), !reclaim);
        assert_eq!(resumed.reclaimed_bytes, if reclaim { 7 } else { 0 });
        if !reclaim {
            assert_eq!(fs::read(quarantine.join("partial")).unwrap(), b"partial");
            assert!(
                resumed
                    .caches
                    .iter()
                    .all(|cache| cache.disposition == CacheDisposition::Indeterminate)
            );
        }
        assert_eq!(fs::read(worker.join("source.rs")).unwrap(), source);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn unknown_cache_fixture() -> (
        tempfile::TempDir,
        PathBuf,
        PathBuf,
        PathBuf,
        TargetCachePolicy,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let cas = crate::store::init_cas_dir(&root).unwrap();
        let checkout = cas.join("epic-missing-gitdir");
        let target = checkout.join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(
            checkout.join(".git"),
            "gitdir: /nonexistent-cas-49b9/git-admin\n",
        )
        .unwrap();
        fs::write(checkout.join("source.rs"), b"uncommitted WIP").unwrap();
        fs::write(target.join("debug/output.rlib"), b"regenerable").unwrap();
        fs::write(target.join("debug/.cargo-lock"), b"").unwrap();
        fs::create_dir_all(cas.join("artifacts/cas-proof")).unwrap();
        fs::write(
            cas.join("artifacts/cas-proof/LEDGER.md"),
            b"durable evidence",
        )
        .unwrap();
        let retired = cas.join("merge-sweeps/assembly-target");
        fs::create_dir_all(&retired).unwrap();
        fs::write(retired.join("output.rlib"), b"scratch").unwrap();
        fs::write(
            retired.parent().unwrap().join("source.rs"),
            b"keep assembly source",
        )
        .unwrap();
        let old = SystemTime::now() - Duration::from_secs(4 * 3600);
        for cache in [&target, &retired] {
            for entry in WalkDir::new(cache).contents_first(true) {
                let path = entry.unwrap().into_path();
                fs::File::open(path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(old))
                    .unwrap();
            }
        }
        let proc = root.join("proc");
        #[cfg(target_os = "linux")]
        fs::create_dir_all(&proc).unwrap();
        let policy = TargetCachePolicy {
            min_idle_secs: 2 * 3600,
            retention_count: 0,
            ..policy()
        };
        (temp, cas, target, proc, policy)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn cleanup_unknown_fixture(
        cas: &Path,
        report: &mut TargetCacheReport,
        policy: TargetCachePolicy,
        opt_in: Option<&Path>,
        proc: &Path,
    ) {
        #[cfg(target_os = "linux")]
        cleanup_selected_using(cas, report, policy, &[], opt_in, proc).unwrap();
        #[cfg(target_os = "macos")]
        {
            // The public entry point must use native lsof/ps, never the fake
            // Linux process table returned by the shared fixture.
            let _ = proc;
            cleanup_selected_with_opt_in(cas, report, policy, &[], opt_in).unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    struct NativeHandleChild(std::process::Child);

    #[cfg(target_os = "macos")]
    impl NativeHandleChild {
        fn holding(path: &Path) -> Self {
            use std::io::BufRead;
            use std::process::{Command, Stdio};
            // FD 2 holds the artifact without putting its path in argv. The
            // readiness pipe proves the shell is running; stdin keeps it live.
            let child = Command::new("/bin/sh")
                .args(["-c", "printf 'ready\\n'; read release"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(fs::File::open(path).unwrap())
                .spawn()
                .unwrap();
            let mut child = Self(child);
            let mut ready = String::new();
            std::io::BufReader::new(child.0.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready, "ready\n");
            let output = Command::new("/usr/sbin/lsof")
                .args(["-nP", "-F0pfn", "-p", &child.0.id().to_string()])
                .output()
                .unwrap();
            assert!(
                output.status.success() && output.stderr.is_empty(),
                "native child FD observation failed"
            );
            assert!(
                lsof_uses_many(&output.stdout, path.parent().unwrap(), path, &[]),
                "child must really retain the artifact"
            );
            child
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for NativeHandleChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn cas_49b9_unknown_target_refuses_without_exact_opt_in_or_with_live_handle() {
        #[cfg(target_os = "macos")]
        if std::env::var_os("CAS_C334_DENY_LSOF").is_some() {
            // Real OS denial, scoped to this disposable subprocess. Neither
            // parser bytes nor the production command paths are substituted.
            let error = std::process::Command::new("/usr/sbin/lsof")
                .args(["-nP", "-F0pfn"])
                .output()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            let (_temp, cas, target, proc, policy) = unknown_cache_fixture();
            let mut report = inspect(&cas, policy, &[], &[], false).unwrap();
            cleanup_unknown_fixture(&cas, &mut report, policy, Some(&target), &proc);
            let row = report.caches.iter().find(|row| row.path == target).unwrap();
            assert_eq!(
                row.disposition,
                CacheDisposition::Indeterminate,
                "{}",
                row.reason
            );
            assert!(
                row.reason.contains("native process/handle evidence"),
                "{}",
                row.reason
            );
            assert_eq!(report.reclaimed_bytes, 0);
            assert_eq!(
                fs::read(target.join("debug/output.rlib")).unwrap(),
                b"regenerable"
            );
            assert_eq!(
                fs::read(target.parent().unwrap().join("source.rs")).unwrap(),
                b"uncommitted WIP"
            );
            assert_eq!(
                fs::read(cas.join("artifacts/cas-proof/LEDGER.md")).unwrap(),
                b"durable evidence"
            );
            return;
        }
        let (_temp, cas, target, proc, policy) = unknown_cache_fixture();
        for opt_in in [None, Some(target.parent().unwrap())] {
            let mut report = inspect(&cas, policy, &[], &[], false).unwrap();
            cleanup_unknown_fixture(&cas, &mut report, policy, opt_in, &proc);
            assert!(target.join("debug/output.rlib").is_file());
            assert_eq!(
                report.reclaimed_bytes, 0,
                "absent/ancestor opt-in is review only"
            );
        }
        // A child or build retaining a target handle vetoes explicit consent.
        #[cfg(target_os = "linux")]
        {
            let pid = proc.join("123");
            fs::create_dir_all(pid.join("fd")).unwrap();
            fs::write(pid.join("stat"), "123 (child) S 1 1 1 0 0 0 0 0 0").unwrap();
            std::os::unix::fs::symlink("/unrelated", pid.join("cwd")).unwrap();
            std::os::unix::fs::symlink("/bin/cat", pid.join("exe")).unwrap();
            fs::write(pid.join("cmdline"), b"cat\0").unwrap();
            fs::write(pid.join("maps"), b"").unwrap();
            std::os::unix::fs::symlink(target.join("debug/output.rlib"), pid.join("fd/7")).unwrap();
        }
        #[cfg(target_os = "macos")]
        let child = NativeHandleChild::holding(&target.join("debug/output.rlib"));
        let mut report = inspect(&cas, policy, &[], &[], false).unwrap();
        cleanup_unknown_fixture(&cas, &mut report, policy, Some(&target), &proc);
        let row = report.caches.iter().find(|row| row.path == target).unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(
            row.disposition,
            CacheDisposition::LiveProcess,
            "explicit consent cannot waive a live target handle: {}",
            row.reason
        );
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                row.disposition,
                CacheDisposition::Indeterminate,
                "native open handle must refuse: {}",
                row.reason
            );
            assert!(
                row.reason.contains("native process/handle evidence"),
                "{}",
                row.reason
            );
            drop(child);
        }
        assert!(target.exists());
        for scenario in [
            "preview",
            "recent",
            "zero-window",
            "opaque",
            "cargo-lock",
            "assembly-lock",
            "evidence",
            "symlink",
        ] {
            #[cfg(target_os = "macos")]
            if scenario == "opaque" {
                // This case depends on deprecated macOS sandbox-exec; the
                // required command below fails loudly if the tool disappears.
                let output = std::process::Command::new("/usr/bin/sandbox-exec")
                    .args(["-p", "(version 1)(allow default)(deny process-exec (literal \"/usr/sbin/lsof\"))"])
                    .arg(std::env::current_exe().unwrap())
                    .args(["--exact", "factory_target_cache::tests::cas_49b9_unknown_target_refuses_without_exact_opt_in_or_with_live_handle", "--nocapture"])
                    .env("CAS_C334_DENY_LSOF", "1")
                    .output().unwrap();
                assert!(
                    output.status.success(),
                    "opaque native child: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
                continue;
            }
            let (_temp, cas, target, proc, mut policy) = unknown_cache_fixture();
            let target = if scenario == "assembly-lock" {
                cas.join("merge-sweeps/assembly-target")
            } else {
                target
            };
            let lock_path = if scenario == "assembly-lock" {
                target.with_file_name("assembly-target.lock")
            } else {
                target.join("debug/.cargo-lock")
            };
            let held = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)
                .unwrap();
            match scenario {
                "zero-window" => policy.min_idle_secs = 0,
                "recent" => fs::write(target.join("debug/output.rlib"), b"just written").unwrap(),
                "opaque" => fs::remove_dir(&proc).unwrap(),
                "cargo-lock" | "assembly-lock" => {
                    held.lock_exclusive().unwrap();
                    // lsof may veto the handle before lock admission. Prove
                    // the real assembly/Cargo guard also refuses this lock.
                    let scan = scan_cache(target.parent().unwrap(), &target, false).unwrap();
                    let error = legacy::acquire_explicit(
                        &cas,
                        target.parent().unwrap(),
                        &target,
                        scan.snapshot.as_ref().unwrap(),
                    )
                    .err()
                    .expect("held lock must refuse admission");
                    let expected = if scenario == "assembly-lock" {
                        "assembly target lease"
                    } else {
                        "Cargo lock"
                    };
                    assert!(error.to_string().contains(expected), "{scenario}: {error}");
                }
                "evidence" => {
                    let alias = cas.join("artifact-alias");
                    std::os::unix::fs::symlink(&target, &alias).unwrap();
                    fs::write(
                        cas.join("config.toml"),
                        format!(
                            "[factory]\nartifacts_root = {:?}\n",
                            alias.join("proof").to_str().unwrap()
                        ),
                    )
                    .unwrap();
                }
                "symlink" => std::os::unix::fs::symlink(
                    target.parent().unwrap().join("source.rs"),
                    target.join("source-link"),
                )
                .unwrap(),
                _ => {}
            }
            // An open, unlocked fixture descriptor is itself live native
            // evidence. Keep a descriptor only for the two lock scenarios.
            let _held = if matches!(scenario, "cargo-lock" | "assembly-lock") {
                Some(held)
            } else {
                drop(held);
                None
            };
            let mut report = inspect(&cas, policy, &[], &[], scenario == "preview").unwrap();
            cleanup_unknown_fixture(&cas, &mut report, policy, Some(&target), &proc);
            let row = report.caches.iter().find(|row| row.path == target).unwrap();
            let expected = match scenario {
                "preview" => CacheDisposition::OwnershipUncertain,
                "recent" | "zero-window" => CacheDisposition::RecentWrite,
                "opaque" | "cargo-lock" | "assembly-lock" => CacheDisposition::Indeterminate,
                _ => CacheDisposition::UnsafePath,
            };
            assert_eq!(row.disposition, expected, "{scenario}: {}", row.reason);
            assert!(target.exists(), "{scenario}: refusal must preserve target");
            assert_eq!(report.reclaimed_bytes, 0);
            let (artifact, bytes): (_, &[u8]) = match scenario {
                "recent" => (target.join("debug/output.rlib"), b"just written"),
                "assembly-lock" => (target.join("output.rlib"), b"scratch"),
                _ => (target.join("debug/output.rlib"), b"regenerable"),
            };
            assert_eq!(
                fs::read(artifact).unwrap(),
                bytes,
                "{scenario}: artifact bytes changed"
            );
            let source: &[u8] = if scenario == "assembly-lock" {
                b"keep assembly source"
            } else {
                b"uncommitted WIP"
            };
            assert_eq!(
                fs::read(target.parent().unwrap().join("source.rs")).unwrap(),
                source
            );
            assert_eq!(
                fs::read(cas.join("artifacts/cas-proof/LEDGER.md")).unwrap(),
                b"durable evidence"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn cas_49b9_exact_unknown_target_opt_in_reclaims_only_quiet_regenerable_subtree() {
        let (_temp, cas, target, proc, policy) = unknown_cache_fixture();
        let retired = cas.join("merge-sweeps/assembly-target");
        for (cache, expected_bytes) in [(&target, 11), (&retired, 7)] {
            let mut report = inspect(&cas, policy, &[], &[], false).unwrap();
            let row = report.caches.iter().find(|row| &row.path == cache).unwrap();
            assert_eq!(row.disposition, CacheDisposition::OwnershipUncertain);
            cleanup_unknown_fixture(&cas, &mut report, policy, Some(cache), &proc);
            let row = report.caches.iter().find(|row| &row.path == cache).unwrap();
            assert!(
                !cache.exists(),
                "exact opt-in plus proven quietness must reclaim {}: {:?} {}",
                cache.display(),
                row.disposition,
                row.reason
            );
            assert_eq!(row.disposition, CacheDisposition::Reclaimed);
            assert_eq!(report.reclaimed_bytes, expected_bytes);
            assert_eq!(
                report
                    .caches
                    .iter()
                    .filter(|row| row.disposition == CacheDisposition::Reclaimed)
                    .count(),
                1
            );
            if cache == &target {
                assert_eq!(
                    fs::read(retired.join("output.rlib")).unwrap(),
                    b"scratch",
                    "consent names only one cache"
                );
            }
        }
        assert_eq!(
            fs::read(retired.parent().unwrap().join("source.rs")).unwrap(),
            b"keep assembly source"
        );
        assert_eq!(
            fs::read(target.parent().unwrap().join("source.rs")).unwrap(),
            b"uncommitted WIP"
        );
        assert_eq!(
            fs::read(cas.join("artifacts/cas-proof/LEDGER.md")).unwrap(),
            b"durable evidence"
        );
    }
}
