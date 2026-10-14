//! Lesson recurrence: how often a release's rescues repeat a lesson the
//! release train had already learned (cas-9734).
//!
//! A release run records each rescue as a row in `blockers.log` or
//! `supervisor-interventions.md`, mapped by `release-gate.sh --learn` to a
//! failure-log class (`learn=<class>`). A rescue *recurs* when its class had a
//! failure-log entry dated on or before the cut that the rescue did not append
//! itself (`recurrence=N` marks a row whose `--learn` appended its own entry).
//! Rows mapped only to a task (`task=<id>`) are new causes, not recurrences.
//!
//! The rate is `recurred / rescues` for one release. A lesson that sticks
//! drives it to zero; 3.51.0 measured 2 of 3.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};

/// The failure log, relative to a cas-src checkout.
pub const FAILURE_LOG: &str = "cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md";
const EVIDENCE_FILES: [&str; 2] = ["blockers.log", "supervisor-interventions.md"];

/// Dated failure-log entries per class (check id or `manual:` class).
#[derive(Debug, Clone, Default)]
pub struct FailureHistory {
    dates: BTreeMap<String, Vec<NaiveDate>>,
}

impl FailureHistory {
    pub fn parse(text: &str) -> Self {
        let mut dates: BTreeMap<String, Vec<NaiveDate>> = BTreeMap::new();
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("- ") else {
                continue;
            };
            let Some(date) = rest
                .get(..10)
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let Some(class) = rest
                .split_once("**")
                .and_then(|(_, tail)| tail.split_once("**"))
                .map(|(class, _)| class.trim())
                .filter(|class| !class.is_empty())
            else {
                continue;
            };
            dates.entry(class.to_string()).or_default().push(date);
        }
        Self { dates }
    }

    /// The project's failure log, falling back to the copy this binary ships.
    pub fn load(project_root: Option<&Path>) -> Self {
        let text = project_root
            .and_then(|root| fs::read_to_string(root.join(FAILURE_LOG)).ok())
            .or_else(|| {
                crate::builtins::BUILTIN_SKILLS
                    .iter()
                    .find(|file| file.path == "skills/cas-cut-release/references/failure-log.md")
                    .map(|file| file.content.to_string())
            })
            .unwrap_or_default();
        Self::parse(&text)
    }

    fn learned_by(&self, class: &str, cut: NaiveDate) -> usize {
        self.dates
            .get(class)
            .map(|dates| dates.iter().filter(|date| **date <= cut).count())
            .unwrap_or(0)
    }
}

/// One rescue row from a release run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rescue {
    pub lesson: Option<String>,
    /// `--learn` appended this rescue's own failure-log entry.
    pub own_entry: bool,
}

pub fn parse_rescues(text: &str) -> Vec<Rescue> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            Rescue {
                lesson: words
                    .iter()
                    .find_map(|word| word.strip_prefix("learn="))
                    .filter(|class| !class.is_empty())
                    .map(str::to_string),
                own_entry: words.iter().any(|word| {
                    word.strip_prefix("recurrence=")
                        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
                }),
            }
        })
        .collect()
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
    pub fn measure(release: &str, rescues: &[Rescue], history: &FailureHistory, cut: NaiveDate) -> Self {
        let recurred = rescues
            .iter()
            .filter_map(|rescue| {
                let class = rescue.lesson.as_deref()?;
                let prior = history
                    .learned_by(class, cut)
                    .saturating_sub(usize::from(rescue.own_entry));
                (prior >= 1).then(|| class.to_string())
            })
            .collect();
        Self {
            release: release.to_string(),
            rescues: rescues.len(),
            recurred,
        }
    }

    /// Percent of rescues that recurred; `None` for a release with none.
    pub fn rate_percent(&self) -> Option<u32> {
        (self.rescues > 0).then(|| {
            u32::try_from((self.recurred.len() * 100 + self.rescues / 2) / self.rescues).unwrap_or(100)
        })
    }

    /// `2 of 3 rescues (67%) repeated a learned lesson: scratch-base, version-literals`
    pub fn summary(&self) -> String {
        if self.rescues == 0 {
            return "0 rescues".to_string();
        }
        let mut text = format!(
            "{} of {} rescue{} ({}%) repeated a learned lesson",
            self.recurred.len(),
            self.rescues,
            if self.rescues == 1 { "" } else { "s" },
            self.rate_percent().unwrap_or(0)
        );
        if !self.recurred.is_empty() {
            let mut classes = self.recurred.clone();
            classes.dedup();
            text.push_str(": ");
            text.push_str(&classes.join(", "));
        }
        text
    }
}

/// `~/.cas/artifacts/release`, where the release train keeps its run dirs.
pub fn release_runs_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".cas/artifacts/release"))
}

/// The version a run directory belongs to (`v3.51.0-release-3.51.0` → `3.51.0`).
fn run_version(name: &str) -> Option<&str> {
    let version = name.strip_prefix('v')?;
    let version = version.split_once('-').map_or(version, |(version, _)| version);
    let mut parts = version.split('.');
    let valid = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && parts.next().is_none();
    valid.then_some(version)
}

fn version_key(version: &str) -> Vec<u64> {
    version.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

/// When a run started: `cut.start.epoch`, else `run.env` `started_at`.
fn run_cut_date(run: &Path) -> Option<NaiveDate> {
    if let Some(date) = fs::read_to_string(run.join("cut.start.epoch"))
        .ok()
        .and_then(|epoch| epoch.trim().parse::<i64>().ok())
        .and_then(|epoch| DateTime::<Utc>::from_timestamp(epoch, 0))
    {
        return Some(date.date_naive());
    }
    let env = fs::read_to_string(run.join("run.env")).ok()?;
    env.lines().find_map(|line| {
        let value = line.trim().strip_prefix("started_at=")?.trim().trim_matches('"');
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|date| date.with_timezone(&Utc).date_naive())
            .or_else(|| value.get(..10).and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()))
    })
}

/// Every recorded run of `version` with rescue evidence, as (rescues, cut).
fn version_runs(root: &Path, version: &str) -> Option<(Vec<Rescue>, Option<NaiveDate>)> {
    let mut runs: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(run_version)
                == Some(version)
        })
        .collect();
    runs.sort();
    let mut rescues = Vec::new();
    let mut cut: Option<NaiveDate> = None;
    let mut found = false;
    for run in runs {
        for file in EVIDENCE_FILES {
            if let Ok(text) = fs::read_to_string(run.join(file)) {
                found = true;
                rescues.extend(parse_rescues(&text));
            }
        }
        if let Some(date) = run_cut_date(&run) {
            cut = Some(cut.map_or(date, |known| known.min(date)));
        }
    }
    found.then_some((rescues, cut))
}

/// Recurrence for `version` from its recorded runs; `None` without evidence.
pub fn for_release(
    root: &Path,
    version: &str,
    history: &FailureHistory,
    fallback_cut: NaiveDate,
) -> Option<ReleaseRecurrence> {
    let version = version.trim_start_matches('v');
    let (rescues, cut) = version_runs(root, version)?;
    Some(ReleaseRecurrence::measure(
        &format!("v{version}"),
        &rescues,
        history,
        cut.unwrap_or(fallback_cut),
    ))
}

/// The latest `limit` releases with rescue evidence, newest first.
pub fn recent_releases(root: &Path, history: &FailureHistory, limit: usize, today: NaiveDate) -> Vec<ReleaseRecurrence> {
    let mut versions: Vec<String> = fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().and_then(run_version).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    versions.sort_by_key(|version| std::cmp::Reverse(version_key(version)));
    versions.dedup();
    versions
        .iter()
        .filter_map(|version| for_release(root, version, history, today))
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    const LOG: &str = "\
- 2026-09-02 — **version-literals** — Symptom: literal. Root cause: no env!. Recurrence: 1. Release: v3.11.0.
- 2026-10-10 — **scratch-base** — Symptom: scratch on tmpfs. Root cause: base. Recurrence: 1. Release: operator-reported.
- 2026-10-14 — **assemble-stale-base** — Symptom: new. Root cause: new. Recurrence: 1. Rung: gate. Release: operator-reported.
- 2026-10-20 — **nextest** — Symptom: later. Root cause: later. Recurrence: 1. Release: operator-reported.
not an entry
";

    #[test]
    fn a_rescue_recurs_only_when_its_lesson_was_already_learned() {
        let history = FailureHistory::parse(LOG);
        // The 9.98.0 run: two rows mapped to existing lessons, one to a task.
        let rescues = parse_rescues(
            "preflight learn=scratch-base\ngate learn=version-literals\n\n# note\nreceipts task=cas-1543\n",
        );
        let measured = ReleaseRecurrence::measure("v9.98.0", &rescues, &history, date("2026-10-14"));
        assert_eq!(measured.rescues, 3);
        assert_eq!(measured.recurred, ["scratch-base", "version-literals"]);
        assert_eq!(measured.rate_percent(), Some(67));
        assert_eq!(
            measured.summary(),
            "2 of 3 rescues (67%) repeated a learned lesson: scratch-base, version-literals"
        );

        // A rescue whose --learn appended the only entry is new, not a repeat;
        // an entry dated after the cut is not prior knowledge.
        let rescues = parse_rescues("assemble learn=assemble-stale-base recurrence=1\ngate learn=nextest\n");
        let measured = ReleaseRecurrence::measure("v9.99.0", &rescues, &history, date("2026-10-14"));
        assert!(measured.recurred.is_empty());
        assert_eq!(measured.summary(), "0 of 2 rescues (0%) repeated a learned lesson");

        let none = ReleaseRecurrence::measure("v9.99.1", &[], &history, date("2026-10-14"));
        assert_eq!((none.rate_percent(), none.summary().as_str()), (None, "0 rescues"));
    }

    #[test]
    fn recent_releases_read_run_dirs_newest_first() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let write = |dir: &str, file: &str, text: &str| {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join(file), text).unwrap();
        };
        write("v9.98.0-release-9.98.0", "blockers.log", "preflight learn=scratch-base\ngate learn=version-literals\nreceipts task=cas-1543\n");
        // 2026-10-14T12:00:00Z
        write("v9.98.0-release-9.98.0", "cut.start.epoch", "1791979200\n");
        write("v9.98.1-release-9.98.1", "rows", "");
        write("v9.99.0-release-9.99.0", "blockers.log", "preflight\n");
        write("v9.99.0-release-9.99.0", "run.env", "started_at=2026-10-14T16:00:00Z\n");
        write("v9.99.0-attempt2", "supervisor-interventions.md", "# Rescues\n- gate: learn=version-literals\n");
        write("notes", "blockers.log", "gate learn=version-literals\n");

        let history = FailureHistory::parse(LOG);
        let recent = recent_releases(root, &history, 5, date("2026-10-30"));
        let names: Vec<&str> = recent.iter().map(|release| release.release.as_str()).collect();
        assert_eq!(names, ["v9.99.0", "v9.98.0"], "9.98.1 has no rescue evidence");
        assert_eq!((recent[0].rescues, recent[0].recurred.len()), (2, 1));
        assert_eq!(recent[1].summary(), "2 of 3 rescues (67%) repeated a learned lesson: scratch-base, version-literals");
        assert!(for_release(root, "v9.98.1", &history, date("2026-10-30")).is_none());
    }

    #[test]
    fn shipped_failure_log_parses_every_entry() {
        let history = FailureHistory::load(None);
        let entries: usize = history.dates.values().map(Vec::len).sum();
        assert!(entries >= 22, "{entries}");
        assert!(history.dates.contains_key("archive-mode"));
    }
}
