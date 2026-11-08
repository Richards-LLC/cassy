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
        /// Push/pull the ledger after recording local history
        #[arg(long)]
        sync: bool,
    },
    /// Give a measured range and record who received it; use --preview for a read
    Estimate {
        /// pipeline, task, epic or release
        kind: String,
        /// Stage name, task id or release version
        subject: String,
        /// Bind a pipeline promise to the SHA whose completion will settle it
        #[arg(long)]
        instance: Option<String>,
        #[arg(long, default_value = "operator")]
        recipient: String,
        #[arg(long, default_value_t = 20)]
        runs: usize,
        #[arg(long)]
        preview: bool,
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
        LedgerCommand::Backfill { days,sync } => {
            let since = Utc::now() - Duration::days(*days);
            let inserted = crate::quality_ledger::backfill(cas_root, since)?;
            let gh = std::env::var_os(crate::github_issue_attach::GH_BIN_ENV)
                .filter(|value| !value.is_empty())
                .map_or_else(|| std::path::PathBuf::from("gh"), std::path::PathBuf::from);
            let pipeline = crate::quality_ledger::estimates::github_history(cas_root, 20, since);
            let (pipeline_inserted, pipeline_skipped) = match pipeline {
                Ok(n) => (n,None), Err(error) => (0,Some(error.to_string())),
            };
            let friction = crate::quality_ledger::backfill_friction(cas_root, since, &gh)?;
            let cloud_sync=if *sync { sync_ledger(cas_root)? } else { None };
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "cloud_sync": cloud_sync,
                        "inserted": inserted + pipeline_inserted + friction.tasks + friction.issues,
                        "pipeline_samples": pipeline_inserted,
                        "pipeline_skipped": pipeline_skipped,
                        "friction_tasks": friction.tasks,
                        "friction_issues": friction.issues,
                        "friction_issues_skipped": friction.issues_skipped,
                    })
                );
            } else {
                if *sync { println!("Cloud sync: {}",cloud_sync.map_or_else(||"not logged in".into(),|s|format!("{} pushed, {} pulled, {} refused, unavailable={}",s.pushed,s.pulled,s.refused,s.capability_absent))); }
                println!("Recorded {pipeline_inserted} measured pipeline samples.");
                if let Some(reason)=pipeline_skipped { println!("Pipeline history skipped: {reason}"); }
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
        LedgerCommand::Estimate { kind,subject,instance,recipient,runs,preview } => {
            if kind == "pipeline" && !preview && instance.is_none() {
                bail!("pipeline estimates need --instance <sha> so completion can settle the promise; use --preview for a read");
            }
            let result=crate::quality_ledger::estimates::estimate(cas_root,kind,subject,instance.as_deref(),recipient,*runs,!preview)?;
            if cli.json { println!("{}",serde_json::to_string_pretty(&result)?); }
            else { println!("{}",result.render());
                for reference in result.references { println!("  {}: {:.1} min",reference.id,reference.minutes); }
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
    if let Err(error)=sync_ledger(cas_root) { tracing::warn!(%error,"ledger report: showing local rows; cloud sync failed"); }
}
fn sync_ledger(cas_root:&Path)->Result<Option<crate::cloud::syncer::quality_events::QualitySyncSummary>> {
    use crate::cloud::{CloudConfig,CloudSyncer,CloudSyncerConfig,SyncQueue};
    let config=CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    if !config.is_logged_in() { return Ok(None); }
    let queue=std::sync::Arc::new(SyncQueue::open(cas_root)?); queue.init()?;
    let project_id=crate::cloud::resolve_canonical_id_for_sync(cas_root)?;
    let syncer=CloudSyncer::new_for_project(queue,config,CloudSyncerConfig::default(),project_id,cas_root);
    Ok(Some(syncer.sync_quality_events(true)?))
}

/// Trimmed stdout of a successful git command, empty output included.
fn git_output(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").current_dir(repo).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    git_output(repo, args).filter(|text| !text.is_empty())
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

pub(crate) fn median(mut values: Vec<f64>) -> Option<f64> {
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

/// A measured value, or the reason it could not be measured. Missing data is
/// never reported as zero.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
enum Measure<T> {
    Known(T),
    Unknown { unknown: String },
}

fn unknown<T>(reason: impl Into<String>) -> Measure<T> {
    Measure::Unknown { unknown: reason.into() }
}

impl<T> Measure<T> {
    fn render(&self, known: impl FnOnce(&T) -> String) -> String {
        match self {
            Measure::Known(value) => known(value),
            Measure::Unknown { unknown } => format!("unknown ({unknown})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct LandingSpread {
    median_minutes: f64,
    p90_minutes: f64,
    deliveries: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct LineChurn {
    added: u64,
    removed: u64,
}

/// New checks: net `#[test]` attributes in Rust sources plus CI workflow jobs
/// that did not exist at the start of the range.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct NewChecks {
    total: i64,
    tests: i64,
    ci_jobs: i64,
}

#[derive(Debug, Serialize)]
struct SpeedRow {
    commit_to_main: Measure<LandingSpread>,
    main_to_published_minutes: Measure<f64>,
}

#[derive(Debug, Serialize)]
struct ComplexityRow {
    lines: Measure<LineChurn>,
    config_keys: Measure<i64>,
    checks: Measure<NewChecks>,
}

/// Where `cas config set` keys are declared (`ConfigMeta { key: .. }`).
const CONFIG_REGISTRY_PATH: &str = "cas-cli/src/config/meta";
const WORKFLOWS_PATH: &str = ".github/workflows";

/// Git revisions bounding a release: its previous tag to its tag, or the
/// newest tag to HEAD for work in progress.
fn release_range(repo: &Path, release: &str, published: bool) -> Result<(String, String), String> {
    if !published {
        let newest = git(repo, &["describe", "--tags", "--abbrev=0"])
            .ok_or("no release tag bounds the work in progress")?;
        return Ok((newest, "HEAD".to_string()));
    }
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("{release}^{{commit}}")])
        .ok_or_else(|| format!("tag {release} is not in this repository"))?;
    let previous = previous_tag(repo, release).ok_or_else(|| format!("no earlier tag bounds {release}"))?;
    Ok((previous, release.to_string()))
}


/// Per delivery (each first-parent landing on main), minutes from its earliest
/// authored commit to the landing.
fn delivery_landing_spread(repo: &Path, from: &str, to: &str) -> Measure<LandingSpread> {
    let Some(landings) =
        git_output(repo, &["log", "--first-parent", "--format=%H %cI", &format!("{from}..{to}")])
    else {
        return unknown(format!("git log {from}..{to} failed"));
    };
    let mut minutes: Vec<f64> = landings
        .lines()
        .filter_map(|landing| {
            let (sha, landed_at) = landing.split_once(' ')?;
            let landed_at = parse_time(landed_at)?;
            let authored = git(repo, &["log", "--no-merges", "--format=%aI", &format!("{sha}^1..{sha}")])?;
            let first = authored.lines().filter_map(parse_time).min()?;
            Some(minutes_between(first, landed_at))
        })
        .collect();
    if minutes.is_empty() {
        return unknown(format!("no deliveries landed in {from}..{to}"));
    }
    minutes.sort_by(f64::total_cmp);
    Measure::Known(LandingSpread {
        median_minutes: median(minutes.clone()).unwrap_or_default(),
        p90_minutes: crate::quality_ledger::estimates::percentile(&minutes, 0.9),
        deliveries: minutes.len(),
    })
}

/// Minutes from the release commit to publication: the release record's own
/// measure, else its publish time, else the annotated tag's date.
fn main_to_published(
    repo: &Path,
    release: &str,
    published: bool,
    recorded: Option<f64>,
    published_at: Option<DateTime<Utc>>,
) -> Measure<f64> {
    if !published {
        return unknown("not published yet");
    }
    if let Some(minutes) = recorded {
        return Measure::Known(minutes);
    }
    let Some(released) = tag_time(repo, release) else {
        return unknown(format!("tag {release} is not in this repository"));
    };
    let tagged = || {
        git(repo, &["for-each-ref", "--format=%(taggerdate:iso-strict)", &format!("refs/tags/{release}")])
            .and_then(|date| parse_time(&date))
    };
    match published_at.or_else(tagged) {
        Some(at) => Measure::Known(minutes_between(released, at)),
        None => unknown(format!("no release record, and {release} is a lightweight tag")),
    }
}

fn line_churn(repo: &Path, from: &str, to: &str) -> Measure<LineChurn> {
    let Some(stat) = git_output(repo, &["diff", "--shortstat", from, to]) else {
        return unknown(format!("git diff {from} {to} failed"));
    };
    let count = |suffix: &str| -> u64 {
        stat.split(',')
            .find(|part| part.contains(suffix))
            .and_then(|part| part.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    Measure::Known(LineChurn { added: count("insertion"), removed: count("deletion") })
}

fn config_keys_at(repo: &Path, rev: &str) -> Option<std::collections::BTreeSet<String>> {
    git_output(repo, &["cat-file", "-e", &format!("{rev}:{CONFIG_REGISTRY_PATH}")])?;
    // `git grep` exits 1 when nothing matches: a registry with no keys yet.
    let found = git_output(repo, &["grep", "-h", "-o", "-E", r#"key: *"[^"]+""#, rev, "--", CONFIG_REGISTRY_PATH])
        .unwrap_or_default();
    Some(found.lines().filter_map(|line| line.split('"').nth(1).map(str::to_string)).collect())
}

fn new_config_keys(repo: &Path, from: &str, to: &str) -> Measure<i64> {
    let Some(after) = config_keys_at(repo, to) else {
        return unknown(format!("no config registry at {CONFIG_REGISTRY_PATH}"));
    };
    let before = config_keys_at(repo, from).unwrap_or_default();
    Measure::Known(after.difference(&before).count() as i64)
}

/// Job ids under a workflow's top-level `jobs:` map.
fn workflow_jobs(text: &str) -> Vec<String> {
    let mut jobs = Vec::new();
    let mut indent: Option<usize> = None;
    let mut in_jobs = false;
    for line in text.lines() {
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let depth = line.len() - body.len();
        if depth == 0 {
            in_jobs = line.trim_end() == "jobs:";
            continue;
        }
        if in_jobs && *indent.get_or_insert(depth) == depth
            && let Some((name, _)) = body.split_once(':')
        {
            jobs.push(name.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    jobs
}

fn ci_jobs_at(repo: &Path, rev: &str) -> std::collections::BTreeSet<String> {
    let files = git_output(repo, &["ls-tree", "-r", "--name-only", rev, "--", WORKFLOWS_PATH]).unwrap_or_default();
    files
        .lines()
        .filter(|file| file.ends_with(".yml") || file.ends_with(".yaml"))
        .flat_map(|file| {
            let text = git_output(repo, &["show", &format!("{rev}:{file}")]).unwrap_or_default();
            workflow_jobs(&text).into_iter().map(move |job| format!("{file}#{job}"))
        })
        .collect()
}

fn new_checks(repo: &Path, from: &str, to: &str) -> Measure<NewChecks> {
    let Some(diff) = git_output(repo, &["diff", "-U0", from, to, "--", "*.rs"]) else {
        return unknown(format!("git diff {from} {to} failed"));
    };
    let is_test = |line: &str| {
        let line = line.trim_start();
        line.starts_with("#[test]") || line.starts_with("#[tokio::test")
    };
    let tests = diff
        .lines()
        .map(|line| match line.split_at_checked(1) {
            Some(("+", body)) if !line.starts_with("+++") && is_test(body) => 1,
            Some(("-", body)) if !line.starts_with("---") && is_test(body) => -1,
            _ => 0,
        })
        .sum::<i64>();
    let before = ci_jobs_at(repo, from);
    let ci_jobs = ci_jobs_at(repo, to).difference(&before).count() as i64;
    Measure::Known(NewChecks { total: tests + ci_jobs, tests, ci_jobs })
}

fn release_shape(
    repo: &Path,
    release: &str,
    published: bool,
    recorded_publish_minutes: Option<f64>,
    published_at: Option<DateTime<Utc>>,
) -> (SpeedRow, ComplexityRow) {
    let main_to_published_minutes =
        main_to_published(repo, release, published, recorded_publish_minutes, published_at);
    match release_range(repo, release, published) {
        Ok((from, to)) => (
            SpeedRow { commit_to_main: delivery_landing_spread(repo, &from, &to), main_to_published_minutes },
            ComplexityRow {
                lines: line_churn(repo, &from, &to),
                config_keys: new_config_keys(repo, &from, &to),
                checks: new_checks(repo, &from, &to),
            },
        ),
        Err(reason) => (
            SpeedRow { commit_to_main: unknown(reason.clone()), main_to_published_minutes },
            ComplexityRow { lines: unknown(reason.clone()), config_keys: unknown(reason.clone()), checks: unknown(reason) },
        ),
    }
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
    // A full release sample uses the receipt's request-to-publication endpoint.
    if let Some(seconds)=fields.get("REQUEST_TO_PUBLISHED_SECS").and_then(|v|v.parse::<f64>().ok()) {
        let mut sample=cas_store::NewQualityEvent::new(format!("release_duration|{version}"),
            QualityPayload::CheckRun { check_id:"release_total".into(),stage:cas_store::CheckStage::Lane,
                outcome:CheckOutcome::Pass,sha:None,minutes:Some(seconds/60.0) });
        sample.occurred_at=published_at; sample.release_version=Some(version.clone());
        crate::quality_ledger::emit(cas_root,&sample);
    }
    crate::quality_ledger::estimates::complete(cas_root,"release",&version,published_at,None,Some(&version));
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
    /// Git-derived speed over the release range; `unknown` carries the reason.
    speed: SpeedRow,
    complexity: ComplexityRow,
    escapes: Vec<EscapeRow>,
    checks: BTreeMap<String, CheckRow>,
    features: BTreeMap<String, FeatureRow>,
    excluded_unscoped_measurements: usize,
    friction: Vec<FrictionRow>,
    estimates: crate::quality_ledger::estimates::Calibration,
    estimate_trend: Vec<(String,crate::quality_ledger::estimates::Calibration)>,
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
    let published_at = published
        .then(|| cas_store::quality_release_events(cas_root))
        .transpose()?
        .and_then(|releases| {
            releases
                .into_iter()
                .find(|event| event.release_version.as_deref() == Some(report.release.as_str()))
                .map(|event| event.occurred_at)
        });
    (report.speed, report.complexity) = release_shape(
        repo,
        &report.release,
        published,
        report.main_to_published_minutes,
        published_at,
    );
    report.friction = friction_rows(cas_root, repo, &events);
    let history=cas_store::quality_events_between(cas_root,None,None)?;
    let mut grouped=BTreeMap::<String,Vec<QualityEvent>>::new();
    let releases=cas_store::quality_release_events(cas_root)?;
    for event in history.into_iter().filter(|e| matches!(e.payload,QualityPayload::EstimateOutcome {..})) {
        let version=event.release_version.clone().or_else(|| releases.iter()
            .filter(|r| r.occurred_at>=event.occurred_at).min_by_key(|r|r.occurred_at)
            .and_then(|r| r.release_version.clone())).unwrap_or_else(||"unreleased".into());
        grouped.entry(version).or_default().push(event);
    }
    let mut grouped:Vec<_>=grouped.into_iter().collect();
    grouped.sort_by_key(|(_,events)|events.iter().map(|e|e.occurred_at).max());
    report.estimate_trend=grouped.into_iter().map(|(release,events)|
        (release,crate::quality_ledger::estimates::calibration(&events))).collect();
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
        .ok()
        .and_then(|store| store.list(None).ok())
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
        } else if *source == FrictionSource::Estimate {
            ("missed", "actual exceeded the bad-day estimate".into())
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
        speed: SpeedRow {
            commit_to_main: unknown("no repository read"),
            main_to_published_minutes: unknown("no repository read"),
        },
        complexity: ComplexityRow {
            lines: unknown("no repository read"),
            config_keys: unknown("no repository read"),
            checks: unknown("no repository read"),
        },
        escapes: Vec::new(),
        checks: BTreeMap::new(),
        features: BTreeMap::new(),
        excluded_unscoped_measurements: 0,
        friction: Vec::new(),
        estimates: crate::quality_ledger::estimates::calibration(events),
        estimate_trend: Vec::new(),
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
            QualityPayload::Friction { .. } | QualityPayload::Estimate { .. } | QualityPayload::EstimateOutcome { .. } => {}
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

    let minutes = |minutes: &f64| duration(Some(*minutes));
    let _ = writeln!(out, "\nSpeed");
    let _ = writeln!(out, "  commit to main  {}", report.speed.commit_to_main.render(|spread| format!(
        "median {}, p90 {} over {} deliveries",
        minutes(&spread.median_minutes), minutes(&spread.p90_minutes), spread.deliveries)));
    let _ = writeln!(out, "  main to publish {}", report.speed.main_to_published_minutes.render(minutes));
    if report.published {
        let _ = writeln!(out, "  queue ejections {}", report.ejections);
    }

    let _ = writeln!(out, "\nComplexity added");
    let _ = writeln!(out, "  lines           {}", report.complexity.lines.render(|churn| format!(
        "+{} / -{}", churn.added, churn.removed)));
    let _ = writeln!(out, "  config keys     {}", report.complexity.config_keys.render(|keys| format!("{keys} new")));
    let _ = writeln!(out, "  new checks      {}", report.complexity.checks.render(|checks| format!(
        "{} ({} tests, {} CI jobs)", checks.total, checks.tests, checks.ci_jobs)));

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

    let estimates=&report.estimates;
    let _ = writeln!(out,"\nEstimates: {} completed, {} hits, {} misses, {} below typical",
        estimates.completed,estimates.hits,estimates.misses.len(),estimates.below_typical);
    let _ = writeln!(out,"  hit rate: {}",estimates.hit_rate.map_or_else(||"uncalibrated: no completed estimates".into(),|r|format!("{:.0}%",r*100.0)));
    for miss in &estimates.misses { let _=writeln!(out,"  {}: {:.1} min ({}, estimate {})",miss.subject,miss.actual_minutes,miss.reason,miss.estimate_id); }
    for (release,row) in &report.estimate_trend { let _=writeln!(out,"  {release}: {}/{} hits, {} below typical",row.hits,row.completed,row.below_typical); }

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
    #[test]
    fn estimate_outcomes_link_and_report_hits_without_early_friction() {
        use crate::quality_ledger::estimates::{estimate,complete,calibration};
        let mut env=crate::test_support::TestEnvGuard::temp_home();
        let temp=tempfile::tempdir().unwrap();
        let root=crate::store::init_cas_dir(temp.path()).unwrap();
        env.set("CAS_ROOT",&root);
        for (i, m) in [8.0, 12.0, 16.0, 20.0, 31.0].into_iter().enumerate() {
            crate::quality_ledger::emit(&root,&NewQualityEvent::new(format!("ref-{i}"),QualityPayload::CheckRun {
                check_id:"merge_queue".into(),stage:cas_store::CheckStage::Queue,outcome:CheckOutcome::Pass,sha:None,minutes:Some(m) }));
        }
        let early = estimate(
            &root,
            "pipeline",
            "merge_queue",
            Some("aaa0001"),
            "client",
            20,
            true,
        )
        .unwrap();
        let late = estimate(
            &root,
            "pipeline",
            "merge_queue",
            Some("aaa0002"),
            "operator",
            20,
            true,
        )
        .unwrap();
        let at = Utc::now() + chrono::Duration::minutes(60);
        for _ in 0..2 {
            complete(
                &root,
                "pipeline",
                "merge_queue:aaa0001",
                at,
                Some(10.0),
                Some("v1.2.3"),
            );
            complete(
                &root,
                "pipeline",
                "merge_queue:aaa0002",
                at,
                Some(45.0),
                Some("v1.2.3"),
            );
        }
        let events = cas_store::quality_events_between(&root, None, None).unwrap();
        let tally = calibration(&events);
        assert_eq!(
            (
                tally.completed,
                tally.hits,
                tally.below_typical,
                tally.hit_rate
            ),
            (2, 1, 1, Some(0.5))
        );
        assert_eq!(tally.misses[0].estimate_id, late.estimate_id.unwrap());
        assert!(events.iter().any(|e| matches!(&e.payload,QualityPayload::EstimateOutcome { estimate_id,inside_range:true,below_typical:true,.. } if Some(estimate_id)==early.estimate_id.as_ref())));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.payload, QualityPayload::Friction { .. }))
                .count(),
            1
        );
        let report=super::aggregate("v1.2.3".into(),true,None,None,&events);
        let text=super::render(&report);
        assert!(text.contains("hit rate: 50%") && text.contains("above_p90"));
    }

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

    /// Runs git with fixed identity and the given author/committer dates.
    fn git_dated(repo: &Path, args: &[&str], authored: &str, committed: &str) {
        let status = Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .env("GIT_AUTHOR_DATE", authored)
            .env("GIT_COMMITTER_DATE", committed)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    const REGISTRY: &str = "cas-cli/src/config/meta/seed/keys.rs";
    const WORKFLOW: &str = ".github/workflows/ci.yml";

    /// v1.0.0 (lightweight) then three deliveries to v1.1.0 (annotated, tagged
    /// 15 min after its release commit): a merged branch whose first commit
    /// landed 120 min later, and two direct commits that landed after 30 and
    /// 10 min. Across the range: one new config key, two new `#[test]`s and
    /// one removed, one new CI job.
    fn release_fixture(repo: &Path) -> std::path::PathBuf {
        let write = |path: &str, text: &str| {
            let path = repo.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let at = |time: &str| format!("2026-10-01T{time}:00Z");
        let commit = |message: &str, authored: &str, committed: &str| {
            git_in(repo, &["add", "-A"]);
            git_dated(repo, &["commit", "-q", "-m", message], &at(authored), &at(committed));
        };
        git_in(repo, &["init", "-q", "-b", "main"]);
        write(REGISTRY, "    key: \"a.one\", section: \"a\",\n");
        write(WORKFLOW, "name: ci\njobs:\n  test:\n    runs-on: ubuntu\n");
        write("src/lib.rs", "#[test]\nfn old() {}\n");
        commit("base", "09:00", "09:00");
        git_in(repo, &["tag", "v1.0.0"]);

        git_in(repo, &["checkout", "-q", "-b", "feature"]);
        write("src/feature.rs", "#[test]\nfn one() {}\n");
        commit("feature one", "11:00", "11:00");
        write("src/feature.rs", "#[test]\nfn one() {}\n#[test]\nfn two() {}\n");
        commit("feature two", "11:30", "11:30");
        git_in(repo, &["checkout", "-q", "main"]);
        git_dated(repo, &["merge", "-q", "--no-ff", "-m", "land feature", "feature"], &at("13:00"), &at("13:00"));
        write(REGISTRY, "    key: \"a.one\", section: \"a\",\n    key: \"a.two\", section: \"a\",\n");
        commit("config key", "13:30", "14:00");
        write(WORKFLOW, "name: ci\njobs:\n  test:\n    runs-on: ubuntu\n  lint:\n    runs-on: ubuntu\n");
        write("src/lib.rs", "fn old() {}\n");
        commit("lint job", "14:00", "14:10");
        git_dated(repo, &["tag", "-a", "v1.1.0", "-m", "v1.1.0"], &at("14:25"), &at("14:25"));
        let cas_root = repo.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        cas_root
    }

    /// Commit to main is per delivery, from its first commit to its landing.
    #[test]
    fn report_shows_commit_to_main_median_and_p90_per_delivery() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let cas_root = release_fixture(dir.path());
        let report = build_report(&cas_root, dir.path(), Some("v1.1.0")).unwrap();
        assert_eq!(
            report.speed.commit_to_main,
            Measure::Known(LandingSpread { median_minutes: 30.0, p90_minutes: 120.0, deliveries: 3 })
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["speed"]["commit_to_main"]["p90_minutes"], 120.0);
        // The first release has no earlier tag to bound it.
        let first = build_report(&cas_root, dir.path(), Some("v1.0.0")).unwrap();
        assert!(matches!(first.speed.commit_to_main, Measure::Unknown { .. }));
    }

    /// Without a release record, an annotated tag dates the publication; a
    /// lightweight tag leaves it unknown rather than zero.
    #[test]
    fn report_shows_main_to_published_from_the_tag_or_why_not() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let cas_root = release_fixture(dir.path());
        let report = build_report(&cas_root, dir.path(), Some("v1.1.0")).unwrap();
        assert_eq!(report.speed.main_to_published_minutes, Measure::Known(15.0));
        let first = build_report(&cas_root, dir.path(), Some("v1.0.0")).unwrap();
        assert!(matches!(first.speed.main_to_published_minutes, Measure::Unknown { .. }));
    }

    /// Complexity added: line churn, new config registry keys, and new checks.
    #[test]
    fn report_shows_complexity_added_over_the_release() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let cas_root = release_fixture(dir.path());
        let report = build_report(&cas_root, dir.path(), Some("v1.1.0")).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["complexity"]["lines"], serde_json::json!({"added": 7, "removed": 1}));
        assert_eq!(json["complexity"]["config_keys"], 1);
        assert_eq!(json["complexity"]["checks"], serde_json::json!({"total": 2, "tests": 1, "ci_jobs": 1}));
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
        assert!(missing.issues_skipped.is_some());

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
