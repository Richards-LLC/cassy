//! `cas ledger`: north-star numbers and feature verdicts from `quality_events`.
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use cas_store::{CheckOutcome, FeedbackSource, FeedbackVerdict, QualityEvent, QualityPayload};
use chrono::{DateTime, Duration, Local, Utc};
use clap::{Args, Subcommand};
use serde::Serialize;

use super::Cli;

#[derive(Args)]
pub struct LedgerArgs {
    #[command(subcommand)]
    command: LedgerCommand,
}

#[derive(Subcommand)]
enum LedgerCommand {
    /// Print interventions, speed, escapes and feature verdicts for a release
    Report {
        /// Release tag, vX.Y.Z. Omit for work since the last release.
        #[arg(long)]
        release: Option<String>,
        /// Read local rows only; do not pull from Cloud first
        #[arg(long)]
        offline: bool,
    },
    /// Turn history already on disk into events (safe to rerun)
    Backfill {
        /// How many days of task notes and verifications to read
        #[arg(long, default_value_t = 30)]
        days: i64,
    },
    /// Record a published release from its latency receipt (the release train runs this)
    #[command(hide = true)]
    RecordRelease {
        /// Path to release-latency.receipt
        #[arg(long)]
        receipt: std::path::PathBuf,
    },
}

pub fn execute(args: &LedgerArgs, cli: &Cli, cas_root: &Path) -> Result<()> {
    match &args.command {
        LedgerCommand::Report { release, offline } => {
            if !offline {
                pull_before_report(cas_root);
            }
            let repo = cas_root.parent().unwrap_or(cas_root);
            let report = build_report(cas_root, repo, release.as_deref())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", render(&report));
            }
        }
        LedgerCommand::Backfill { days } => {
            let inserted =
                crate::quality_ledger::backfill(cas_root, Utc::now() - Duration::days(*days))?;
            if cli.json {
                println!("{}", serde_json::json!({ "inserted": inserted }));
            } else {
                println!("Recorded {inserted} events from the last {days} days of history.");
            }
        }
        LedgerCommand::RecordRelease { receipt } => {
            let repo = cas_root.parent().unwrap_or(cas_root);
            let version = record_release(cas_root, repo, receipt)?;
            println!("Recorded release {version}.");
        }
    }
    Ok(())
}

/// A report reads the fleet's rows, so it asks for them first. Being offline or
/// on a server without the route still leaves a correct local report.
fn pull_before_report(cas_root: &Path) {
    let synced = (|| -> Result<_> {
        use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};
        let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
        if !config.is_logged_in() {
            return Ok(None);
        }
        let queue = std::sync::Arc::new(SyncQueue::open(cas_root)?);
        queue.init()?;
        let project_id = crate::cloud::resolve_canonical_id_for_sync(cas_root)?;
        let syncer = CloudSyncer::new_for_project(
            queue,
            config,
            CloudSyncerConfig::default(),
            project_id,
            cas_root,
        );
        Ok(Some(syncer.sync_quality_events(true)?))
    })();
    if let Err(error) = synced {
        tracing::warn!(%error, "ledger report: showing local rows; cloud sync failed");
    }
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").current_dir(repo).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim()).ok().map(|time| time.with_timezone(&Utc))
}

fn tag_time(repo: &Path, tag: &str) -> Option<DateTime<Utc>> {
    parse_time(&git(repo, &["log", "-1", "--format=%cI", tag])?)
}

fn previous_tag(repo: &Path, tag: &str) -> Option<String> {
    git(repo, &["describe", "--tags", "--abbrev=0", &format!("{tag}^")])
}

fn minutes_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_seconds().max(0) as f64 / 60.0
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    })
}

/// Median minutes from a commit's author time to its landing on the first-parent
/// line, across the commits this tag added.
fn commit_to_main_minutes(repo: &Path, tag: &str) -> Option<f64> {
    let previous = previous_tag(repo, tag)?;
    let landings = git(
        repo,
        &["log", "--first-parent", "--format=%H %cI", &format!("{previous}..{tag}")],
    )?;
    let mut minutes = Vec::new();
    for landing in landings.lines() {
        let (sha, landed_at) = landing.split_once(' ')?;
        let landed_at = parse_time(landed_at)?;
        let range = format!("{sha}^1..{sha}");
        let authored = git(repo, &["log", "--no-merges", "--format=%aI", &range]).unwrap_or_default();
        minutes.extend(
            authored
                .lines()
                .filter_map(parse_time)
                .map(|authored_at| minutes_between(authored_at, landed_at)),
        );
    }
    median(minutes)
}

fn record_release(cas_root: &Path, repo: &Path, receipt: &Path) -> Result<String> {
    let text = std::fs::read_to_string(receipt)
        .with_context(|| format!("read {}", receipt.display()))?;
    let fields: BTreeMap<&str, &str> = text.lines().filter_map(|line| line.split_once('=')).collect();
    let tag = fields.get("TAG").copied().context("receipt has no TAG")?;
    let version = cas_store::normalize_release_version(tag)
        .with_context(|| format!("receipt TAG is not a version: {tag}"))?;
    let published_at = fields
        .get("PUBLISHED_AT")
        .and_then(|value| parse_time(value))
        .context("receipt has no valid PUBLISHED_AT")?;
    let main_to_published = fields
        .get("MAIN_LANDED_AT")
        .and_then(|value| parse_time(value))
        .map(|landed_at| minutes_between(landed_at, published_at));
    let previous = previous_tag(repo, tag).and_then(|previous| tag_time(repo, &previous));
    let ejections = cas_store::quality_events_between(cas_root, previous, Some(published_at))?
        .iter()
        .filter(|event| {
            event.project_id.is_empty()
                && matches!(
                    &event.payload,
                    QualityPayload::CheckRun { stage: cas_store::CheckStage::Queue, outcome: CheckOutcome::Fail, .. }
                )
        })
        .count() as u32;
    let mut event = cas_store::NewQualityEvent::new(
        format!("release|{version}"),
        QualityPayload::Release {
            commit_to_main_minutes: commit_to_main_minutes(repo, tag),
            main_to_published_minutes: main_to_published,
            ejections,
            rescues: fields
                .get("INTERVENTIONS")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        },
    );
    event.occurred_at = published_at;
    event.release_version = Some(version.clone());
    crate::quality_ledger::emit(cas_root, &event);
    Ok(version)
}

#[derive(Debug, Default, Serialize)]
struct FeatureRow {
    measured: BTreeMap<&'static str, u32>,
    supervisors: BTreeMap<&'static str, u32>,
    injected: u64,
    opened: u64,
    used: u64,
}

#[derive(Debug, Serialize)]
struct EscapeRow {
    project: String,
    task: Option<String>,
    title: Option<String>,
    found_in: String,
    missed_stage: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct CheckRow {
    pass: u32,
    fail: u32,
    flaky: u32,
    error: u32,
}

#[derive(Debug, Serialize)]
struct LedgerReport {
    release: String,
    published: bool,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    events: usize,
    projects: Vec<String>,
    interventions_total: u32,
    interventions: BTreeMap<&'static str, u32>,
    commit_to_main_minutes: Option<f64>,
    main_to_published_minutes: Option<f64>,
    ejections: u32,
    escapes: Vec<EscapeRow>,
    checks: BTreeMap<String, CheckRow>,
    features: BTreeMap<String, FeatureRow>,
}

/// `(from, to]` for a release. A release ends at its release event, or at its tag
/// when no event exists yet (history before this ledger). An unknown or omitted
/// release is work in progress since the newest known release.
fn release_window(
    cas_root: &Path,
    repo: &Path,
    release: Option<&str>,
) -> Result<(String, bool, Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
    let events = cas_store::quality_release_events(cas_root)?;
    let wanted = release
        .map(|raw| {
            cas_store::normalize_release_version(raw)
                .with_context(|| format!("--release must look like vX.Y.Z, got {raw}"))
        })
        .transpose()?;
    let end_of = |version: &str| {
        events
            .iter()
            .find(|event| event.release_version.as_deref() == Some(version))
            .map(|event| event.occurred_at)
            .or_else(|| tag_time(repo, version))
    };
    if let Some(version) = wanted.as_deref()
        && let Some(end) = end_of(version)
    {
        let previous = previous_tag(repo, version)
            .and_then(|previous| end_of(&previous))
            .or_else(|| events.iter().map(|event| event.occurred_at).find(|at| *at < end));
        return Ok((version.to_string(), true, previous, Some(end)));
    }
    let newest_tag = git(repo, &["describe", "--tags", "--abbrev=0"]).and_then(|tag| end_of(&tag));
    let newest_event = events.first().map(|event| event.occurred_at);
    let from = newest_tag.into_iter().chain(newest_event).max();
    Ok((wanted.unwrap_or_else(|| "unreleased".to_string()), false, from, None))
}

fn build_report(cas_root: &Path, repo: &Path, release: Option<&str>) -> Result<LedgerReport> {
    let (name, published, from, to) = release_window(cas_root, repo, release)?;
    if from.is_some() && from == to {
        bail!("release {name} has an empty window");
    }
    let mut events = cas_store::quality_events_between(cas_root, from, to)?;
    // An escape belongs to the release it escaped into, however much later it was
    // found. Work in progress has no such release, so it lists what its window found.
    if published {
        let is_escape = |event: &QualityEvent| matches!(event.payload, QualityPayload::Escape { .. });
        events.retain(|event| !is_escape(event));
        events.extend(
            cas_store::quality_events_between(cas_root, None, None)?
                .into_iter()
                .filter(|event| {
                    matches!(&event.payload, QualityPayload::Escape { found_in, .. } if *found_in == name)
                }),
        );
    }
    Ok(aggregate(name, published, from, to, &events))
}

fn project_name(event: &QualityEvent) -> String {
    if event.project_id.is_empty() {
        "this project".to_string()
    } else {
        event.project_id.clone()
    }
}

fn aggregate(
    release: String,
    published: bool,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    events: &[QualityEvent],
) -> LedgerReport {
    let mut report = LedgerReport {
        release,
        published,
        from,
        to,
        events: events.len(),
        projects: Vec::new(),
        interventions_total: 0,
        interventions: BTreeMap::new(),
        commit_to_main_minutes: None,
        main_to_published_minutes: None,
        ejections: 0,
        escapes: Vec::new(),
        checks: BTreeMap::new(),
        features: BTreeMap::new(),
    };
    for event in events {
        let project = project_name(event);
        if !report.projects.contains(&project) {
            report.projects.push(project.clone());
        }
        match &event.payload {
            QualityPayload::Intervention { intervention, .. } => {
                report.interventions_total += 1;
                *report.interventions.entry(intervention.as_str()).or_default() += 1;
            }
            QualityPayload::CheckRun { stage, outcome, .. } => {
                let stage = serde_json::to_value(stage)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default();
                let row = report.checks.entry(stage).or_default();
                match outcome {
                    CheckOutcome::Pass => row.pass += 1,
                    CheckOutcome::Fail => row.fail += 1,
                    CheckOutcome::Flaky => row.flaky += 1,
                    CheckOutcome::Error => row.error += 1,
                }
            }
            QualityPayload::Escape { found_in, missed_stage } => report.escapes.push(EscapeRow {
                project,
                task: event.task_id.clone(),
                title: event.title.clone(),
                found_in: found_in.clone(),
                missed_stage: missed_stage.and_then(|stage| {
                    serde_json::to_value(stage).ok()?.as_str().map(str::to_string)
                }),
            }),
            QualityPayload::Release {
                commit_to_main_minutes,
                main_to_published_minutes,
                ejections,
                ..
            } => {
                if event.release_version.as_deref() == Some(report.release.as_str()) {
                    report.commit_to_main_minutes = *commit_to_main_minutes;
                    report.main_to_published_minutes = *main_to_published_minutes;
                    report.ejections = *ejections;
                }
            }
            QualityPayload::FeatureFeedback { feature, verdict, source, injected, opened, used } => {
                let row = report.features.entry(feature.clone()).or_default();
                let tally = match source {
                    FeedbackSource::Measured => &mut row.measured,
                    FeedbackSource::Supervisor => &mut row.supervisors,
                };
                *tally.entry(verdict_name(*verdict)).or_default() += 1;
                row.injected += injected.unwrap_or(0);
                row.opened += opened.unwrap_or(0);
                row.used += used.unwrap_or(0);
            }
        }
    }
    report
}

fn verdict_name(verdict: FeedbackVerdict) -> &'static str {
    verdict.as_str()
}

fn tally(values: &BTreeMap<&'static str, u32>) -> String {
    values
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn when(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local).format("%b %-d, %-I:%M %p").to_string()
}

fn duration(minutes: Option<f64>) -> String {
    minutes.map_or_else(|| "not measured".to_string(), |minutes| format!("{minutes:.0} min"))
}

fn render(report: &LedgerReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let state = if report.published { "" } else { " (in progress)" };
    let _ = writeln!(out, "Friction ledger: {}{state}", report.release);
    let window = match (report.from, report.to) {
        (Some(from), Some(to)) => format!("{} to {}", when(from), when(to)),
        (Some(from), None) => format!("since {}", when(from)),
        (None, Some(to)) => format!("through {}", when(to)),
        (None, None) => "all recorded history".to_string(),
    };
    let _ = writeln!(
        out,
        "{window}. {} events from {} project(s).",
        report.events,
        report.projects.len()
    );

    let _ = writeln!(out, "\nInterventions: {}", report.interventions_total);
    for (kind, count) in &report.interventions {
        let _ = writeln!(out, "  {kind:<16}{count:>4}");
    }

    let _ = writeln!(out, "\nSpeed");
    if report.published {
        let _ = writeln!(out, "  commit to main  {}", duration(report.commit_to_main_minutes));
        let _ = writeln!(out, "  main to publish {}", duration(report.main_to_published_minutes));
        let _ = writeln!(out, "  queue ejections {}", report.ejections);
    } else {
        let _ = writeln!(out, "  not published yet");
    }

    let _ = writeln!(out, "\nEscapes: {}", report.escapes.len());
    for escape in &report.escapes {
        let _ = writeln!(
            out,
            "  {} {} (found in {}, should have been caught at {}; {})",
            escape.task.as_deref().unwrap_or("unknown task"),
            escape.title.as_deref().unwrap_or("untitled"),
            escape.found_in,
            escape.missed_stage.as_deref().unwrap_or("unknown stage"),
            escape.project,
        );
    }

    let _ = writeln!(out, "\nChecks (pass/fail/flaky/could not run)");
    if report.checks.is_empty() {
        let _ = writeln!(out, "  none recorded");
    }
    for (stage, row) in &report.checks {
        let _ = writeln!(out, "  {stage:<10}{}/{}/{}/{}", row.pass, row.fail, row.flaky, row.error);
    }

    let _ = writeln!(out, "\nFeature feedback");
    if report.features.is_empty() {
        let _ = writeln!(out, "  none recorded");
    }
    for (feature, row) in &report.features {
        let mut parts = Vec::new();
        if !row.measured.is_empty() {
            parts.push(format!(
                "measured {} ({} injected, {} opened, {} used)",
                tally(&row.measured),
                row.injected,
                row.opened,
                row.used
            ));
        }
        if !row.supervisors.is_empty() {
            parts.push(format!("supervisors {}", tally(&row.supervisors)));
        }
        let _ = writeln!(out, "  {feature}: {}", parts.join("; "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{InterventionKind, NewQualityEvent};
    use chrono::TimeZone;

    fn git_in(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// The train's receipt becomes a release event, and the report for that
    /// release shows its speed, the window's interventions and the fleet's rows.
    #[test]
    fn receipt_becomes_a_release_and_the_report_reads_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let cas_root = repo.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        git_in(repo, &["init", "-q", "-b", "main"]);
        for (tag, date) in [("v1.0.0", "2026-10-01T12:00:00Z"), ("v1.1.0", "2026-10-02T12:00:00Z")] {
            let commit = Command::new("git")
                .current_dir(repo)
                .args(["commit", "-q", "--allow-empty", "-m", tag])
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.test")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.test")
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date)
                .status()
                .unwrap();
            assert!(commit.success());
            git_in(repo, &["tag", tag]);
        }

        let at = |day, hour| Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap();
        let intervention = |key: &str, occurred_at| {
            let mut event = NewQualityEvent::new(
                key,
                QualityPayload::Intervention {
                    intervention: InterventionKind::Rebind,
                    minutes: None,
                },
            );
            event.occurred_at = occurred_at;
            crate::quality_ledger::emit(&cas_root, &event);
        };
        intervention("before the window", at(1, 6));
        intervention("in the window", at(2, 6));
        intervention("after the window", at(3, 6));
        // Found two days after the release, it still counts against that release.
        let mut escape = NewQualityEvent::new(
            "late escape",
            QualityPayload::Escape { found_in: "v1.1.0".into(), missed_stage: None },
        );
        escape.occurred_at = at(4, 6);
        crate::quality_ledger::emit(&cas_root, &escape);
        let other_project = cas_store::QualityEventWire {
            id: "qe-other".into(),
            kind: "feature_feedback".into(),
            occurred_at: at(2, 7),
            task_id: None,
            release_version: None,
            actor: Some("ending-supervisor".into()),
            title: None,
            link: None,
            payload: QualityPayload::FeatureFeedback {
                feature: "ambient_recall".into(),
                verdict: FeedbackVerdict::Noise,
                source: FeedbackSource::Measured,
                injected: Some(283),
                opened: Some(1),
                used: Some(0),
            },
            project_id: Some("acme/other".into()),
        };
        // Older backfills emitted lifetime totals dated at the time of backfill.
        // They remain immutable after sync, and a second day could repeat them.
        let mut legacy = other_project.clone();
        legacy.id = "qe-lifetime-day-one".into();
        legacy.actor = None;
        legacy.payload = QualityPayload::FeatureFeedback {
            feature: "ambient_recall".into(),
            verdict: FeedbackVerdict::Noise,
            source: FeedbackSource::Measured,
            injected: Some(39_852),
            opened: Some(1_095),
            used: Some(5),
        };
        legacy.occurred_at = at(1, 13);
        let mut repeated = legacy.clone();
        repeated.id = "qe-lifetime-day-two".into();
        repeated.occurred_at = at(2, 8);
        cas_store::import_quality_events(&cas_root, &[other_project, legacy, repeated], "acme/this").unwrap();

        let receipt = repo.join("release-latency.receipt");
        std::fs::write(
            &receipt,
            "TAG=v1.1.0\nMAIN_LANDED_AT=2026-10-02T12:00:00Z\n\
             PUBLISHED_AT=2026-10-02T12:09:00Z\nINTERVENTIONS=2\n",
        )
        .unwrap();
        assert_eq!(record_release(&cas_root, repo, &receipt).unwrap(), "v1.1.0");
        record_release(&cas_root, repo, &receipt).unwrap();

        let report = build_report(&cas_root, repo, Some("v1.1.0")).unwrap();
        assert_eq!(report.interventions.get("rebind"), Some(&1), "{report:?}");
        assert_eq!(report.main_to_published_minutes, Some(9.0));
        assert_eq!(report.commit_to_main_minutes, Some(0.0));
        assert_eq!(report.projects.len(), 2, "{report:?}");
        assert_eq!(report.escapes.len(), 1, "{report:?}");
        let feature = &report.features["ambient_recall"];
        assert_eq!((feature.injected, feature.opened, feature.used), (283, 1, 0));
        assert_eq!(feature.measured.get("noise"), Some(&1));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["excluded_unscoped_measurements"], 2);
        let text = render(&report);
        for expected in [
            "Friction ledger: v1.1.0",
            "Interventions: 1",
            "main to publish 9 min",
            "ambient_recall: measured noise 1 (283 injected, 1 opened, 0 used)",
            "Excluded 2 unscoped measurement(s); may include lifetime totals.",
        ] {
            assert!(text.contains(expected), "missing {expected}:\n{text}");
        }

        let unreleased = build_report(&cas_root, repo, None).unwrap();
        assert!(!unreleased.published);
        assert_eq!(unreleased.interventions.get("rebind"), Some(&1), "{unreleased:?}");
    }
}
