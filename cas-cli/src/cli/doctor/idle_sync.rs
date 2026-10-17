//! Doctor view of the hub's idle-store cloud sweep (cas-f1fb).
use std::path::Path;

use chrono::{DateTime, Utc};

use super::{Check, CheckStatus};
#[cfg(test)]
use crate::hub::idle_sync::SweepOutcome;
use crate::hub::idle_sync::{IdleSyncConfig, IdleSyncState, StoreSweepRecord};

pub(super) const HOST_CHECK: &str = "hub idle sync";
pub(super) const PROJECT_CHECK: &str = "cloud idle sync";

/// A sweep older than this many intervals means the hub is not sweeping.
const STALE_INTERVALS: i32 = 3;

fn load() -> Result<Option<IdleSyncState>, String> {
    let path = crate::hub::idle_sync::default_state_path().map_err(|error| error.to_string())?;
    if !path.is_file() {
        return Ok(None);
    }
    IdleSyncState::load(&path)
        .map(Some)
        .map_err(|error| format!("{error:#}"))
}

fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (now - at).num_minutes().max(0);
    match minutes {
        0 => "just now".to_owned(),
        m if m < 120 => format!("{m}m ago"),
        m if m < 48 * 60 => format!("{}h ago", m / 60),
        m => format!("{}d ago", m / (24 * 60)),
    }
}

fn store_name(record: &StoreSweepRecord) -> String {
    record
        .repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| record.repo.display().to_string())
}

/// One store's last sweep result, in a line.
fn describe(record: &StoreSweepRecord, now: DateTime<Utc>) -> String {
    let mut line = format!(
        "{} {}",
        record.outcome.label(),
        ago(record.last_attempt_at, now)
    );
    if record.outcome.is_failure() {
        if record.consecutive_failures > 1 {
            line.push_str(&format!(" ({} in a row)", record.consecutive_failures));
        }
        if let Some(detail) = &record.detail {
            line.push_str(&format!(": {detail}"));
        }
        if let Some(next) = record.next_attempt_after {
            line.push_str(&format!(", next try {}", next.format("%Y-%m-%d %H:%M UTC")));
        }
        match record.last_success_at {
            Some(success) => line.push_str(&format!(", last synced {}", ago(success, now))),
            None => line.push_str(", never synced by the sweep"),
        }
    }
    line
}

fn stale_sweep(state: &IdleSyncState, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let interval = chrono::Duration::from_std(IdleSyncConfig::default().interval).ok()?;
    state
        .last_sweep_at
        .filter(|at| now - *at > interval * STALE_INTERVALS)
}

/// Every swept store's last result.
pub(super) fn host_check() -> Check {
    host_check_from(load(), Utc::now())
}

pub(super) fn host_check_from(
    state: Result<Option<IdleSyncState>, String>,
    now: DateTime<Utc>,
) -> Check {
    let state = match state {
        Ok(Some(state)) => state,
        Ok(None) => {
            return Check::new(
                HOST_CHECK,
                CheckStatus::Info,
                "no idle-store sweep recorded yet. The Commander hub syncs cloud-linked stores no session has open; start it with `cas hub start`",
            );
        }
        Err(error) => {
            return Check::new(
                HOST_CHECK,
                CheckStatus::Warning,
                format!("sweep state unreadable: {error}"),
            );
        }
    };
    if let Some(last) = stale_sweep(&state, now) {
        return Check::new(
            HOST_CHECK,
            CheckStatus::Warning,
            format!(
                "the last idle-store sweep ran {}, so idle stores are not syncing; run `cas hub status`, then `cas hub start`",
                ago(last, now)
            ),
        );
    }
    let failing: Vec<&StoreSweepRecord> = state
        .stores
        .values()
        .filter(|record| record.outcome.is_failure())
        .collect();
    if !failing.is_empty() {
        let lines = failing
            .iter()
            .map(|record| format!("{}: {}", store_name(record), describe(record, now)))
            .collect::<Vec<_>>()
            .join("; ");
        return Check::new(
            HOST_CHECK,
            CheckStatus::Warning,
            format!(
                "{} of {} store(s) failing: {lines}",
                failing.len(),
                state.stores.len()
            ),
        );
    }
    if state.stores.is_empty() {
        return Check::new(
            HOST_CHECK,
            CheckStatus::Ok,
            "no cloud-linked store to sweep",
        );
    }
    let lines = state
        .stores
        .values()
        .map(|record| format!("{}: {}", store_name(record), describe(record, now)))
        .collect::<Vec<_>>()
        .join("; ");
    Check::new(HOST_CHECK, CheckStatus::Ok, lines)
}

/// This store's last sweep result, for a cloud-linked store.
pub(super) fn project_check(cas_root: &Path) -> Option<Check> {
    cas_root
        .join("cloud.json")
        .is_file()
        .then(|| project_check_from(load(), cas_root, Utc::now()))
}

fn same_store(record: &StoreSweepRecord, cas_root: &Path) -> bool {
    let swept = record.repo.join(".cas");
    swept == cas_root
        || matches!(
            (swept.canonicalize(), cas_root.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

pub(super) fn project_check_from(
    state: Result<Option<IdleSyncState>, String>,
    cas_root: &Path,
    now: DateTime<Utc>,
) -> Check {
    let state = match state {
        Ok(Some(state)) => state,
        Ok(None) => {
            return Check::new(
                PROJECT_CHECK,
                CheckStatus::Info,
                "not swept yet. While no session is open, this store syncs only when the Commander hub runs; start it with `cas hub start`",
            );
        }
        Err(error) => {
            return Check::new(
                PROJECT_CHECK,
                CheckStatus::Warning,
                format!("sweep state unreadable: {error}"),
            );
        }
    };
    let Some(record) = state
        .stores
        .values()
        .find(|record| same_store(record, cas_root))
    else {
        return Check::new(
            PROJECT_CHECK,
            CheckStatus::Info,
            "not swept yet. The hub sweeps registered stores; a store with auto_sync off or no login is left out",
        );
    };
    let stale = stale_sweep(&state, now).is_some();
    let status = if record.outcome.is_failure() || stale {
        CheckStatus::Warning
    } else {
        CheckStatus::Ok
    };
    let mut message = describe(record, now);
    if record.outcome.is_failure() {
        message.push_str(&format!(
            "; output in {}",
            cas_root
                .join("logs")
                .join(crate::hub::idle_sync::STORE_LOG_FILE)
                .display()
        ));
    } else if stale {
        message.push_str("; the hub has stopped sweeping. Run `cas hub start`");
    }
    Check::new(PROJECT_CHECK, status, message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    fn record(repo: &str, outcome: SweepOutcome, at: DateTime<Utc>) -> StoreSweepRecord {
        StoreSweepRecord {
            repo: PathBuf::from(repo),
            last_attempt_at: at,
            outcome,
            detail: outcome
                .is_failure()
                .then(|| "exit 1: cloud said no".to_owned()),
            duration_ms: 10,
            consecutive_failures: u32::from(outcome.is_failure()),
            next_attempt_after: None,
            last_success_at: None,
        }
    }

    fn state(records: Vec<StoreSweepRecord>, swept_at: DateTime<Utc>) -> IdleSyncState {
        IdleSyncState {
            last_sweep_at: Some(swept_at),
            stores: records
                .into_iter()
                .map(|record| (record.repo.display().to_string(), record))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn host_check_names_every_store_and_flags_failures() {
        let now = Utc::now();
        let healthy = state(
            vec![
                record("/w/alpha", SweepOutcome::Synced, now),
                record("/w/beta", SweepOutcome::LiveServe, now),
            ],
            now,
        );
        let check = host_check_from(Ok(Some(healthy)), now);
        assert!(matches!(check.status, CheckStatus::Ok));
        assert!(check.message.contains("alpha: synced"), "{}", check.message);
        assert!(
            check
                .message
                .contains("beta: skipped (a live cas serve syncs it)")
        );

        let failing = state(
            vec![
                record("/w/alpha", SweepOutcome::Synced, now),
                record("/w/beta", SweepOutcome::Failed, now),
            ],
            now,
        );
        let check = host_check_from(Ok(Some(failing)), now);
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("1 of 2 store(s) failing"),
            "{}",
            check.message
        );
        assert!(
            check
                .message
                .contains("beta: failed just now: exit 1: cloud said no")
        );
    }

    #[test]
    fn host_check_warns_when_the_hub_stopped_sweeping() {
        let now = Utc::now();
        let old = now - chrono::Duration::hours(2);
        let check = host_check_from(
            Ok(Some(state(
                vec![record("/w/alpha", SweepOutcome::Synced, old)],
                old,
            ))),
            now,
        );
        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check.message.contains("`cas hub start`"),
            "{}",
            check.message
        );
    }

    #[test]
    fn missing_state_points_at_the_hub() {
        let check = host_check_from(Ok(None), Utc::now());
        assert!(matches!(check.status, CheckStatus::Info));
        assert!(check.message.contains("cas hub start"));
    }

    #[test]
    fn project_check_reports_this_store_only() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("alpha");
        std::fs::create_dir_all(repo.join(".cas")).unwrap();
        let now = Utc::now();
        let mut failed = record(repo.to_str().unwrap(), SweepOutcome::TimedOut, now);
        failed.detail = Some("killed after 180s".into());
        let swept = state(
            vec![failed, record("/w/other", SweepOutcome::Synced, now)],
            now,
        );

        let check = project_check_from(Ok(Some(swept)), &repo.join(".cas"), now);

        assert!(matches!(check.status, CheckStatus::Warning));
        assert!(
            check
                .message
                .starts_with("timed out just now: killed after 180s"),
            "{}",
            check.message
        );
        assert!(check.message.contains("hub-idle-sync.log"));
    }
}
