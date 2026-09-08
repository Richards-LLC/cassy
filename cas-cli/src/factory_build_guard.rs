//! Spawn-time protection against a factory build storm.
//!
//! Worker panes are cheap to queue but their Cargo processes are not. Keep the
//! guard here, beside the MCP spawn handler, so the decision is made before a
//! request reaches the daemon queue. The process scan is deliberately narrow:
//! only `cargo` processes whose cwd is inside this project's factory worktree
//! root count as worker builders.

use std::collections::HashSet;
use std::path::Path;

use crate::config::FactoryConfig;

/// Snapshot used in the spawn receipt and refusal diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BuildGuardSnapshot {
    pub cpu_count: usize,
    pub load_1m: Option<f64>,
    pub live_cargo_workers: usize,
    pub requested_workers: usize,
    pub max_concurrent_builders: usize,
    pub disabled: bool,
}

impl BuildGuardSnapshot {
    /// A spawn is refused when it would leave more builders than configured,
    /// or when the host's one-minute load is already above CPU capacity.
    pub(crate) fn violations(&self) -> Vec<String> {
        if self.disabled {
            return Vec::new();
        }
        let mut violations = Vec::new();
        if let Some(load) = self.load_1m
            && load > self.cpu_count as f64
        {
            violations.push(format!(
                "1-minute load {:.2} exceeds {} CPUs",
                load, self.cpu_count
            ));
        }
        let projected = self
            .live_cargo_workers
            .saturating_add(self.requested_workers);
        if projected > self.max_concurrent_builders {
            violations.push(format!(
                "{} live/requested Cargo workers would exceed max_concurrent_builders={}",
                projected, self.max_concurrent_builders
            ));
        }
        violations
    }

    pub(crate) fn refusal_message(&self) -> String {
        let violations = self.violations();
        format!(
            "Build concurrency guard: refusing spawn of {} worker(s): {}. Current state: load_1m={}, cpu_count={}, live_cargo_workers={}, max_concurrent_builders={}. Pass force=true to override this soft guard.",
            self.requested_workers,
            violations.join("; "),
            format_load(self.load_1m),
            self.cpu_count,
            self.live_cargo_workers,
            self.max_concurrent_builders,
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
        format!(
            "\nBuild guard: {state}; load_1m={} / cpu_count={}; live_cargo_workers={}; requested_workers={}; max_concurrent_builders={}",
            format_load(self.load_1m),
            self.cpu_count,
            self.live_cargo_workers,
            self.requested_workers,
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
    BuildGuardSnapshot {
        cpu_count: cpu_count.max(1),
        load_1m,
        live_cargo_workers,
        requested_workers,
        max_concurrent_builders,
        disabled: false,
    }
}

/// Inspect the host and evaluate the requested spawn against the factory cap.
pub(crate) fn inspect(
    cas_root: &Path,
    config: &FactoryConfig,
    requested_workers: usize,
) -> BuildGuardSnapshot {
    let cpu_count = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    if build_guard_disabled_override() {
        return BuildGuardSnapshot {
            cpu_count,
            load_1m: None,
            live_cargo_workers: 0,
            requested_workers,
            max_concurrent_builders: config.max_concurrent_builders,
            disabled: true,
        };
    }
    evaluate(
        cpu_count,
        one_minute_load(),
        live_cargo_worker_count(cas_root),
        requested_workers,
        config.max_concurrent_builders,
    )
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

#[cfg(target_os = "linux")]
fn one_minute_load() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|contents| contents.split_whitespace().next()?.parse().ok())
}

#[cfg(not(target_os = "linux"))]
fn one_minute_load() -> Option<f64> {
    None
}

/// Count distinct worker cwd roots with a live `cargo` process. A worker can
/// have more than one cargo child during a nested command, so counting pids
/// would overstate builder concurrency. Read failures are ignored: an
/// unverifiable process must not turn a soft guard into a deadlock.
#[cfg(target_os = "linux")]
fn live_cargo_worker_count(cas_root: &Path) -> usize {
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
fn live_cargo_worker_count(_cas_root: &Path) -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

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
