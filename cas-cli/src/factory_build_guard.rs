//! Shared host-pressure admission for factory spawns and capped worker builds.
//!
//! Worker panes are cheap to queue but their Cargo processes are not. Keep the
//! guard here, beside the MCP spawn handler, so the decision is made before a
//! request reaches the daemon queue. The process scan is deliberately narrow:
//! only `cargo` processes whose cwd is inside this project's factory worktree
//! root count as worker builders.

use std::collections::HashSet;
use std::path::Path;

use crate::config::{BuildGuardConfig, FactoryConfig};
use crate::factory_target_cache::{FilesystemCapacity, TargetCachePolicy};

// The largest observed first-run target was 8.6 GiB (cas-94ed). Reserve growth
// per live/requested lane; the configurable target-cache watermark stays free.
const ESTIMATED_LANE_GROWTH_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// cas-f8a6: a scratch GC that reclaimed nothing is not repeated by every
/// spawn and capped check (each a separate process) until this cooldown
/// expires or free space moves materially. Capacity is always re-measured.
const ADMISSION_GC_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);
const ADMISSION_GC_STATE: &str = "build-guard-admission-gc.json";
const MATERIAL_CHANGE_MIN_BYTES: u64 = 1024 * 1024 * 1024;

/// The last admission GC for one Cassy root, keyed by its location under `cas_root`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AdmissionGcAttempt {
    at_unix_secs: u64,
    reclaimed_bytes: u64,
    total_bytes: u64,
    available_bytes: u64,
}

fn unix_secs(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// True when the last attempt reclaimed nothing, is inside the cooldown, and
/// the disk has not changed materially since; any doubt runs the GC.
fn admission_gc_cooling_down(cas_root: &Path, disk: &FilesystemCapacity) -> bool {
    let Some(attempt) = std::fs::read_to_string(cas_root.join(ADMISSION_GC_STATE))
        .ok()
        .and_then(|text| serde_json::from_str::<AdmissionGcAttempt>(&text).ok())
    else {
        return false;
    };
    let now = unix_secs(std::time::SystemTime::now());
    let material = MATERIAL_CHANGE_MIN_BYTES.max(disk.total_bytes / 100);
    attempt.reclaimed_bytes == 0
        && attempt.at_unix_secs <= now
        && now - attempt.at_unix_secs < ADMISSION_GC_COOLDOWN.as_secs()
        && attempt.total_bytes == disk.total_bytes
        && attempt.available_bytes.abs_diff(disk.available_bytes) < material
}

/// Best effort and atomic: a lost record only costs one extra scan.
fn record_admission_gc(cas_root: &Path, reclaimed_bytes: u64, disk: &FilesystemCapacity) {
    let attempt = AdmissionGcAttempt {
        at_unix_secs: unix_secs(std::time::SystemTime::now()),
        reclaimed_bytes,
        total_bytes: disk.total_bytes,
        available_bytes: disk.available_bytes,
    };
    let Ok(text) = serde_json::to_string(&attempt) else {
        return;
    };
    let temporary = cas_root.join(format!(".{ADMISSION_GC_STATE}.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, text).is_ok()
        && std::fs::rename(&temporary, cas_root.join(ADMISSION_GC_STATE)).is_err()
    {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Shared projected disk admission. GC reserves the configured builder cap so
/// it can restore headroom before the next capped build is refused.
pub(crate) fn disk_pressure(disk: &FilesystemCapacity, lanes: usize) -> bool {
    let growth = ESTIMATED_LANE_GROWTH_BYTES.saturating_mul(lanes as u64);
    let available = disk.available_bytes.saturating_sub(growth);
    let reserve =
        (u128::from(disk.total_bytes) * u128::from(100 - disk.high_watermark_percent)) / 100;
    disk.pressure || u128::from(available) <= reserve
}

/// Linux PSI stall percentages and reclaimable memory, with native macOS memory.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct HostPressure {
    cpu_some_avg10: Option<f64>,
    memory_some_avg10: Option<f64>,
    available_memory_bytes: Option<u64>,
}

impl HostPressure {
    fn read(proc_root: &Path) -> Self {
        let psi = |resource: &str| -> Option<f64> {
            let text = std::fs::read_to_string(proc_root.join("pressure").join(resource)).ok()?;
            let some = text
                .lines()
                .find(|line| line.split_whitespace().next() == Some("some"))?;
            some.split_whitespace().find_map(|field| {
                field
                    .strip_prefix("avg10=")?
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            })
        };
        let available_memory_bytes = std::fs::read_to_string(proc_root.join("meminfo"))
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    let mut fields = line.split_whitespace();
                    if fields.next()? != "MemAvailable:" {
                        return None;
                    }
                    let kib = fields.next()?.parse::<u64>().ok()?;
                    if fields.next()? != "kB" {
                        return None;
                    }
                    kib.checked_mul(1024)
                })
            });
        Self {
            cpu_some_avg10: psi("cpu"),
            memory_some_avg10: psi("memory"),
            available_memory_bytes,
        }
    }

    fn notice(&self) -> String {
        let memory = self.available_memory_bytes.map_or_else(
            || "unknown".into(),
            |bytes| format!("{:.2}GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
        );
        format!(
            "cpu_psi_some_avg10={}%; memory_psi_some_avg10={}%; available_memory={memory}",
            format_load(self.cpu_some_avg10),
            format_load(self.memory_some_avg10)
        )
    }
}

/// Snapshot used in the spawn receipt and refusal diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BuildGuardSnapshot {
    pub cpu_count: usize,
    pub load_1m: Option<f64>,
    pub live_cargo_workers: usize,
    pub requested_workers: usize,
    pub requested_builders: usize,
    pub expected_build: &'static str,
    pub max_concurrent_builders: usize,
    pub disabled: bool,
    pub pressure: HostPressure,
    pub limits: BuildGuardConfig,
    pub disk: Option<Result<FilesystemCapacity, String>>,
}

impl BuildGuardSnapshot {
    fn with_disk_probe(
        mut self,
        cas_root: &Path,
        config: &FactoryConfig,
        probe: impl Fn(&Path, TargetCachePolicy) -> std::io::Result<FilesystemCapacity>,
    ) -> Self {
        let existing = crate::factory_scratch::build_filesystem(cas_root, config);
        let policy = TargetCachePolicy::from(config);
        let measure = || {
            probe(&existing, policy).map_err(|error| {
                format!(
                    "disk capacity unavailable for {}: {error}",
                    existing.display()
                )
            })
        };
        self.disk = Some(measure());
        let lanes = self
            .live_cargo_workers
            .saturating_add(self.requested_builders);
        let pressured = match &self.disk {
            Some(Ok(disk)) if self.requested_builders > 0 && disk_pressure(disk, lanes) => {
                Some(disk.clone())
            }
            _ => None,
        };
        if let Some(disk) = &pressured
            && admission_gc_cooling_down(cas_root, disk)
        {
            eprintln!(
                "Build guard scratch GC skipped: the last attempt reclaimed nothing within {}s and free space is unchanged",
                ADMISSION_GC_COOLDOWN.as_secs()
            );
        } else if pressured.is_some() {
            // One attempt per admission, with the same provenance, quiet-window,
            // warm retention and nonblocking destructive locks as manual GC.
            let reclaimed = match crate::factory_target_cache::reclaim_scratch_for_admission(
                cas_root,
                policy,
                |path, policy| {
                    // GC reloads persisted configuration. A changed or absent
                    // scratch setting must not redirect this admission's probe.
                    if path != existing {
                        return Err(std::io::Error::other(
                            "scratch filesystem changed during admission; cleanup deferred",
                        ));
                    }
                    probe(path, policy)
                },
            ) {
                Ok(report) => {
                    eprintln!(
                        "Build guard scratch GC: reclaimed {} bytes from {} target(s); warm, live and unverifiable targets retained",
                        report.reclaimed_bytes,
                        report
                            .caches
                            .iter()
                            .filter(|cache| cache.disposition
                                == crate::factory_target_cache::CacheDisposition::Reclaimed)
                            .count(),
                    );
                    Some(report.reclaimed_bytes)
                }
                Err(error) => {
                    eprintln!("Build guard scratch GC deferred: {error}");
                    None
                }
            };
            // Even a partial cleanup or competing GC may have freed capacity.
            // Never credit planned/logical bytes as actual filesystem headroom.
            self.disk = Some(measure());
            // A deferred GC is retried; only a completed attempt starts the cooldown.
            if let (Some(reclaimed), Some(Ok(disk))) = (reclaimed, &self.disk) {
                record_admission_gc(cas_root, reclaimed, disk);
            }
        }
        self
    }

    fn with_host(mut self, proc_root: &Path, config: &FactoryConfig) -> Self {
        self.load_1m = std::fs::read_to_string(proc_root.join("loadavg"))
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
            .filter(|load| load.is_finite() && *load >= 0.0);
        self.pressure = HostPressure::read(proc_root);
        #[cfg(target_os = "macos")]
        if proc_root == Path::new("/proc") {
            // Injected proc fixtures must not accidentally sample the host.
            self.load_1m = macos_load_1m();
            self.pressure.available_memory_bytes = macos_available_memory();
        }
        self.limits = config.build_guard.effective();
        self
    }

    /// Refuse host contention or a projected builder count above the cap.
    pub(crate) fn violations(&self) -> Vec<String> {
        self.admission_violations(false)
    }

    /// [`Self::violations`], optionally carrying the earlier CPU admission
    /// (cas-f616). A lane proof's second capped step carries its first
    /// step's admission: the load that step raised must not refuse its own
    /// continuation. Disk, memory pressure and the builder cap are never waived.
    pub(crate) fn admission_violations(&self, load_admitted: bool) -> Vec<String> {
        if self.disabled {
            return Vec::new();
        }
        let mut violations = Vec::new();
        if self.requested_builders > 0 {
            match &self.disk {
                Some(Ok(disk)) => {
                    let lanes = self
                        .live_cargo_workers
                        .saturating_add(self.requested_builders);
                    if disk_pressure(disk, lanes) {
                        violations.push(format!(
                            "disk {:.2}GiB available; estimated {} lane(s) x 10GiB growth would reach the {}% high watermark; free target caches before retrying",
                            disk.available_bytes as f64 / 1024_f64.powi(3),
                            lanes,
                            disk.high_watermark_percent,
                        ));
                    }
                }
                Some(Err(error)) => violations.push(format!("disk probe failed: {error}")),
                None => {}
            }
        }
        if !load_admitted {
            if let Some(cpu) = self.pressure.cpu_some_avg10 {
                if cpu > self.limits.max_cpu_pressure_percent {
                    violations.push(format!(
                        "CPU PSI some avg10 {cpu:.2}% exceeds {:.2}%",
                        self.limits.max_cpu_pressure_percent
                    ));
                }
            } else if let Some(load) = self.load_1m
                && load > self.cpu_count as f64 * self.limits.max_load_ratio
            {
                violations.push(format!(
                    "1-minute load {load:.2} exceeds {} CPUs x max_load_ratio={:.2}",
                    self.cpu_count, self.limits.max_load_ratio
                ));
            }
        }
        // Continuing a recently admitted proof never waives fresh memory safety.
        if let Some(memory) = self.pressure.memory_some_avg10
            && memory > self.limits.max_memory_pressure_percent
        {
            violations.push(format!(
                "memory PSI some avg10 {memory:.2}% exceeds {:.2}%",
                self.limits.max_memory_pressure_percent
            ));
        }
        if let Some(bytes) = self.pressure.available_memory_bytes
            && bytes < u64::from(self.limits.min_available_memory_gib) * 1024 * 1024 * 1024
        {
            violations.push(format!(
                "available memory {:.2}GiB is below {}GiB",
                bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                self.limits.min_available_memory_gib
            ));
        }
        if self.requested_builders > 0 {
            let projected = self
                .live_cargo_workers
                .saturating_add(self.requested_builders);
            if projected > self.max_concurrent_builders {
                violations.push(format!(
                    "{} live/requested Cargo builders would exceed max_concurrent_builders={}",
                    projected, self.max_concurrent_builders
                ));
            }
        }
        violations
    }

    pub(crate) fn refusal_message(&self) -> String {
        let violations = self.violations();
        if let Some(disk) = violations.iter().find(|reason| reason.starts_with("disk ")) {
            return format!(
                "Waiting for build admission: {disk}.\nRetry the same spawn_workers request after restoring disk headroom."
            );
        }
        let retry = if violations
            .iter()
            .any(|violation| violation.contains("max_concurrent_builders="))
        {
            " Retry the same spawn_workers request after a Cargo worker releases a builder slot; pass force=true only for an intentional override."
        } else {
            " Pass force=true to override this soft guard."
        };
        format!(
            "Build concurrency guard: waiting for admission; refusing spawn of {} worker(s): {}. Current state: load_1m={}, cpu_count={}, live_cargo_workers={}, requested_builders={}, expected_build={}, max_concurrent_builders={}.{}",
            self.requested_workers,
            violations.join("; "),
            format_load(self.load_1m),
            self.cpu_count,
            self.live_cargo_workers,
            self.requested_builders,
            self.expected_build,
            self.max_concurrent_builders,
            retry,
        )
    }

    pub(crate) fn receipt_notice(&self, forced: bool) -> String {
        let violations = self.violations();
        let state = if self.disabled {
            "disabled by CAS_FACTORY_BUILD_GUARD=off".to_string()
        } else if violations.is_empty() {
            "within limits".to_string()
        } else if forced {
            format!("forced override ({})", violations.join("; "))
        } else {
            violations.join("; ")
        };
        let state = format!("{state}; {}", self.pressure.notice());
        format!(
            "\nBuild guard: {state}; load_1m={} / cpu_count={}; live_cargo_workers={}; requested_workers={}; requested_builders={}; expected_build={}; max_concurrent_builders={}",
            format_load(self.load_1m),
            self.cpu_count,
            self.live_cargo_workers,
            self.requested_workers,
            self.requested_builders,
            self.expected_build,
            self.max_concurrent_builders,
        )
    }
}

/// Evaluate a supplied snapshot. Keeping this pure makes the refusal/force
/// contract testable without mutating `/proc` or relying on host load.
pub(crate) fn evaluate(
    cpu_count: usize,
    load_1m: Option<f64>,
    live_cargo_workers: usize,
    requested_workers: usize,
    max_concurrent_builders: usize,
) -> BuildGuardSnapshot {
    evaluate_with_builders(
        cpu_count,
        load_1m,
        live_cargo_workers,
        requested_workers,
        requested_workers,
        "unknown (counted conservatively)",
        max_concurrent_builders,
    )
}

/// Evaluate a spawn with its pre-assigned task's build expectation. Tasks
/// explicitly marked no-code or docs-only do not consume Cargo builder slots;
/// all other and unassigned work is counted conservatively.
pub(crate) fn evaluate_for_task(
    cpu_count: usize,
    load_1m: Option<f64>,
    live_cargo_workers: usize,
    requested_workers: usize,
    max_concurrent_builders: usize,
    task: Option<&cas_types::Task>,
) -> BuildGuardSnapshot {
    let expected_build = task_build_expectation(task);
    let requested_builders = if matches!(expected_build, "no-code" | "docs-only") {
        0
    } else {
        requested_workers
    };
    evaluate_with_builders(
        cpu_count,
        load_1m,
        live_cargo_workers,
        requested_workers,
        requested_builders,
        expected_build,
        max_concurrent_builders,
    )
}

fn evaluate_with_builders(
    cpu_count: usize,
    load_1m: Option<f64>,
    live_cargo_workers: usize,
    requested_workers: usize,
    requested_builders: usize,
    expected_build: &'static str,
    max_concurrent_builders: usize,
) -> BuildGuardSnapshot {
    BuildGuardSnapshot {
        cpu_count: cpu_count.max(1),
        load_1m,
        live_cargo_workers,
        requested_workers,
        requested_builders,
        expected_build,
        max_concurrent_builders,
        disabled: false,
        pressure: HostPressure::default(),
        limits: BuildGuardConfig::default(),
        disk: None,
    }
}

fn task_build_expectation(task: Option<&cas_types::Task>) -> &'static str {
    let Some(task) = task else {
        return "unknown (counted conservatively)";
    };
    if task.execution_note.as_deref() == Some("no-code") {
        return "no-code";
    }
    if task.labels.iter().any(|label| {
        matches!(
            label.trim().to_ascii_lowercase().replace('_', "-").as_str(),
            "docs-only" | "documentation-only"
        )
    }) {
        return "docs-only";
    }
    "yes"
}

/// Inspect the host and evaluate the requested spawn against the factory cap.
pub(crate) fn inspect(
    cas_root: &Path,
    config: &FactoryConfig,
    requested_workers: usize,
) -> BuildGuardSnapshot {
    inspect_for_task(cas_root, config, requested_workers, None)
}

/// Inspect host pressure while accounting for the pre-assigned task's build
/// expectation, when one is available.
pub(crate) fn inspect_for_task(
    cas_root: &Path,
    config: &FactoryConfig,
    requested_workers: usize,
    task: Option<&cas_types::Task>,
) -> BuildGuardSnapshot {
    let cpu_count = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    if build_guard_disabled_override() {
        let mut snapshot = evaluate_for_task(
            cpu_count,
            None,
            0,
            requested_workers,
            config.max_concurrent_builders,
            task,
        );
        snapshot.disabled = true;
        return snapshot;
    }
    evaluate_for_task(
        cpu_count,
        None,
        live_cargo_worker_count(cas_root),
        requested_workers,
        config.max_concurrent_builders,
        task,
    )
    .with_host(Path::new("/proc"), config)
    .with_disk_probe(
        cas_root,
        config,
        crate::factory_target_cache::capacity_status,
    )
}

#[cfg(target_os = "macos")]
fn macos_load_1m() -> Option<f64> {
    let mut load = 0.0;
    // SAFETY: getloadavg may write exactly one double into this valid pointer.
    (unsafe { libc::getloadavg(&mut load, 1) } == 1)
        .then_some(load)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc exposes Mach host statistics, but deprecates mach_task_self.
fn macos_available_memory() -> Option<u64> {
    unsafe extern "C" {
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: Mach returns a host send right owned by this call. The zeroed
    // buffer is correctly sized for HOST_VM_INFO64; count is its integer count.
    let status = unsafe {
        let host = libc::mach_host_self();
        let status = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        );
        mach_port_deallocate(libc::mach_task_self(), host);
        status
    };
    if status != libc::KERN_SUCCESS || count < 3 {
        return None;
    }
    // SAFETY: the buffer was zeroed and a successful call populated the first
    // fields. Free pages already include speculative pages (vm_statistics.h).
    let stats = unsafe { stats.assume_init() };
    // SAFETY: sysconf has no pointer parameters or side effects on Rust memory.
    let page_size = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
    // Inactive pages are reclaimable. Do not add speculative or purgeable
    // pages again: those overlap free/active/inactive counts.
    (u64::from(stats.free_count) + u64::from(stats.inactive_count)).checked_mul(page_size)
}

fn build_guard_disabled_override() -> bool {
    std::env::var("CAS_FACTORY_BUILD_GUARD")
        .ok()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("off"))
}

/// Render the effective worker build settings in the queued spawn receipt.
/// Numeric config/env overrides are preserved; `auto` follows the same floor
/// and fleet divisor as cas-pty's worker environment injection.
pub(crate) fn throttle_notice(
    config: &FactoryConfig,
    requested_workers: usize,
    guard: &BuildGuardSnapshot,
) -> String {
    let configured_jobs = std::env::var("CAS_FACTORY_CARGO_BUILD_JOBS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.cargo_build_jobs.clone());
    let jobs = if configured_jobs.trim().parse::<usize>().is_ok() {
        configured_jobs.trim().to_string()
    } else {
        let divisor = 4usize.max(requested_workers.max(1));
        (2usize.max(guard.cpu_count / divisor)).to_string()
    };
    let nice_enabled = std::env::var("CAS_FACTORY_NICE_WORKER")
        .map(|value| value == "1")
        .unwrap_or(config.nice_cargo);
    let nice_level = std::env::var("CAS_FACTORY_NICE_LEVEL")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(10);
    let nice = if nice_enabled {
        format!("nice -n {nice_level}")
    } else {
        "disabled".to_string()
    };
    format!(
        "\nBuild throttle: CARGO_BUILD_JOBS={jobs} per worker; command priority={nice}; worker build cap={}.",
        config.max_concurrent_builders
    )
}

fn format_load(load: Option<f64>) -> String {
    load.map_or_else(|| "unknown".to_string(), |value| format!("{value:.2}"))
}

/// Count distinct worker cwd roots with a live `cargo` process. A worker can
/// have more than one cargo child during a nested command, so counting pids
/// would overstate builder concurrency. Read failures are ignored: an
/// unverifiable process must not turn a soft guard into a deadlock.
#[cfg(target_os = "linux")]
fn proc_cargo_worker_count(cas_root: &Path) -> usize {
    let worktrees = cas_root.join("worktrees");
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    let mut roots = HashSet::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|value| value.trim().to_string());
        if comm.as_deref() != Some("cargo") {
            continue;
        }
        let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        if !cwd.starts_with(&worktrees) {
            continue;
        }
        roots.insert(cwd);
    }
    roots.len()
}

#[cfg(not(target_os = "linux"))]
fn proc_cargo_worker_count(_cas_root: &Path) -> usize {
    0
}

fn live_cargo_worker_count(cas_root: &Path) -> usize {
    use fs2::FileExt;
    // OS slot locks are the portable source of live capped builders. Empty
    // lock files survive a run; only an occupied lock reserves target growth.
    let occupied = std::fs::read_dir(cas_root.join("worker-check-slots"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("slot-")
                .and_then(|name| name.strip_suffix(".lock"))
                .is_some_and(|index| index.parse::<usize>().is_ok())
        })
        .filter(|entry| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path())
                .is_ok_and(|file| match file.try_lock_exclusive() {
                    Ok(()) => false, // Closing this descriptor releases the probe lock.
                    Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
                })
        })
        .count();
    occupied.max(proc_cargo_worker_count(cas_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the filesystem capacity boundary is injected; inventory, planning,
    /// provenance, process/lifetime/Cargo locks and deletion remain real.
    #[cfg(target_os = "linux")]
    #[test]
    fn admission_reclaims_stale_scratch_and_remeasures_cas_2e6e() {
        use std::fs;
        use std::time::{Duration, SystemTime};
        let env = crate::test_support::TestEnvGuard::temp_home();
        let cas = env.home().join("repo/.cas");
        let scratch = env.home().join("scratch");
        fs::create_dir_all(cas.join("worktrees")).unwrap();
        fs::write(cas.join("config.toml"), format!(
            "[factory]\nscratch_root = {:?}\nmax_concurrent_builders = 1\ntarget_cache_min_idle_secs = 60\ntarget_cache_retention_count = 1\n",
            scratch.display().to_string()
        )).unwrap();
        let config = crate::config::Config::load(&cas).unwrap().factory();
        let mut targets = Vec::new();
        for (name, age) in [("old", 300), ("warm", 200), ("live", 100)] {
            let worker = cas.join("worktrees").join(name);
            fs::create_dir_all(&worker).unwrap();
            drop(
                crate::factory_target_cache::owner::acquire(&cas, &worker)
                    .unwrap()
                    .unwrap(),
            );
            let target = crate::factory_scratch::worker_target_dir(&worker);
            fs::create_dir_all(target.join("debug")).unwrap();
            fs::write(target.join("debug/.cargo-lock"), "").unwrap();
            fs::write(target.join("debug/output"), "regenerable").unwrap();
            let time =
                filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(age));
            for entry in walkdir::WalkDir::new(&target).contents_first(true) {
                filetime::set_file_mtime(entry.unwrap().path(), time).unwrap();
            }
            targets.push((worker, target));
        }
        let _live = crate::factory_target_cache::owner::acquire(&cas, &targets[2].0)
            .unwrap()
            .unwrap();
        let unmanaged = scratch.join("verify-unmanaged");
        fs::create_dir_all(&unmanaged).unwrap();
        fs::write(unmanaged.join("proof"), "review only").unwrap();
        let unknown = targets[0].1.parent().unwrap().join("missing-checkout");
        fs::create_dir(&unknown).unwrap();
        fs::write(unknown.join("output"), "review only").unwrap();
        let snapshot =
            evaluate(32, Some(0.0), 0, 1, 1).with_disk_probe(&cas, &config, |path, policy| {
                assert_eq!(path, scratch);
                crate::factory_target_cache::capacity_status_with(path, policy, |_| {
                    let available = if targets[0].1.exists() { 10 } else { 60 } * 1024_u64.pow(3);
                    Ok(crate::fs_space::FsSpace {
                        total_bytes: 100 * 1024_u64.pow(3),
                        available_bytes: available,
                        free_bytes: available,
                    })
                })
            });
        assert!(
            !targets[0].1.exists(),
            "admission must reclaim the stale dead-owner target itself"
        );
        assert!(
            snapshot.violations().is_empty(),
            "fresh capacity should admit: {:?}",
            snapshot.violations()
        );
        assert_eq!(snapshot.disk.unwrap().unwrap().used_percent, 40);
        assert!(targets[1].1.exists(), "warm cache must survive");
        assert!(targets[2].1.exists(), "live cache must survive");
        assert!(unknown.exists(), "unverifiable checkout must survive");
        assert_eq!(
            fs::read_to_string(unmanaged.join("proof")).unwrap(),
            "review only"
        );
    }

    /// cas-f8a6: a fruitless admission GC is not repeated on every request.
    #[cfg(target_os = "linux")]
    #[test]
    fn fruitless_admission_gc_cools_down_but_still_measures_cas_f8a6() {
        use std::cell::Cell;
        let env = crate::test_support::TestEnvGuard::temp_home();
        let cas = env.home().join("repo/.cas");
        let scratch = env.home().join("scratch");
        std::fs::create_dir_all(cas.join("worktrees")).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(
            cas.join("config.toml"),
            format!(
                "[factory]\nscratch_root = {:?}\n",
                scratch.display().to_string()
            ),
        )
        .unwrap();
        let config = crate::config::Config::load(&cas).unwrap().factory();
        let gib = 1024_u64.pow(3);
        let available = Cell::new(10 * gib);
        let probes = Cell::new(0);
        let admit = || {
            let before = probes.get();
            let snapshot =
                evaluate(32, Some(0.0), 0, 1, 1).with_disk_probe(&cas, &config, |path, policy| {
                    probes.set(probes.get() + 1);
                    crate::factory_target_cache::capacity_status_with(path, policy, |_| {
                        Ok(crate::fs_space::FsSpace {
                            total_bytes: 100 * gib,
                            available_bytes: available.get(),
                            free_bytes: available.get(),
                        })
                    })
                });
            (probes.get() - before, snapshot)
        };
        let (first, _) = admit();
        assert!(
            first > 1,
            "pressured admission runs the scratch GC: {first} probe(s)"
        );
        let (second, snapshot) = admit();
        assert_eq!(
            second, 1,
            "nothing was reclaimable: no rescan inside the cooldown"
        );
        assert!(
            snapshot
                .refusal_message()
                .contains("Waiting for build admission: disk 10.00GiB available"),
            "the refusal still reports freshly measured usage: {}",
            snapshot.refusal_message()
        );
        available.set(4 * gib);
        let (changed, _) = admit();
        assert!(
            changed > 1,
            "a material change in free bytes ends the cooldown"
        );
        let state = cas.join(ADMISSION_GC_STATE);
        let mut attempt: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
        attempt["at_unix_secs"] =
            (attempt["at_unix_secs"].as_u64().unwrap() - ADMISSION_GC_COOLDOWN.as_secs() - 1)
                .into();
        std::fs::write(&state, attempt.to_string()).unwrap();
        let (expired, _) = admit();
        assert!(expired > 1, "an expired cooldown runs the scratch GC again");
    }

    #[test]
    fn cas_94ed_disk_admission_projects_target_growth_on_scratch_volume() {
        use fs2::FileExt;
        let root = tempfile::tempdir().unwrap();
        let slots = root.path().join("worker-check-slots");
        std::fs::create_dir(&slots).unwrap();
        let slot = std::fs::File::create(slots.join("slot-0.lock")).unwrap();
        slot.lock_exclusive().unwrap();
        let live = live_cargo_worker_count(root.path());
        assert_eq!(live, 1, "portable occupied slots must reserve disk growth");
        let config = FactoryConfig {
            scratch_root: Some(root.path().join("future/targets").display().to_string()),
            ..FactoryConfig::default()
        };
        let gib = 1024_u64.pow(3);
        for (available_gib, refused) in [(40, false), (30, true), (10, true)] {
            let snapshot = evaluate(32, Some(1.0), live, 1, 4).with_disk_probe(
                Path::new("/wrong-repository-volume"),
                &config,
                |path, policy| {
                    assert_eq!(path, root.path(), "probe the actual scratch volume");
                    crate::factory_target_cache::capacity_status_with(path, policy, |_| {
                        Ok(crate::fs_space::FsSpace {
                            total_bytes: 100 * gib,
                            available_bytes: available_gib * gib,
                            free_bytes: available_gib * gib,
                        })
                    })
                },
            );
            for continuation in [false, true] {
                let violations = snapshot.admission_violations(continuation);
                assert_eq!(
                    violations.iter().any(|v| v.starts_with("disk ")),
                    refused,
                    "{available_gib}GiB: {violations:?}"
                );
            }
            if refused {
                assert!(
                    snapshot
                        .refusal_message()
                        .contains("Waiting for build admission: disk ")
                );
            }
            let no_build = BuildGuardSnapshot {
                requested_builders: 0,
                ..snapshot
            };
            assert!(
                no_build.violations().is_empty(),
                "no-code work consumes no target growth"
            );
        }
        drop(slot);
        assert_eq!(
            live_cargo_worker_count(root.path()),
            0,
            "stale slot files are not live builders"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cas_94ed_macos_native_host_probes_are_available() {
        let snapshot =
            evaluate(32, None, 0, 1, 4).with_host(Path::new("/proc"), &FactoryConfig::default());
        assert!(
            snapshot
                .load_1m
                .is_some_and(|load| load.is_finite() && load >= 0.0),
            "{snapshot:?}"
        );
        assert!(
            snapshot
                .pressure
                .available_memory_bytes
                .is_some_and(|bytes| bytes > 0),
            "{snapshot:?}"
        );
    }

    fn fake_proc(load: f64, cpu: &str, memory: &str, available_kib: u64) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("pressure")).unwrap();
        std::fs::write(root.path().join("loadavg"), format!("{load} 0 0 1/1 1\n")).unwrap();
        std::fs::write(root.path().join("pressure/cpu"), cpu).unwrap();
        std::fs::write(root.path().join("pressure/memory"), memory).unwrap();
        std::fs::write(
            root.path().join("meminfo"),
            format!("MemAvailable: {available_kib} kB\n"),
        )
        .unwrap();
        root
    }

    #[test]
    fn cas_2d63_admission_uses_proc_pressure_and_preserves_builder_cap() {
        for (label, load, cpu, memory, available, allowed) in [
            (
                "quiet PSI, noisy load",
                64.0,
                "some avg10=2.5 avg60=1 total=42",
                "some avg10=0.1",
                8 * 1024 * 1024,
                true,
            ),
            (
                "CPU contention",
                2.0,
                "some avg10=95.0",
                "some avg10=0.1",
                8 * 1024 * 1024,
                false,
            ),
            (
                "memory stalls",
                2.0,
                "some avg10=2.5",
                "some avg10=12.0",
                8 * 1024 * 1024,
                false,
            ),
            (
                "low available memory",
                2.0,
                "some avg10=2.5",
                "some avg10=0.1",
                1024 * 1024,
                false,
            ),
            ("PSI unavailable", 64.0, "", "", 8 * 1024 * 1024, false),
            (
                "invalid PSI",
                64.0,
                "some avg10=NaN",
                "some avg10=0.1",
                8 * 1024 * 1024,
                false,
            ),
        ] {
            let proc = fake_proc(load, cpu, memory, available);
            let snapshot =
                evaluate(32, None, 0, 1, 4).with_host(proc.path(), &FactoryConfig::default());
            assert_eq!(
                snapshot.violations().is_empty(),
                allowed,
                "{label}: {:?}",
                snapshot.violations()
            );
            if allowed {
                let crowded = BuildGuardSnapshot {
                    live_cargo_workers: 4,
                    ..snapshot
                };
                assert!(
                    crowded
                        .admission_violations(true)
                        .iter()
                        .any(|v| v.contains("max_concurrent_builders"))
                );
            } else if matches!(label, "memory stalls" | "low available memory") {
                assert!(
                    !snapshot.admission_violations(true).is_empty(),
                    "continuation must recheck memory: {label}"
                );
            }
        }
    }

    #[test]
    fn cas_2d63_load_fallback_honors_registered_configuration() {
        let proc = fake_proc(48.0, "", "", 8 * 1024 * 1024);
        let mut config = crate::config::Config::default();
        config
            .set("factory.build_guard.max_load_ratio", "2")
            .unwrap();
        let relaxed = evaluate(32, None, 0, 1, 4).with_host(proc.path(), &config.factory());
        assert!(
            relaxed.violations().is_empty(),
            "{:?}",
            relaxed.violations()
        );
        config
            .set("factory.build_guard.max_load_ratio", "1")
            .unwrap();
        let normal = evaluate(32, None, 0, 1, 4).with_host(proc.path(), &config.factory());
        assert!(!normal.violations().is_empty());
        let pressed = fake_proc(64.0, "some avg10=95", "some avg10=12", 1024 * 1024);
        for (key, value) in [
            ("factory.build_guard.max_cpu_pressure_percent", "100"),
            ("factory.build_guard.max_memory_pressure_percent", "20"),
            ("factory.build_guard.min_available_memory_gib", "0"),
        ] {
            config.set(key, value).unwrap();
        }
        assert!(
            evaluate(32, None, 0, 1, 4)
                .with_host(pressed.path(), &config.factory())
                .violations()
                .is_empty()
        );
        let invalid: crate::config::Config = toml::from_str("[factory.build_guard]\nmax_load_ratio = nan\nmax_cpu_pressure_percent = inf\nmax_memory_pressure_percent = nan\nmin_available_memory_gib = 70000\n").unwrap();
        assert!(
            !evaluate(32, None, 0, 1, 4)
                .with_host(proc.path(), &invalid.factory())
                .violations()
                .is_empty(),
            "hand-edited NaN cannot disable load fallback"
        );
        assert_eq!(
            evaluate(32, None, 0, 1, 4)
                .with_host(pressed.path(), &invalid.factory())
                .violations()
                .len(),
            3,
            "invalid hand-edited thresholds use safe defaults"
        );
    }

    #[test]
    fn load_above_cpu_refuses_without_force() {
        let snapshot = evaluate(8, Some(8.01), 0, 1, 4);
        assert_eq!(snapshot.violations().len(), 1);
        assert!(snapshot.refusal_message().contains("refusing spawn"));
        assert!(snapshot.refusal_message().contains("force=true"));
    }

    #[test]
    fn projected_builders_trigger_cap_and_force_is_receipted() {
        let snapshot = evaluate(32, Some(2.0), 4, 1, 4);
        assert!(
            snapshot
                .violations()
                .iter()
                .any(|violation| violation.contains("max_concurrent_builders=4"))
        );
        assert!(snapshot.receipt_notice(true).contains("forced override"));
    }

    #[test]
    fn no_code_preassigned_spawn_at_builder_cap_needs_no_force() {
        let mut task = cas_types::Task::new("cas-no-code".into(), "Triage an issue".into());
        task.execution_note = Some("no-code".into());
        let snapshot = evaluate_for_task(32, Some(2.0), 3, 1, 3, Some(&task));

        assert!(
            snapshot.violations().is_empty(),
            "{:?}",
            snapshot.violations()
        );
        assert_eq!(snapshot.requested_builders, 0);
        assert!(
            snapshot
                .receipt_notice(false)
                .contains("expected_build=no-code")
        );
    }

    #[test]
    fn docs_only_preassigned_spawn_at_builder_cap_needs_no_force() {
        let mut task = cas_types::Task::new("cas-docs-only".into(), "Document a workflow".into());
        task.labels = vec!["docs-only".into()];
        let snapshot = evaluate_for_task(32, Some(2.0), 3, 1, 3, Some(&task));

        assert!(
            snapshot.violations().is_empty(),
            "{:?}",
            snapshot.violations()
        );
        assert_eq!(snapshot.requested_builders, 0);
        assert!(
            snapshot
                .receipt_notice(false)
                .contains("expected_build=docs-only")
        );
    }

    #[test]
    fn rust_preassigned_spawn_at_builder_cap_is_refused_with_named_retry() {
        let mut task = cas_types::Task::new("cas-rust".into(), "Fix a Rust bug".into());
        task.execution_note = Some("test-first".into());
        let snapshot = evaluate_for_task(32, Some(2.0), 3, 1, 3, Some(&task));

        assert_eq!(snapshot.requested_builders, 1);
        assert!(!snapshot.violations().is_empty());
        let refusal = snapshot.refusal_message();
        assert!(refusal.contains("expected_build=yes"), "{refusal}");
        assert!(
            refusal.contains(
                "Retry the same spawn_workers request after a Cargo worker releases a builder slot"
            ),
            "{refusal}"
        );
    }

    #[test]
    fn healthy_snapshot_has_no_guard_violation() {
        let snapshot = evaluate(32, Some(1.0), 2, 1, 4);
        assert!(snapshot.violations().is_empty());
    }

    #[test]
    fn disabled_override_neutralizes_live_probe_for_test_fixtures() {
        let loaded_snapshot = evaluate(1, Some(2.0), 0, 1, 4);
        assert!(
            !loaded_snapshot.violations().is_empty(),
            "the injected loaded snapshot must exercise the refusal path"
        );

        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let snapshot = inspect(
            Path::new("/nonexistent-cas-root"),
            &FactoryConfig::default(),
            1,
        );
        assert!(snapshot.disabled);
        assert!(snapshot.violations().is_empty());
        assert!(snapshot.receipt_notice(false).contains("disabled"));
    }

    #[test]
    fn throttle_notice_records_jobs_and_nice() {
        let config = FactoryConfig::default();
        let snapshot = evaluate(32, Some(1.0), 0, 4, 4);
        let notice = throttle_notice(&config, 4, &snapshot);
        assert!(notice.contains("CARGO_BUILD_JOBS=8"), "{notice}");
        assert!(notice.contains("nice -n 10"), "{notice}");
        assert!(notice.contains("worker build cap=4"), "{notice}");
    }
}
