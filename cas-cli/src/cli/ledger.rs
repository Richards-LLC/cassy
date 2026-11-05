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
    /// Print wakes, usage, interventions, speed, escapes, friction found and feature verdicts for a release
    Report {
        /// Release tag, vX.Y.Z. Omit for work since the last release.
        #[arg(long)]
        release: Option<String>,
        /// Read local rows only; do not pull from Cloud first
        #[arg(long)]
        offline: bool,
    },
    /// Turn history already on disk into events, including filed Cassy problems (safe to rerun)
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
            let since = Utc::now() - Duration::days(*days);
            let inserted = crate::quality_ledger::backfill(cas_root, since)?;
            let gh = std::env::var_os(crate::github_issue_attach::GH_BIN_ENV)
                .filter(|value| !value.is_empty())
                .map_or_else(|| std::path::PathBuf::from("gh"), std::path::PathBuf::from);
            let friction = crate::quality_ledger::backfill_friction(cas_root, since, &gh)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "inserted": inserted + friction.tasks + friction.issues,
                        "friction_tasks": friction.tasks,
                        "friction_issues": friction.issues,
                        "friction_issues_skipped": friction.issues_skipped,
                    })
                );
            } else {
                println!("Recorded {inserted} events from the last {days} days of history.");
                println!(
                    "Recorded {} friction row(s): {} Cassy bug task(s), {} Cassy issue(s).",
                    friction.tasks + friction.issues,
                    friction.tasks,
                    friction.issues
                );
                if let Some(reason) = &friction.issues_skipped {
                    println!("Cassy issues skipped: {reason}");
                }
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

/// One identified Cassy problem and where it stands now (cas-87a2).
#[derive(Debug, Serialize)]
struct FrictionRow {
    reference: String,
    severity: Option<String>,
    title: Option<String>,
    project: Option<String>,
    link: Option<String>,
    /// open, in progress, blocked, awaiting merge, fixed, closed, cancelled or filed.
    state: &'static str,
    status: String,
}

#[derive(Debug, Default, Serialize)]
struct CheckRow {
    pass: u32,
    fail: u32,
    flaky: u32,
    error: u32,
}

/// Sums of measured usage, with explicit counts for missing measurements.
#[derive(Debug, Default, Serialize)]
struct TurnTotals {
    wakes: u64,
    no_op_wakes: u64,
    input_tokens: u64,
    output_tokens: u64,
    minutes: f64,
    missing_tokens: u64,
    missing_minutes: u64,
}

impl TurnTotals {
    fn add(&mut self, no_op: bool, input: Option<u64>, output: Option<u64>, minutes: Option<f64>) {
        self.wakes += 1;
        self.no_op_wakes += u64::from(no_op);
        self.input_tokens = self.input_tokens.saturating_add(input.unwrap_or(0));
        self.output_tokens = self.output_tokens.saturating_add(output.unwrap_or(0));
        self.minutes += minutes.unwrap_or(0.0);
        self.missing_tokens += u64::from(input.is_none() || output.is_none());
        self.missing_minutes += u64::from(minutes.is_none());
    }
}

#[derive(Debug, Serialize)]
struct LedgerReport {
    release: String,
    published: bool,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    events: usize,
    projects: Vec<String>,
    turns: TurnTotals,
    turns_by_channel: BTreeMap<&'static str, TurnTotals>,
    interventions_total: u32,
    interventions: BTreeMap<&'static str, u32>,
    commit_to_main_minutes: Option<f64>,
    main_to_published_minutes: Option<f64>,
    ejections: u32,
    escapes: Vec<EscapeRow>,
    checks: BTreeMap<String, CheckRow>,
    features: BTreeMap<String, FeatureRow>,
    excluded_unscoped_measurements: usize,
    friction: Vec<FrictionRow>,
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
    let mut report = aggregate(name, published, from, to, &events);
    report.friction = friction_rows(cas_root, repo, &events);
    Ok(report)
}

/// Where a task stands: `fixed in vX` once a release tag holds its delivery.
fn task_state(repo: &Path, task: &cas_types::Task) -> (&'static str, String) {
    use cas_types::TaskStatus;
    match task.status {
        TaskStatus::Closed => crate::issue_reconcile::delivery_commits(task)
            .into_iter()
            .find_map(|sha| crate::issue_reconcile::release_tag_containing(repo, sha))
            .map_or(("closed", "closed, not in a release yet".to_string()), |tag| {
                ("fixed", format!("fixed in {tag}"))
            }),
        TaskStatus::Open => ("open", "open".into()),
        TaskStatus::InProgress => ("in progress", "in progress".into()),
        TaskStatus::Blocked => ("blocked", "blocked".into()),
        TaskStatus::AwaitingMerge => ("awaiting merge", "awaiting merge".into()),
        TaskStatus::Cancelled => ("cancelled", "cancelled".into()),
    }
}

/// cas-87a2: every identified problem in the window with its found→fixed
/// status. An issue whose local task has its own row is counted once.
fn friction_rows(cas_root: &Path, repo: &Path, events: &[QualityEvent]) -> Vec<FrictionRow> {
    use cas_store::FrictionSource;
    let tasks: BTreeMap<String, cas_types::Task> = crate::store::open_task_store(cas_root)
        .and_then(|store| store.list(None))
        .map(|tasks| tasks.into_iter().map(|task| (task.id.clone(), task)).collect())
        .unwrap_or_default();
    let default_repo = crate::github_issue_attach::default_repo_for(cas_root);
    let task_rows: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            QualityPayload::Friction { source: FrictionSource::Task, reference, .. }
                if event.project_id.is_empty() => Some(reference.as_str()),
            _ => None,
        })
        .collect();
    let mut rows = Vec::new();
    for event in events {
        let QualityPayload::Friction { source, reference, severity, project } = &event.payload
        else {
            continue;
        };
        let (state, status) = if !event.project_id.is_empty() {
            ("filed", format!("reported by {}", event.project_id))
        } else if *source == FrictionSource::Task {
            tasks.get(reference).map_or(("filed", "task not found here".into()), |task| {
                task_state(repo, task)
            })
        } else {
            let fixing = tasks.values().find(|task| {
                crate::issue_reconcile::task_issue_refs(task, default_repo.as_deref())
                    .iter()
                    .any(|issue| {
                        format!("{}/{}/issues/{}", issue.owner, issue.repo, issue.number)
                            .eq_ignore_ascii_case(reference)
                    })
            });
            if fixing.is_some_and(|task| task_rows.contains(&task.id.as_str())) {
                continue;
            }
            fixing.map_or(("filed", "filed; no task here yet".into()), |task| {
                let (state, status) = task_state(repo, task);
                (state, format!("{status} ({})", task.id))
            })
        };
        rows.push(FrictionRow {
            reference: reference.clone(),
            severity: severity.clone(),
            title: event.title.clone(),
            project: project.clone(),
            link: event.link.clone(),
            state,
            status,
        });
    }
    rows
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
        turns: TurnTotals::default(),
        turns_by_channel: BTreeMap::new(),
        interventions_total: 0,
        interventions: BTreeMap::new(),
        commit_to_main_minutes: None,
        main_to_published_minutes: None,
        ejections: 0,
        escapes: Vec::new(),
        checks: BTreeMap::new(),
        features: BTreeMap::new(),
        excluded_unscoped_measurements: 0,
        friction: Vec::new(),
    };
    for event in events {
        let project = project_name(event);
        if !report.projects.contains(&project) {
            report.projects.push(project.clone());
        }
        match &event.payload {
            QualityPayload::Turn { woken_by, no_op, input_tokens, output_tokens, minutes, .. } => {
                report.turns.add(*no_op, *input_tokens, *output_tokens, *minutes);
                report.turns_by_channel.entry(woken_by.as_str()).or_default()
                    .add(*no_op, *input_tokens, *output_tokens, *minutes);
            }
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
            // Status needs the task store; build_report resolves these rows.
            QualityPayload::Friction { .. } => {}
            QualityPayload::FeatureFeedback { feature, verdict, source, injected, opened, used } => {
                // Old backfills dated lifetime totals as new measured rows.
                // Their keys were hashed and the rows may already be synced, so
                // use the missing actor to conservatively exclude unknown scope.
                // The real SessionEnd emitter always supplies its ending agent.
                if *source == FeedbackSource::Measured && event.actor.is_none() {
                    report.excluded_unscoped_measurements += 1;
                    continue;
                }
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
    // Recovered facts can arrive after an immutable release snapshot. Keep
    // snapshot counts as a floor when older relay history has been pruned.
    let observed_ejections = events.iter().filter(|event| event.project_id.is_empty()
        && matches!(&event.payload, QualityPayload::CheckRun { check_id, stage: cas_store::CheckStage::Queue,
            outcome: CheckOutcome::Fail, .. } if check_id == crate::quality_ledger::MERGE_QUEUE_CHECK)).count() as u32;
    report.ejections = report.ejections.max(observed_ejections);
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

    let _ = writeln!(out, "\nModel turns: {} wakes, {} no-op wakes", report.turns.wakes, report.turns.no_op_wakes);
    let _ = writeln!(out, "  measured usage: {} input tokens, {} output tokens, {:.2} minutes",
        report.turns.input_tokens, report.turns.output_tokens, report.turns.minutes);
    let _ = writeln!(out, "  missing usage: {} turn(s) without tokens, {} without minutes",
        report.turns.missing_tokens, report.turns.missing_minutes);
    for (channel, row) in &report.turns_by_channel {
        let _ = writeln!(out, "  {channel}: {} wakes, {} no-op", row.wakes, row.no_op_wakes);
        let _ = writeln!(out, "    {} input, {} output tokens; {:.2} minutes", row.input_tokens, row.output_tokens, row.minutes);
        if row.missing_tokens > 0 || row.missing_minutes > 0 {
            let _ = writeln!(out, "    missing tokens: {}; missing minutes: {}", row.missing_tokens, row.missing_minutes);
        }
    }

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

    let mut states: BTreeMap<&'static str, u32> = BTreeMap::new();
    for row in &report.friction {
        *states.entry(row.state).or_default() += 1;
    }
    let _ = writeln!(out, "\nFriction found: {}", report.friction.len());
    if !states.is_empty() {
        let _ = writeln!(out, "  {}", tally(&states));
    }
    for row in &report.friction {
        let severity = row.severity.as_deref().map(|s| format!(" [{s}]")).unwrap_or_default();
        let _ = writeln!(
            out,
            "  {}{severity} {}: {}",
            row.reference,
            row.title.as_deref().unwrap_or("untitled"),
            row.status,
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
    if report.excluded_unscoped_measurements > 0 {
        let _ = writeln!(out,
            "  Excluded {} unscoped measurement(s); may include lifetime totals.",
            report.excluded_unscoped_measurements,
        );
    }
    if report.features.is_empty() {
        let _ = writeln!(out, "  {}", if report.excluded_unscoped_measurements == 0 {
            "none recorded"
        } else { "none with known scope" });
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

    /// cas-87a2: filed Cassy problems become friction rows, a rerun adds none,
    /// and the report shows each with its found→fixed status. An issue whose
    /// local task has its own row counts once; feature requests are not friction.
    #[test]
    fn friction_backfill_and_report_show_found_to_fixed() {
        use cas_types::{Priority, Task, TaskStatus, TaskType};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        std::fs::create_dir_all(repo.join("cas-cli")).unwrap();
        std::fs::write(repo.join("cas-cli/Cargo.toml"), "[package]\nname = \"cas\"\n").unwrap();
        git_in(repo, &["init", "-q", "-b", "main"]);
        let commit = Command::new("git")
            .current_dir(repo)
            .args(["commit", "-q", "--allow-empty", "-m", "fix"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .env("GIT_AUTHOR_DATE", "2026-10-01T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-10-01T12:00:00Z")
            .status()
            .unwrap();
        assert!(commit.success());
        git_in(repo, &["tag", "v1.0.0"]);
        let fix = git(repo, &["rev-parse", "HEAD"]).unwrap();

        let cas_root = crate::store::init_cas_dir(repo).unwrap();
        let mut config = crate::config::Config::load(&cas_root).unwrap_or_default();
        config.set("issues.components.cassy", "owner/cassy").unwrap();
        config.save(&cas_root).unwrap();
        let store = crate::store::open_task_store(&cas_root).unwrap();
        let mut fixed = Task::new("cas-f1x".into(), "Shutdown loses work".into());
        fixed.task_type = TaskType::Bug;
        fixed.priority = Priority(0);
        fixed.status = TaskStatus::Closed;
        fixed.deliverables.merge_commit = Some(fix);
        let mut open = Task::new("cas-0pen".into(), "Status lies (owner/cassy#7)".into());
        open.task_type = TaskType::Bug;
        let feature = Task::new("cas-feat".into(), "A new idea".into());
        for task in [&fixed, &open, &feature] {
            store.add(task).unwrap();
        }

        let since = Utc::now() - Duration::days(30);
        let missing = crate::quality_ledger::backfill_friction(&cas_root, since, Path::new("/nonexistent/gh")).unwrap();
        assert_eq!((missing.tasks, missing.issues), (2, 0));
        assert!(missing.issues_skipped.is_some_and(|reason| reason.contains("gh issue list")));

        let created = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let issues = format!(
            r#"[{{"url":"https://github.com/owner/cassy/issues/7","title":"Status lies","createdAt":"{created}","labels":[]}},
               {{"url":"https://github.com/owner/cassy/issues/8","title":"Exit strands commits","createdAt":"{created}","labels":[{{"name":"bug"}}]}},
               {{"url":"https://github.com/owner/cassy/issues/9","title":"Wish","createdAt":"{created}","labels":[{{"name":"enhancement"}}]}}]"#
        );
        let gh = repo.join("fake-gh");
        std::fs::write(repo.join("issues.json"), issues).unwrap();
        std::fs::write(&gh, format!("#!/bin/sh\ncat '{}'\n", repo.join("issues.json").display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = crate::quality_ledger::backfill_friction(&cas_root, since, &gh).unwrap();
        assert_eq!(found, crate::quality_ledger::FrictionBackfill { tasks: 0, issues: 2, issues_skipped: None });
        assert_eq!(crate::quality_ledger::backfill_friction(&cas_root, since, &gh).unwrap().issues, 0);

        let report = build_report(&cas_root, repo, None).unwrap();
        let text = render(&report);
        assert!(text.contains("Friction found: 3"), "{text}");
        assert!(text.contains("cas-f1x [p0] Shutdown loses work: fixed in v1.0.0"), "{text}");
        assert!(text.contains("cas-0pen [p2]"), "{text}");
        assert!(text.contains("owner/cassy/issues/8 Exit strands commits: filed; no task here yet"), "{text}");
        assert!(!text.contains("issues/7 ") && !text.contains("issues/9"), "{text}");
        assert!(text.contains("filed 1, fixed 1, open 1"), "{text}");
    }

    /// The train's receipt becomes a release event, and the report for that
    /// release shows its speed, the window's interventions and the fleet's rows.
    #[test]
    fn receipt_becomes_a_release_and_the_report_reads_the_window() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
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
        for (key, occurred_at, channel, no_op, usage) in [
            ("old turn", at(1, 6), cas_store::WakeChannel::Director, false, Some(900)),
            ("acted turn", at(2, 6), cas_store::WakeChannel::Director, false, Some(30)),
            ("no-op turn", at(2, 7), cas_store::WakeChannel::Worker, true, None),
            ("future turn", at(3, 6), cas_store::WakeChannel::Director, false, Some(900)),
        ] {
            let mut event = NewQualityEvent::new(key, QualityPayload::Turn {
                woken_by: channel, actions: if no_op { vec![] } else { vec![cas_store::TurnAction::TaskMutation] },
                no_op, input_tokens: usage, output_tokens: usage.map(|_| 5), minutes: usage.map(|_| 0.5),
            });
            event.occurred_at = occurred_at;
            crate::quality_ledger::emit(&cas_root, &event);
        }
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
        let mut supervisor = repeated.clone();
        supervisor.id = "qe-supervisor".into();
        supervisor.payload = QualityPayload::FeatureFeedback {
            feature: "handoff".into(), verdict: FeedbackVerdict::Helped,
            source: FeedbackSource::Supervisor, injected: None, opened: None, used: None,
        };
        cas_store::import_quality_events(&cas_root, &[other_project, legacy, repeated, supervisor], "acme/this").unwrap();

        let receipt = repo.join("release-latency.receipt");
        std::fs::write(
            &receipt,
            "TAG=v1.1.0\nMAIN_LANDED_AT=2026-10-02T12:00:00Z\n\
             PUBLISHED_AT=2026-10-02T12:09:00Z\nINTERVENTIONS=2\n",
        )
        .unwrap();
        assert_eq!(record_release(&cas_root, repo, &receipt).unwrap(), "v1.1.0");
        record_release(&cas_root, repo, &receipt).unwrap();

        // cas-c28c: an already-consumed ejection relay predates the ledger.
        // Backfill must recover its original time, then the report must count
        // it even though the immutable release snapshot recorded zero.
        git_in(repo, &["remote", "add", "origin", "https://github.com/example/project.git"]);
        // cas-df19: v3.94-3.98 already emitted this fact under its queue-exit
        // alias. The append-only ledger retains that row across the upgrade.
        let mut old_ejection = NewQualityEvent::new(
            "merge_queue|cas-legacy|1447|queue-exit:2026-10-02T05:00:00Z:merge-group-run:41246801444",
            QualityPayload::CheckRun { check_id: crate::quality_ledger::MERGE_QUEUE_CHECK.into(),
                stage: cas_store::CheckStage::Queue, outcome: CheckOutcome::Fail,
                sha: None, minutes: None },
        );
        old_ejection.task_id = Some("cas-legacy".into());
        old_ejection.occurred_at = at(2, 5);
        // The old emitter stored no run link; identity must use its hashed key.
        assert!(crate::quality_ledger::emit(&cas_root, &old_ejection));
        let queue = crate::store::open_supervisor_queue_store(&cas_root).unwrap();
        for (task, pr) in [("cas-release", 1446), ("cas-legacy", 1447)] {
        for occurrence in ["queue-exit:2026-10-02T05:00:00Z:merge-group-run:41246801444", "merge-group-run:41246801444"] {
        let relay = queue.notify("old-supervisor", "merge_queue_ejected", &serde_json::json!({
            "task_id":task, "pr_number":pr, "failed_run_id":41246801444_u64,
            "occurrence":occurrence, "failing_job":"Rust tests shard 1"
        }).to_string(), cas_store::NotificationPriority::High).unwrap();
        queue.mark_prompt_delivered(relay).unwrap();
        queue.ack(relay).unwrap();
        rusqlite::Connection::open(cas_root.join("cas.db")).unwrap().execute(
            "UPDATE supervisor_queue SET created_at=?1 WHERE id=?2",
            rusqlite::params![at(2, 5).to_rfc3339(), relay]).unwrap();
        }
        }
        crate::quality_ledger::backfill(&cas_root, at(1, 12)).unwrap();
        assert_eq!(build_report(&cas_root, repo, Some("v1.1.0")).unwrap().ejections, 2,
            "historical queue ejection must survive a zero release snapshot");
        assert_eq!(crate::quality_ledger::backfill(&cas_root, at(1, 12)).unwrap(), 0,
            "replaying consumed relay history must not add another ejection");
        let recovered = cas_store::quality_events_for_task(&cas_root, "cas-release").unwrap();
        assert_eq!(recovered.len(), 1, "one ejection across both historical relay aliases");
        assert_eq!(recovered[0].occurred_at, at(2, 5));
        assert_eq!(recovered[0].title.as_deref(), Some("Rust tests shard 1"));
        assert_eq!(recovered[0].link.as_deref(), Some("https://github.com/example/project/actions/runs/41246801444"));
        let legacy = cas_store::quality_events_for_task(&cas_root, "cas-legacy").unwrap();
        assert_eq!(legacy.len(), 1, "backfill must preserve the old emitter's fact without adding another");
        assert!(legacy[0].link.is_none(), "append-only history must not be rewritten");

        let report = build_report(&cas_root, repo, Some("v1.1.0")).unwrap();
        assert_eq!(report.interventions.get("rebind"), Some(&1), "{report:?}");
        assert_eq!(report.main_to_published_minutes, Some(9.0));
        assert_eq!(report.commit_to_main_minutes, Some(0.0));
        assert_eq!(report.projects.len(), 2, "{report:?}");
        assert_eq!(report.escapes.len(), 1, "{report:?}");
        let feature = &report.features["ambient_recall"];
        assert_eq!((feature.injected, feature.opened, feature.used), (283, 1, 0));
        assert_eq!(feature.measured.get("noise"), Some(&1));
        assert_eq!(report.features["handoff"].supervisors.get("helped"), Some(&1));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["excluded_unscoped_measurements"], 2);
        assert_eq!(json["turns"]["wakes"], 2);
        assert_eq!(json["turns"]["no_op_wakes"], 1);
        assert_eq!(json["turns"]["input_tokens"], 30);
        assert_eq!(json["turns"]["output_tokens"], 5);
        assert_eq!(json["turns"]["minutes"], 0.5);
        assert_eq!(json["turns"]["missing_tokens"], 1);
        assert_eq!(json["turns_by_channel"]["director"]["wakes"], 1);
        let text = render(&report);
        for expected in [
            "Friction ledger: v1.1.0",
            "Interventions: 1",
            "Model turns: 2 wakes, 1 no-op wakes",
            "measured usage: 30 input tokens, 5 output tokens, 0.50 minutes",
            "missing usage: 1 turn(s) without tokens, 1 without minutes",
            "main to publish 9 min",
            "ambient_recall: measured noise 1 (283 injected, 1 opened, 0 used)",
            "Excluded 2 unscoped measurement(s); may include lifetime totals.",
        ] {
            assert!(text.contains(expected), "missing {expected}:\n{text}");
        }

        let unreleased = build_report(&cas_root, repo, None).unwrap();
        assert!(!unreleased.published);
        assert_eq!(unreleased.interventions.get("rebind"), Some(&1), "{unreleased:?}");
        let mut empty = unreleased;
        empty.features.clear();
        for excluded in [0, 2] {
            empty.excluded_unscoped_measurements = excluded;
            let text = render(&empty);
            assert!(text.contains(if excluded == 0 {
                "Feature feedback\n  none recorded"
            } else { "none with known scope" }), "{text}");
        }

        // A fresh immutable release snapshot must also count the legacy run
        // once, independently of the report's observed-ejection floor above.
        git_in(repo, &["tag", "v1.1.1", "v1.1.0"]);
        std::fs::write(&receipt, "TAG=v1.1.1\nPUBLISHED_AT=2026-10-02T12:10:00Z\n").unwrap();
        record_release(&cas_root, repo, &receipt).unwrap();
        let release = cas_store::quality_release_events(&cas_root).unwrap().into_iter()
            .find(|event| event.release_version.as_deref() == Some("v1.1.1")).unwrap();
        assert!(matches!(release.payload, QualityPayload::Release { ejections: 2, .. }),
            "release snapshot double-counted the old emitter and recovered alias: {release:?}");
    }
}
