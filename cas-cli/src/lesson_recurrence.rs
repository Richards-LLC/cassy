//! Lesson recurrence: how often a release's rescues repeat a lesson the
//! release train had already learned (cas-9734).
//!
//! A release run records each rescue in `blockers.log` or
//! `supervisor-interventions.md`, mapped to a failure-log class by
//! `learn=<class>`. A rescue *recurs* when its class already had a failure-log
//! entry on or before the cut that the rescue did not append itself
//! (`release-gate.sh --learn` marks such a row `recurrence=N`). Task-only rows
//! are new causes. A lesson that sticks drives the rate to zero.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};

/// The failure log, relative to a cas-src checkout.
pub const FAILURE_LOG: &str =
    "cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md";

/// Failure-log entry dates per class.
pub fn failure_history(text: &str) -> BTreeMap<String, Vec<NaiveDate>> {
    let mut history: BTreeMap<String, Vec<NaiveDate>> = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("- ") else { continue };
        let date = rest.get(..10).and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok());
        let class = rest.split("**").nth(1).filter(|class| !class.trim().is_empty());
        if let (Some(date), Some(class)) = (date, class) {
            history.entry(class.trim().to_string()).or_default().push(date);
        }
    }
    history
}

/// Recurrence for one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRecurrence {
    pub release: String,
    pub rescues: usize,
    /// Classes of the rescues that repeated a learned lesson, in row order.
    pub recurred: Vec<String>,
}

impl ReleaseRecurrence {
    /// Measure the rescue rows of one release against the failure log.
    pub fn measure(
        release: &str,
        rows: &str,
        history: &BTreeMap<String, Vec<NaiveDate>>,
        cut: NaiveDate,
    ) -> Self {
        let rows: Vec<Vec<&str>> = rows
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.split_whitespace().collect())
            .collect();
        let recurred = rows
            .iter()
            .filter_map(|words| {
                let class = words.iter().find_map(|word| word.strip_prefix("learn="))?;
                let own = words.iter().any(|word| word.starts_with("recurrence="));
                let learned = history
                    .get(class)
                    .map_or(0, |dates| dates.iter().filter(|date| **date <= cut).count());
                (learned > usize::from(own)).then(|| class.to_string())
            })
            .collect();
        Self { release: release.to_string(), rescues: rows.len(), recurred }
    }

    /// `2 of 3 rescues (67%) repeated a learned lesson: scratch-base, version-literals`
    pub fn summary(&self) -> String {
        if self.rescues == 0 {
            return "0 rescues".to_string();
        }
        let percent = (self.recurred.len() * 100 + self.rescues / 2) / self.rescues;
        let mut text = format!(
            "{} of {} rescue{} ({percent}%) repeated a learned lesson",
            self.recurred.len(),
            self.rescues,
            if self.rescues == 1 { "" } else { "s" },
        );
        if !self.recurred.is_empty() {
            text.push_str(&format!(": {}", self.recurred.join(", ")));
        }
        text
    }
}

/// `~/.cas/artifacts/release`, where the release train keeps its run dirs.
pub fn release_runs_root() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| home.join(".cas/artifacts/release"))
}

/// The version of a run directory: `v3.51.0-release-3.51.0` → `3.51.0`.
fn run_version(name: &str) -> Option<Vec<u64>> {
    let version = name.strip_prefix('v')?.split('-').next()?;
    let parts: Vec<u64> = version.split('.').map(|part| part.parse().ok()).collect::<Option<_>>()?;
    (parts.len() == 3).then_some(parts)
}

/// The latest `limit` releases with rescue evidence, newest first. A run's
/// cut date is `cut.start.epoch`; without one, `today`.
pub fn recent_releases(
    root: &Path,
    history: &BTreeMap<String, Vec<NaiveDate>>,
    limit: usize,
    today: NaiveDate,
) -> Vec<ReleaseRecurrence> {
    // version → (rescue rows, earliest cut)
    let mut releases: BTreeMap<Vec<u64>, (String, Option<NaiveDate>)> = BTreeMap::new();
    for run in fs::read_dir(root).into_iter().flatten().flatten() {
        let Some(version) = run.file_name().to_str().and_then(run_version) else { continue };
        let path = run.path();
        let rows: Vec<String> = ["blockers.log", "supervisor-interventions.md"]
            .iter()
            .filter_map(|file| fs::read_to_string(path.join(file)).ok())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let cut = fs::read_to_string(path.join("cut.start.epoch"))
            .ok()
            .and_then(|epoch| DateTime::<Utc>::from_timestamp(epoch.trim().parse().ok()?, 0))
            .map(|cut| cut.date_naive());
        let release = releases.entry(version).or_default();
        release.0.push_str(&rows.join("\n"));
        release.0.push('\n');
        release.1 = match (release.1, cut) {
            (Some(known), Some(cut)) => Some(known.min(cut)),
            (known, cut) => known.or(cut),
        };
    }
    releases
        .iter()
        .rev()
        .take(limit)
        .map(|(version, (rows, cut))| {
            let name = format!("v{}", version.iter().map(u64::to_string).collect::<Vec<_>>().join("."));
            ReleaseRecurrence::measure(&name, rows, history, cut.unwrap_or(today))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const LOG: &str = "\
- 2026-09-02 — **version-literals** — Symptom: literal. Root cause: no env!. Recurrence: 1. Release: v9.11.0.
- 2026-10-10 — **scratch-base** — Symptom: scratch on tmpfs. Root cause: base. Recurrence: 1. Release: operator-reported.
- 2026-10-14 — **assemble-stale-base** — Symptom: new. Root cause: new. Recurrence: 1. Rung: gate. Release: operator-reported.
";

    /// The recorded 3.51.0 run: two rescues mapped to existing lessons, one to
    /// a task. A later run whose --learn appended the class's only entry is new.
    #[test]
    fn release_runs_measure_rescues_that_repeat_a_learned_lesson() {
        let temp = TempDir::new().unwrap();
        let write = |dir: &str, file: &str, text: &str| {
            fs::create_dir_all(temp.path().join(dir)).unwrap();
            fs::write(temp.path().join(dir).join(file), text).unwrap();
        };
        write("v9.98.0-release-9.98.0", "blockers.log", "preflight learn=scratch-base\ngate learn=version-literals\nreceipts task=cas-1543\n");
        write("v9.98.0-release-9.98.0", "cut.start.epoch", "1791979200\n"); // 2026-10-14
        write("v9.98.1-release-9.98.1", "rows", "");
        write("v9.99.0-release-9.99.0", "blockers.log", "assemble learn=assemble-stale-base recurrence=1\n");
        write("v9.99.0-attempt2", "supervisor-interventions.md", "# Rescues\n");
        write("notes", "blockers.log", "gate learn=version-literals\n");

        let history = failure_history(LOG);
        let recent = recent_releases(temp.path(), &history, 5, NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        let summaries: Vec<String> = recent.iter().map(|release| format!("{} {}", release.release, release.summary())).collect();
        assert_eq!(
            summaries,
            [
                "v9.99.0 0 of 1 rescue (0%) repeated a learned lesson",
                "v9.98.0 2 of 3 rescues (67%) repeated a learned lesson: scratch-base, version-literals",
            ]
        );
    }
}
