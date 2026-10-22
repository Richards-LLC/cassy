//! The release-gate run (finding 16): `cas qa release-gate --sha X`.
//!
//! One persisted run per SHA at `<cas_root>/release-gate/<sha>.json`:
//! preflight (inside `start`) → `frozen` → `suites` → `manual` → `sealed`,
//! or `aborted`. `cas serve`'s lease owner drives the automated suites; a
//! person records manual rows. While a run is live, factory merges into the
//! integration branch are refused (see [`active_freeze`]). Rows are written
//! after each walk, so a restart resumes and reruns only rows with no verdict.
use super::coverage;
use super::fixtures::{FixtureReceipt, FixtureSuite};
use super::flags::FlagSnapshot;
use super::pack::{self, Pack, Row, SuiteTiming};
use super::resources::{self, Meter, MeteredError, PlannedRow as PlannedUse, ResourcePlan};
use super::verdict::{Gate, GateVerdict, NotExercisedCause, RowVerdict};
use super::{
    CallError, ReleaseConfig, Request, ServedComponent, ServedRefusal, Verb, configured,
    preflight_served, printable,
};
use crate::qa_registry::{FixtureSource, Journey, Registry};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Run files live in `<cas_root>/release-gate/`.
pub const DIR: &str = "release-gate";
/// The refusal class an override reason must name to merge through a freeze.
pub const FREEZE_LABEL: &str = "RELEASE FREEZE";
/// Added to the suites' wall clocks for the freeze TTL.
const FREEZE_SLACK_MINS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Preflight passed; the freeze holds and no suite has started.
    Frozen,
    /// The driver is walking automated suites.
    Suites,
    /// Automated rows are done; manual rows await `record`.
    Manual,
    Sealed,
    Aborted,
}

impl State {
    pub fn live(self) -> bool {
        matches!(self, Self::Frozen | Self::Suites | Self::Manual)
    }
}

/// One row the gate owes, chosen at preflight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedRow {
    pub id: String,
    pub journey: String,
    /// The suite that walks it; `None` for a manual or unselected row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    /// No `spec`: a person walks it and `record`s the verdict.
    pub manual: bool,
    /// `release_always`, `impacted` or `no_base` (no CLEAN ancestor or merge base).
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub sha: String,
    pub env: String,
    pub state: State,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Merges into the integration branch are refused until this instant,
    /// or until seal or abort.
    pub freeze_until: DateTime<Utc>,
    /// The CLEAN ancestor (or merge base) scope and PR coverage are measured from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub plan: Vec<PlannedRow>,
    #[serde(default)]
    pub rows: Vec<Row>,
    #[serde(default)]
    pub suites: Vec<SuiteTiming>,
    pub served_start: Vec<ServedComponent>,
    pub flags: FlagSnapshot,
    pub resources: ResourcePlan,
    #[serde(default)]
    pub fixtures: Vec<FixtureReceipt>,
    /// Fixture teardown failures; debt only, never a verdict.
    #[serde(default)]
    pub orphan_debt: Vec<String>,
    /// The sealed gate, once sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    /// Pack generations sealed; a reseal of an OWED gate adds one.
    #[serde(default)]
    pub generation: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_reason: Option<String>,
    /// Best-effort publication and cleanup outcomes; never a verdict.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Run {
    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|row| row.id == id)
    }

    fn put(&mut self, row: Row) {
        self.rows.retain(|r| r.id != row.id);
        self.rows.push(row);
    }

    /// Planned rows with no verdict yet.
    pub fn pending(&self) -> impl Iterator<Item = &PlannedRow> {
        self.plan.iter().filter(|p| self.row(&p.id).is_none())
    }

    /// Whether merges into the integration branch are refused at `now`.
    pub fn frozen(&self, now: DateTime<Utc>) -> bool {
        self.state.live() && now < self.freeze_until
    }

    fn note(&mut self, text: String) {
        self.notes.push(format!("[{}] {text}", Utc::now().format("%Y-%m-%d %H:%M")));
    }
}

/// What a live run means for a merge into `target`.
#[derive(Debug, Clone, PartialEq)]
pub struct Freeze {
    pub sha: String,
    pub until: DateTime<Utc>,
}

impl Freeze {
    pub fn refusal(&self, target: &str) -> String {
        format!(
            "{FREEZE_LABEL}: the release gate for {} holds {target} until {}. Seal it, or abort it with `cas qa release-gate abort --sha {} --reason <why>`, then retry.",
            &self.sha[..12],
            self.until.format("%Y-%m-%d %H:%M UTC"),
            self.sha,
        )
    }
}

/// The live run's freeze when `target` is `qa.release.integration_branch`.
/// An unreadable run directory freezes nothing: the run file is the record.
pub fn active_freeze(
    cas_root: &Path,
    release: Option<&ReleaseConfig>,
    target: &str,
    now: DateTime<Utc>,
) -> Option<Freeze> {
    let release = release.filter(|r| !r.integration_branch.is_empty())?;
    if release.integration_branch != target {
        return None;
    }
    let run = active(cas_root).ok().flatten()?;
    run.frozen(now).then(|| Freeze { sha: run.sha, until: run.freeze_until })
}

fn dir(cas_root: &Path) -> PathBuf {
    cas_root.join(DIR)
}

fn full_sha(sha: &str) -> Result<&str> {
    ensure!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "--sha must be a full 40-character commit SHA"
    );
    Ok(sha)
}

fn run_path(cas_root: &Path, sha: &str) -> Result<PathBuf> {
    Ok(dir(cas_root).join(format!("{}.json", full_sha(sha)?.to_ascii_lowercase())))
}

fn lock_file(path: PathBuf) -> Result<File> {
    fs::create_dir_all(path.parent().context("lock path has no parent")?)?;
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path)?;
    fs2::FileExt::lock_exclusive(&file)?;
    Ok(file)
}

/// Serializes writers of one run across processes (driver, `record`, `abort`).
fn lock(cas_root: &Path, sha: &str) -> Result<File> {
    lock_file(run_path(cas_root, sha)?.with_extension("lock"))
}

pub fn load(cas_root: &Path, sha: &str) -> Result<Option<Run>> {
    match fs::read(run_path(cas_root, sha)?) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).context("release-gate run file")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn save(cas_root: &Path, run: &mut Run) -> Result<()> {
    run.updated_at = Utc::now();
    let path = run_path(cas_root, &run.sha)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_vec_pretty(run)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

/// Loads, changes and saves one run under its lock.
fn update<T>(cas_root: &Path, sha: &str, change: impl FnOnce(&mut Run) -> Result<T>) -> Result<T> {
    let _lock = lock(cas_root, sha)?;
    let mut run = load(cas_root, sha)?.context("no release-gate run for this SHA")?;
    let out = change(&mut run)?;
    save(cas_root, &mut run)?;
    Ok(out)
}

/// The project's live run, if any (at most one).
pub fn active(cas_root: &Path) -> Result<Option<Run>> {
    let Ok(entries) = fs::read_dir(dir(cas_root)) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(sha) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(Some(run)) = load(cas_root, sha)
            && run.state.live()
        {
            return Ok(Some(run));
        }
    }
    Ok(None)
}

/// The run `--sha` names, or the live run when it is omitted.
pub fn resolve(cas_root: &Path, sha: Option<&str>) -> Result<Run> {
    match sha {
        Some(sha) => load(cas_root, &resolve_sha(cas_root, sha)?)?
            .context("no release-gate run for this SHA; start one with cas qa release-gate --sha <sha>"),
        None => active(cas_root)?.context("no live release-gate run; pass --sha"),
    }
}

fn project_root(cas_root: &Path) -> Result<&Path> {
    cas_root.parent().context("Cassy store has no project directory")
}

fn resolve_sha(cas_root: &Path, sha: &str) -> Result<String> {
    let root = project_root(cas_root)?;
    super::resolve_commit(root, sha.trim())
        .with_context(|| format!("{} is not a local commit; fetch it", printable(sha)))
}

fn config(cas_root: &Path) -> Result<ReleaseConfig> {
    super::configured_checked(cas_root)
        .map_err(anyhow::Error::msg)?
        .context("No qa.release.adapter configured; set qa.release.adapter first")
}

/// The run's hand-walk debt: manual minutes per journey with each journey's
/// automation owner (an unreadable registry leaves every owner unknown).
pub fn hand_walk(cas_root: &Path, run: &Run) -> pack::HandWalk {
    let registry = registry(cas_root).ok().map(|(registry, _)| registry);
    pack::HandWalk::measure(&run.rows, registry.as_ref())
}

fn registry(cas_root: &Path) -> Result<(Registry, crate::config::QaConfig)> {
    let qa = crate::config::Config::load(cas_root)?.qa.unwrap_or_default();
    let path = project_root(cas_root)?.join(&qa.registry.path);
    let registry = crate::qa_registry::load(&path)
        .with_context(|| format!("release gate needs the journey registry at {}", path.display()))?;
    Ok((registry, qa))
}

fn live_in(journey: &Journey, env: &str) -> bool {
    journey.env_status.get(env).map_or(journey.status == "live", |s| s.status == "live")
}

fn git_ok(root: &Path, args: &[&str]) -> Option<String> {
    super::git(root, args)
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The newest sealed CLEAN pack whose SHA is a proper ancestor of `sha`.
fn last_clean(cas_root: &Path, root: &Path, sha: &str) -> Option<String> {
    let release = pack::pack_dir(cas_root, sha).ok()?.parent()?.to_path_buf();
    let mut best: Option<(DateTime<Utc>, String)> = None;
    for entry in fs::read_dir(release).ok()?.flatten() {
        let Ok(found) = Pack::load(&entry.path()) else { continue };
        if found.gate.verdict != GateVerdict::Clean || found.sha == sha {
            continue;
        }
        let ancestor = super::git(root, &["merge-base", "--is-ancestor", &found.sha, sha])
            .is_some_and(|o| o.status.success());
        if ancestor && best.as_ref().is_none_or(|(at, _)| found.created_at > *at) {
            best = Some((found.created_at, found.sha));
        }
    }
    best.map(|(_, sha)| sha)
}

/// The scope base: the last CLEAN ancestor, else the merge base with the
/// first promotion branch (local, then `origin/`).
fn scope_base(cas_root: &Path, root: &Path, config: &ReleaseConfig, sha: &str) -> Option<String> {
    last_clean(cas_root, root, sha).or_else(|| {
        let branch = config.branches.first()?;
        [branch.clone(), format!("origin/{branch}")]
            .iter()
            .find_map(|b| git_ok(root, &["merge-base", "--end-of-options", sha, b]))
            .filter(|base| base != sha)
    })
}

/// Q2: every staging-live `release_always` journey, plus the journeys whose
/// owners the range `base..sha` touches; with no base, every live journey.
fn plan(
    registry: &Registry,
    qa: &crate::config::QaConfig,
    config: &ReleaseConfig,
    root: &Path,
    base: Option<&str>,
    sha: &str,
) -> Result<Vec<PlannedRow>> {
    let impacted: Option<BTreeSet<String>> = match base {
        Some(base) => {
            let changes = crate::qa_impact::io::changes_for_diff(root, base, Some(sha))?;
            Some(
                crate::qa_impact::impact_for_changes(Some(registry), qa, &changes, "")
                    .journeys
                    .into_iter()
                    .map(|j| j.id)
                    .collect(),
            )
        }
        None => None,
    };
    let mut rows = Vec::new();
    for journey in registry.journeys.iter().filter(|j| live_in(j, &config.gate_env)) {
        let why = match &impacted {
            _ if journey.release_always => "release_always",
            Some(ids) if ids.contains(&journey.id) => "impacted",
            Some(_) => continue,
            None => "no_base",
        };
        let manual = journey.spec.as_deref().is_none_or(|s| s.trim().is_empty());
        let suite = (!manual)
            .then(|| config.suites.iter().find(|(_, s)| s.selects(journey)).map(|(n, _)| n.clone()))
            .flatten();
        rows.push(PlannedRow { id: journey.id.clone(), journey: journey.id.clone(), suite, manual, why: why.into() });
    }
    Ok(rows)
}

fn row(id: &str, journey: &str, verdict: RowVerdict) -> Row {
    Row {
        id: id.into(),
        journey: journey.into(),
        cell: None,
        run: None,
        verdict,
        frames: Vec::new(),
        readbacks: Vec::new(),
        manual_minutes: None,
        covers_prs: Vec::new(),
        covers_commits: Vec::new(),
    }
}

fn not_exercised(cause: NotExercisedCause, detail: impl Into<String>) -> RowVerdict {
    RowVerdict::NotExercised { cause, detail: detail.into() }
}

/// Starts the gate for `sha`, or returns its existing run unchanged with no
/// adapter call. Preflight runs here, before the freeze; a refusal persists
/// nothing, so rerunning retries it.
pub fn start(cas_root: &Path, sha: &str) -> Result<Run> {
    let config = config(cas_root)?;
    let root = project_root(cas_root)?;
    let sha = resolve_sha(cas_root, sha)?;
    let _start = lock_file(dir(cas_root).join("start.lock"))?;
    if let Some(existing) = load(cas_root, &sha)? {
        if existing.state != State::Aborted {
            return Ok(existing);
        }
        // An aborted run is kept beside the new one, never resumed.
        let path = run_path(cas_root, &sha)?;
        fs::rename(&path, path.with_extension(format!("aborted-{}", existing.updated_at.timestamp())))?;
    }
    if let Some(live) = active(cas_root)? {
        bail!(
            "the release gate for {} is {}; abort it first (cas qa release-gate abort --sha {} --reason <why>)",
            live.sha,
            serde_json::to_value(live.state)?.as_str().unwrap_or("live"),
            live.sha
        );
    }
    let (registry, qa) = registry(cas_root)?;
    let env = config.gate_env.clone();
    let served_start = preflight_served(root, &config, &env, &sha).map_err(anyhow::Error::msg)?;
    let base = scope_base(cas_root, root, &config, &sha);
    let plan = plan(&registry, &qa, &config, root, base.as_deref(), &sha)?;
    let journeys: Vec<Journey> = plan
        .iter()
        .filter_map(|p| registry.journeys.iter().find(|j| j.id == p.journey).cloned())
        .collect();
    let flags = super::flags::read(root, &config, &env, &sha, &journeys)
        .map_err(|e| anyhow::anyhow!("FLAGS UNKNOWN: {e}"))?;
    let uses: Vec<PlannedUse<'_>> =
        journeys.iter().map(|journey| PlannedUse { journey, runs: 1, cells: 1 }).collect();
    let resources = resources::preflight(root, cas_root, &config, &env, &sha, &registry.resources, &uses)
        .map_err(anyhow::Error::msg)?;
    let clocks: i64 = config
        .suites
        .iter()
        .filter(|(name, _)| plan.iter().any(|p| p.suite.as_ref() == Some(*name)))
        .map(|(_, suite)| i64::from(suite.wall_clock_mins))
        .sum();
    let now = Utc::now();
    let mut run = Run {
        sha: sha.clone(),
        env,
        state: State::Frozen,
        created_at: now,
        updated_at: now,
        freeze_until: now + ChronoDuration::minutes(clocks + FREEZE_SLACK_MINS),
        base,
        rows: plan
            .iter()
            .filter(|p| !p.manual && p.suite.is_none())
            .map(|p| {
                row(&p.id, &p.journey, not_exercised(NotExercisedCause::Automation, "no qa.release.suites entry selects it"))
            })
            .collect(),
        plan,
        suites: Vec::new(),
        served_start,
        flags,
        resources,
        fixtures: Vec::new(),
        orphan_debt: Vec::new(),
        gate: None,
        generation: 0,
        abort_reason: None,
        notes: Vec::new(),
    };
    run.note(format!("START {} rows; freeze until {}", run.plan.len(), run.freeze_until.to_rfc3339()));
    save(cas_root, &mut run)?;
    if let Err(error) = super::promotion::publish_freeze(cas_root, root, &config, true) {
        run = update(cas_root, &sha, |run| {
            run.note(format!("freeze status not published: {}", printable(&error)));
            Ok(run.clone())
        })?;
    }
    Ok(run)
}

/// Ends a live run without a pack: fixtures were torn down per suite, and
/// the freeze lifts.
pub fn abort(cas_root: &Path, sha: Option<&str>, reason: &str) -> Result<Run> {
    ensure!(!reason.trim().is_empty(), "abort needs a non-empty --reason");
    let sha = resolve(cas_root, sha)?.sha;
    let run = update(cas_root, &sha, |run| {
        ensure!(run.state.live(), "the release gate for {sha} is already {:?}", run.state);
        run.state = State::Aborted;
        run.abort_reason = Some(printable(reason));
        run.note(format!("ABORT: {}", printable(reason)));
        Ok(run.clone())
    })?;
    match configured(cas_root).map(|config| {
        super::promotion::publish_freeze(cas_root, project_root(cas_root)?, &config, false).map_err(anyhow::Error::msg)
    }) {
        Some(Err(error)) => update(cas_root, &sha, |run| {
            run.note(format!("freeze release not published: {error}"));
            Ok(run.clone())
        }),
        _ => Ok(run),
    }
}

/// A manual row: what a person walked and saw.
#[derive(Debug, Clone, Default)]
pub struct Record {
    pub row: String,
    /// `PASS`, `FAIL`, `NOT_A_ROUTE` or `NOT_EXERCISED`.
    pub verdict: String,
    pub reason: Option<String>,
    pub cause: Option<NotExercisedCause>,
    /// Files copied into the pack as the row's frames.
    pub evidence: Option<PathBuf>,
    pub minutes: Option<u32>,
    /// Read-back subject ids, `kind → id`.
    pub subjects: BTreeMap<String, String>,
    pub covers_prs: Vec<u64>,
    /// Direct-push commits this row covers.
    pub covers_commits: Vec<String>,
}

/// Records a manual row. On a live run it fills a planned manual row (or adds
/// a row that covers a PR). On a sealed OWED run it replaces an owed row
/// (BLOCKED_EXTERNAL, NOT_EXERCISED, SERVED_DRIFT…) and reseals as a new pack
/// generation. A FAILED or CLEAN gate is final; a fix needs a new SHA.
pub fn record(cas_root: &Path, sha: Option<&str>, input: &Record) -> Result<Run> {
    let config = config(cas_root)?;
    let root = project_root(cas_root)?;
    let (registry, _) = registry(cas_root)?;
    let sha = resolve(cas_root, sha)?.sha;
    let journey = {
        let run = load(cas_root, &sha)?.context("no release-gate run")?;
        let id = run.plan.iter().find(|p| p.id == input.row).map_or(input.row.as_str(), |p| p.journey.as_str());
        registry
            .journeys
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .with_context(|| format!("row {} names no registry journey", printable(&input.row)))?
    };
    let reseal = update(cas_root, &sha, |run| {
        let planned = run.plan.iter().find(|p| p.id == input.row);
        match run.state {
            State::Sealed => {
                let gate = run.gate.as_ref().map(|g| g.verdict);
                ensure!(
                    gate == Some(GateVerdict::Owed),
                    "the gate for {sha} is {gate:?}, which is final; a fix needs a new SHA"
                );
                if let Some(existing) = run.row(&input.row) {
                    ensure!(
                        !matches!(existing.verdict, RowVerdict::Pass | RowVerdict::NotARoute | RowVerdict::Fail { .. }),
                        "row {} is {}, which is not owed",
                        input.row,
                        existing.verdict.word()
                    );
                }
            }
            State::Aborted => bail!("the release gate for {sha} was aborted; start a new SHA"),
            _ => ensure!(
                planned.is_none_or(|p| p.manual) && run.row(&input.row).is_none_or(|_| planned.is_none()),
                "row {} is walked by the driver or already recorded",
                input.row
            ),
        }
        ensure!(
            planned.is_some() || !input.covers_prs.is_empty() || !input.covers_commits.is_empty(),
            "row {} is not planned; an extra manual row must name --covers-pr or --covers-commit",
            input.row
        );
        Ok(run.state == State::Sealed)
    })?;
    let walked = walk_manual(cas_root, root, &config, &sha, &journey, input)?;
    let mut run = update(cas_root, &sha, |run| {
        run.put(walked);
        if reseal {
            run.note(format!("RECORD {} after seal; resealing", input.row));
        }
        Ok(run.clone())
    })?;
    if reseal || (run.state == State::Manual && run.pending().next().is_none()) {
        run = seal(cas_root, &config, &sha)?;
    }
    Ok(run)
}

fn walk_manual(
    cas_root: &Path,
    root: &Path,
    config: &ReleaseConfig,
    sha: &str,
    journey: &Journey,
    input: &Record,
) -> Result<Row> {
    let dir = pack::pack_dir(cas_root, sha).map_err(anyhow::Error::msg)?;
    let frames = match &input.evidence {
        Some(evidence) => copy_evidence(evidence, &dir, &input.row)?,
        None => Vec::new(),
    };
    let reason = || input.reason.as_deref().map(printable).filter(|r| !r.trim().is_empty());
    let mut readbacks = Vec::new();
    let verdict = match input.verdict.to_ascii_uppercase().as_str() {
        "PASS" => {
            ensure!(!frames.is_empty(), "a PASS needs --evidence with at least one frame");
            let outcome = journey.readback.as_ref().map(|_| {
                super::readback(root, config, &config.gate_env, sha, journey, &input.subjects)
                    .unwrap_or_else(crate::qa_readback::Outcome::Missing)
            });
            readbacks.extend(readback_record(outcome.as_ref()));
            RowVerdict::grade(Ok(()), frames.len(), outcome)
        }
        "FAIL" => RowVerdict::Fail { reason: reason().context("a FAIL needs --reason")? },
        "NOT_A_ROUTE" => RowVerdict::NotARoute,
        "NOT_EXERCISED" => not_exercised(
            input.cause.unwrap_or(NotExercisedCause::OperatorSkip),
            reason().context("NOT_EXERCISED needs --reason")?,
        ),
        other => bail!("--verdict {} is not PASS, FAIL, NOT_A_ROUTE or NOT_EXERCISED", printable(other)),
    };
    let mut walked = row(&input.row, &journey.id, verdict);
    walked.frames = frames;
    walked.readbacks = readbacks;
    walked.manual_minutes = input.minutes;
    walked.covers_prs = input.covers_prs.clone();
    walked.covers_commits = input.covers_commits.clone();
    Ok(walked)
}

/// Copies regular files under `evidence` into `media/<row>/`; links are refused.
fn copy_evidence(evidence: &Path, pack: &Path, row: &str) -> Result<Vec<String>> {
    ensure!(
        !row.is_empty() && row.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
        "row id {} must be letters, digits, `_`, `.` or `-`",
        printable(row)
    );
    ensure!(fs::symlink_metadata(evidence).is_ok_and(|m| m.is_dir()), "--evidence must be a directory");
    let mut frames = Vec::new();
    for entry in walkdir::WalkDir::new(evidence).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        let relative = entry.path().strip_prefix(evidence)?;
        if entry.file_type().is_dir() {
            continue;
        }
        ensure!(entry.file_type().is_file(), "{} is not a regular file", entry.path().display());
        let target = Path::new(pack::MEDIA_DIR).join(row).join(relative);
        let name = target.to_str().context("evidence path is not UTF-8")?.replace('\\', "/");
        fs::create_dir_all(pack.join(&target).parent().context("media path")?)?;
        fs::copy(entry.path(), pack.join(&target))?;
        frames.push(name);
    }
    Ok(frames)
}

fn readback_record(outcome: Option<&crate::qa_readback::Outcome>) -> Option<crate::qa_readback::Readback> {
    use crate::qa_readback::Outcome;
    match outcome? {
        Outcome::Pass(readback) | Outcome::Fail { readback, .. } => Some(readback.clone()),
        _ => None,
    }
}

/// The `run` verb's answer.
#[derive(Debug, Deserialize)]
struct RunAnswer {
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    frames: Vec<String>,
    #[serde(default)]
    subjects: BTreeMap<String, String>,
}

/// Everything a row walk reads; shared by a suite's worker threads.
struct Walk<'a> {
    cas_root: &'a Path,
    root: &'a Path,
    config: &'a ReleaseConfig,
    registry: &'a Registry,
    sha: &'a str,
    env: &'a str,
    pack: PathBuf,
    served_start: &'a [ServedComponent],
    fixtures: &'a BTreeMap<String, Result<Map<String, Value>, String>>,
    deadline: Instant,
}

impl Walk<'_> {
    fn walk(&self, planned: &PlannedRow) -> Row {
        let mut walked = row(&planned.id, &planned.journey, RowVerdict::NotARoute);
        let Some(journey) = self.registry.journeys.iter().find(|j| j.id == planned.journey) else {
            walked.verdict = not_exercised(NotExercisedCause::Automation, "journey left the registry");
            return walked;
        };
        let mut outputs = Map::new();
        for name in &journey.fixtures {
            match self.fixtures.get(name) {
                Some(Ok(values)) => {
                    outputs.insert(name.clone(), Value::Object(values.clone()));
                }
                Some(Err(error)) => {
                    walked.verdict = not_exercised(NotExercisedCause::FixtureSetup, error.clone());
                    return walked;
                }
                None => {}
            }
        }
        let media = self.pack.join(pack::MEDIA_DIR).join(&planned.id);
        if let Err(error) = fs::create_dir_all(&media) {
            walked.verdict = not_exercised(NotExercisedCause::Automation, format!("media directory: {error}"));
            return walked;
        }
        let meter = Meter {
            cas_root: self.cas_root,
            env: self.env,
            resources: &self.registry.resources,
            deadline: self.deadline,
        };
        let request = Request {
            env: self.env,
            sha: self.sha,
            journey: Some(&journey.id),
            args: json!({
                "row": planned.id,
                "spec": journey.spec,
                "media_dir": media.to_string_lossy(),
                "fixtures": outputs,
            }),
        };
        let answer = super::resources::call_metered(self.root, self.config, &meter, journey, Verb::Run, request);
        walked.verdict = match answer {
            Ok(data) => self.grade(&mut walked, journey, data),
            Err(MeteredError::Blocked(blocked)) => RowVerdict::BlockedExternal(blocked),
            Err(MeteredError::Budget { resource }) => {
                not_exercised(NotExercisedCause::TimedOut, format!("no {resource} budget before the wall clock"))
            }
            Err(MeteredError::Refused(refusal)) => match refusal.blocked() {
                Some(blocked) => RowVerdict::BlockedExternal(blocked),
                None => not_exercised(NotExercisedCause::Automation, refusal.to_string()),
            },
            Err(MeteredError::Call(CallError::TimedOut(_))) => {
                not_exercised(NotExercisedCause::TimedOut, "the run call timed out")
            }
            Err(MeteredError::Call(error)) => not_exercised(NotExercisedCause::Automation, error.to_string()),
        };
        // Row end: an environment that changed under the row voids it.
        match preflight_served(self.root, self.config, self.env, self.sha) {
            Ok(now) => {
                let changed: Vec<String> = now
                    .iter()
                    .filter(|c| !self.served_start.contains(c))
                    .map(|c| printable(&c.component))
                    .collect();
                if !changed.is_empty() {
                    walked.verdict = RowVerdict::ServedDrift { components: changed };
                }
            }
            Err(ServedRefusal::Mismatch(problems)) => {
                walked.verdict = RowVerdict::ServedDrift { components: problems };
            }
            Err(ServedRefusal::Call(error)) => {
                walked.verdict =
                    not_exercised(NotExercisedCause::Automation, format!("served unknown at row end: {error}"));
            }
        }
        walked
    }

    fn grade(&self, walked: &mut Row, journey: &Journey, data: Value) -> RowVerdict {
        // The parse error is dropped: it could quote adapter data.
        let Ok(answer) = serde_json::from_value::<RunAnswer>(data) else {
            return not_exercised(NotExercisedCause::Automation, "run data is not {status, frames, …}");
        };
        let prefix = format!("{}/{}/", pack::MEDIA_DIR, walked.id);
        for frame in &answer.frames {
            if !frame.starts_with(&prefix) || pack::media_path(&self.pack, frame).is_err() {
                return not_exercised(
                    NotExercisedCause::Automation,
                    format!("frame {} is not a file under {prefix}", printable(frame)),
                );
            }
        }
        walked.frames = answer.frames;
        let runner = match answer.status.as_str() {
            "pass" => Ok(()),
            "fail" => Err(answer.reason.unwrap_or_else(|| "the runner failed".into())),
            "not_a_route" => return RowVerdict::NotARoute,
            other => {
                return not_exercised(NotExercisedCause::Automation, format!("run status {} is unknown", printable(other)));
            }
        };
        let readback = (runner.is_ok() && journey.readback.is_some()).then(|| {
            super::readback(self.root, self.config, self.env, self.sha, journey, &answer.subjects)
                .unwrap_or_else(crate::qa_readback::Outcome::Missing)
        });
        walked.readbacks.extend(readback_record(readback.as_ref()));
        RowVerdict::grade(runner, walked.frames.len(), readback)
    }
}

/// Why a suite stopped before its rows were all walked.
enum Stop {
    Done,
    Cancelled,
    Aborted,
}

/// Drives the live run one step: walks every unfinished suite, then moves to
/// `manual` or seals; at `freeze_until` the unrecorded rows time out and the
/// run seals. `cancel` is checked between rows; a row in flight finishes or
/// is left without a verdict and rerun on the next tick.
pub fn tick(cas_root: &Path, cancel: &AtomicBool) -> Result<()> {
    let Some(config) = super::configured_checked(cas_root).map_err(anyhow::Error::msg)? else {
        return Ok(());
    };
    let live = active(cas_root)?;
    // Idempotent: posts only new heads or a changed state, so a PR opened
    // between runs gets its release.
    let frozen = live.as_ref().is_some_and(|run| run.frozen(Utc::now()));
    if let Err(error) = super::promotion::publish_freeze(cas_root, project_root(cas_root)?, &config, frozen) {
        tracing::warn!(%error, "release-gate freeze status not published");
    }
    let Some(run) = live else {
        return Ok(());
    };
    let sha = run.sha.clone();
    let (registry, _) = registry(cas_root)?;
    // Suites run in name order.
    let suites: BTreeSet<String> = run.pending().filter_map(|p| p.suite.clone()).collect();
    for suite in suites {
        match walk_suite(cas_root, &config, &registry, &sha, &suite, cancel)? {
            Stop::Done => {}
            Stop::Cancelled | Stop::Aborted => return Ok(()),
        }
    }
    let run = load(cas_root, &sha)?.context("run vanished")?;
    let automated_done = run.pending().all(|p| p.manual);
    let expired = Utc::now() >= run.freeze_until;
    if run.state.live() && automated_done && (run.pending().next().is_none() || expired) {
        update(cas_root, &sha, |run| {
            let timed_out: Vec<PlannedRow> = run.pending().cloned().collect();
            for p in timed_out {
                run.put(row(&p.id, &p.journey, not_exercised(NotExercisedCause::TimedOut, "not recorded before the freeze ended")));
            }
            Ok(())
        })?;
        seal(cas_root, &config, &sha)?;
    } else if run.state.live() && automated_done && run.state != State::Manual {
        update(cas_root, &sha, |run| {
            run.state = State::Manual;
            Ok(())
        })?;
    }
    Ok(())
}

fn walk_suite(
    cas_root: &Path,
    config: &ReleaseConfig,
    registry: &Registry,
    sha: &str,
    suite: &str,
    cancel: &AtomicBool,
) -> Result<Stop> {
    let settings = config.suites.get(suite).cloned().unwrap_or_default();
    // The clock starts once and survives restarts.
    let started = update(cas_root, sha, |run| {
        ensure!(run.state.live(), "run is no longer live");
        run.state = State::Suites;
        if let Some(timing) = run.suites.iter().find(|t| t.name == suite) {
            return Ok(timing.started_at);
        }
        let now = Utc::now();
        run.suites.push(SuiteTiming { name: suite.into(), started_at: now, finished_at: None });
        Ok(now)
    })?;
    let clock = ChronoDuration::minutes(i64::from(settings.wall_clock_mins));
    let left = (started + clock - Utc::now()).to_std().unwrap_or(Duration::ZERO);
    let deadline = Instant::now() + left;
    let run = load(cas_root, sha)?.context("run vanished")?;
    let pending: Vec<PlannedRow> = run.pending().filter(|p| p.suite.as_deref() == Some(suite)).cloned().collect();
    let root = project_root(cas_root)?;
    let mut fixtures = BTreeMap::new();
    let mut setup = FixtureSuite::new(root, cas_root, config, &run.env, sha);
    let mut receipts = Vec::new();
    if !left.is_zero() {
        let names: BTreeSet<&String> = pending
            .iter()
            .filter_map(|p| registry.journeys.iter().find(|j| j.id == p.journey))
            .flat_map(|j| &j.fixtures)
            .collect();
        for name in names {
            let recipe = registry.fixtures.as_ref().and_then(|f| f.get(name)).and_then(|f| match &f.source {
                Some(FixtureSource::Recipe(recipe)) => Some(recipe),
                _ => None,
            });
            if let Some(recipe) = recipe {
                let outcome = setup.setup(recipe).map_err(|e| e.to_string());
                if let Ok(receipt) = &outcome {
                    receipts.push(receipt.clone());
                }
                fixtures.insert(name.clone(), outcome.map(|r| r.outputs));
            }
        }
    }
    let walk = Walk {
        cas_root,
        root,
        config,
        registry,
        sha,
        env: &run.env,
        pack: pack::pack_dir(cas_root, sha).map_err(anyhow::Error::msg)?,
        served_start: &run.served_start,
        fixtures: &fixtures,
        deadline,
    };
    let queue = Mutex::new(pending.iter().collect::<VecDeque<_>>());
    let stop = Mutex::new(Stop::Done);
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..settings.concurrency.max(1) {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    *stop.lock().unwrap() = Stop::Cancelled;
                    return;
                }
                if !matches!(*stop.lock().unwrap(), Stop::Done) || failure.lock().unwrap().is_some() {
                    return;
                }
                let Some(planned) = queue.lock().unwrap().pop_front() else { return };
                let walked = if Instant::now() >= deadline {
                    row(&planned.id, &planned.journey, not_exercised(NotExercisedCause::TimedOut, format!("suite {suite} wall clock ended")))
                } else {
                    walk.walk(planned)
                };
                let saved = update(cas_root, sha, |run| {
                    if !run.state.live() {
                        return Ok(false);
                    }
                    run.put(walked);
                    Ok(true)
                });
                match saved {
                    Ok(true) => {}
                    Ok(false) => {
                        *stop.lock().unwrap() = Stop::Aborted;
                        return;
                    }
                    Err(error) => {
                        *failure.lock().unwrap() = Some(error);
                        return;
                    }
                }
            });
        }
    });
    let report = setup.finish();
    let stop = stop.into_inner().unwrap();
    let finished = matches!(stop, Stop::Done);
    update(cas_root, sha, |run| {
        run.fixtures.extend(receipts);
        run.orphan_debt.extend(report.orphan_debt.iter().map(|d| format!("{}: {}", d.cleanup_id, printable(&d.error))));
        if finished && let Some(timing) = run.suites.iter_mut().find(|t| t.name == suite) {
            timing.finished_at = Some(Utc::now());
        }
        Ok(())
    })?;
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error);
    }
    Ok(stop)
}

/// Seals the run's pack. A previous generation's `pack.json` is kept as
/// `history/pack-<n>.json` and the new one replaces it, so every reader
/// (batch receipt, promotion) sees the latest. Publication and pruning are
/// best effort and recorded in the run's notes.
fn seal(cas_root: &Path, config: &ReleaseConfig, sha: &str) -> Result<Run> {
    let root = project_root(cas_root)?;
    let (registry, qa) = registry(cas_root)?;
    let dir = pack::pack_dir(cas_root, sha).map_err(anyhow::Error::msg)?;
    let project = dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map_or_else(|| "project".into(), |n| n.to_string_lossy().into_owned());
    let mut run = update(cas_root, sha, |run| {
        let mut pack = Pack::new(&project, sha, run.served_start.clone());
        pack.served.end = preflight_served(root, config, &run.env, sha).ok();
        pack.flags = Some(run.flags.clone());
        pack.rows = run
            .plan
            .iter()
            .filter_map(|p| run.row(&p.id))
            .chain(run.rows.iter().filter(|r| !run.plan.iter().any(|p| p.id == r.id)))
            .cloned()
            .collect();
        // A frame retention already pruned keeps its earlier digest.
        let previous = Pack::load(&dir).ok();
        for frame in pack.rows.iter().flat_map(|r| r.frames.clone()).collect::<Vec<_>>() {
            if let Err(error) = pack.add_media(&dir, &frame) {
                let kept = previous.as_ref().and_then(|p| p.media.iter().find(|m| m.path == frame));
                pack.media.push(kept.cloned().ok_or_else(|| anyhow::Error::msg(error))?);
            }
        }
        if let Some(base) = &run.base {
            let prs = coverage::candidate_prs(root, base, sha)?;
            pack.coverage = coverage::coverage(&prs, Some(&registry), &qa, &pack.rows);
        }
        pack.fixtures = run.fixtures.clone();
        pack.resources = Some(run.resources.clone());
        pack.suites = run.suites.clone();
        pack.hand_walk = Some(pack::HandWalk::measure(&pack.rows, Some(&registry))).filter(|h| h.rows > 0);
        let current = dir.join(pack::PACK_FILE);
        if current.exists() {
            let history = dir.join("history");
            fs::create_dir_all(&history)?;
            fs::copy(&current, history.join(format!("pack-{}.json", run.generation)))?;
        }
        pack.seal(&dir).map_err(anyhow::Error::msg)?;
        run.generation += 1;
        run.state = State::Sealed;
        run.gate = Some(pack.gate.clone());
        run.note(format!("SEAL generation {}: {:?}", run.generation, pack.gate.verdict));
        Ok(run.clone())
    })?;
    let mut notes = Vec::new();
    if let Some(release) = dir.parent() {
        let pruned = super::media::prune_media(release, config.retention_days, Utc::now());
        if !pruned.is_empty() {
            notes.push(format!("pruned {} media files past {} days", pruned.len(), config.retention_days));
        }
    }
    match Pack::load(&dir) {
        Ok(sealed) => notes.extend(publish_comments(root, config, &sealed)),
        Err(error) => notes.push(format!("pack reload for publication failed: {error}")),
    }
    notes.extend(super::slack::post_sealed(config, &dir));
    if let Err(error) = super::promotion::publish_freeze(cas_root, root, config, false) {
        notes.push(format!("freeze release not published: {}", printable(&error)));
    }
    if let Err(error) = super::promotion::publish_promotions(cas_root, root, config) {
        notes.push(format!("promotion status not published: {}", printable(&error)));
    }
    if !notes.is_empty() {
        run = update(cas_root, sha, |run| {
            for note in notes {
                run.note(note);
            }
            Ok(run.clone())
        })?;
    }
    Ok(run)
}

/// Edits the gate comment on each open PR from the integration branch into a
/// promotion branch whose head is the walked SHA.
fn publish_comments(root: &Path, config: &ReleaseConfig, sealed: &Pack) -> Vec<String> {
    if config.integration_branch.is_empty() || config.branches.is_empty() {
        return Vec::new();
    }
    let gh = |args: &[&str]| -> Option<String> {
        let program = std::env::var_os(crate::qa_pass::github_gate::GH_BIN_ENV)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "gh".into());
        let mut command = std::process::Command::new(program);
        command.current_dir(root).args(args);
        let cap = Duration::from_secs(20);
        crate::bounded_process::run_command(&mut command, crate::bounded_process::Deadline::after(cap), cap)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let Some(repo) = gh(&["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]) else {
        return vec!["PR comment skipped: gh could not name the repository".into()];
    };
    let mut notes = Vec::new();
    for branch in &config.branches {
        let query = format!(".[] | select(.headRefOid == \"{}\") | .number", sealed.sha);
        let Some(numbers) = gh(&[
            "pr", "list", "--state", "open", "--base", branch, "--head", &config.integration_branch,
            "--json", "number,headRefOid", "--jq", &query,
        ]) else {
            notes.push(format!("PR comment skipped: gh pr list into {branch} failed"));
            continue;
        };
        for pr in numbers.lines().filter_map(|n| n.trim().parse::<u64>().ok()) {
            match super::comment::publish_comment(&super::comment::GhComments, &repo, pr, sealed) {
                Ok(_) => notes.push(format!("PR #{pr} comment updated")),
                Err(error) => notes.push(format!("PR #{pr} comment failed: {}", printable(&error))),
            }
        }
    }
    notes
}

/// Starts the background driver when this `cas serve` wins the project's
/// lease; contenders and unconfigured projects spawn nothing. Dropping the
/// task (runtime shutdown) raises the cancel flag, so a suite stops between rows.
pub(crate) fn spawn_driver(cas_root: PathBuf) {
    if super::configured(&cas_root).is_none() {
        return;
    }
    let lease = match lock_lease(&cas_root) {
        Ok(Some(lease)) => lease,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "could not acquire the release-gate driver lease");
            return;
        }
    };
    struct CancelOnDrop(std::sync::Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    tokio::spawn(async move {
        let _lease = lease;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(cancel.clone());
        // A crash left fixture handles: replay their teardown once.
        let root = cas_root.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let config = super::configured(&root)?;
            super::fixtures::sweep(root.parent()?, &root, &config).ok()
        })
        .await;
        loop {
            let root = cas_root.clone();
            let flag = cancel.clone();
            let result = tokio::task::spawn_blocking(move || {
                if super::configured(&root).is_none() {
                    return Ok(false);
                }
                tick(&root, &flag).map(|()| true)
            })
            .await
            .unwrap_or_else(|error| Err(error.into()));
            match result {
                Ok(false) => return,
                Err(error) => tracing::warn!(%error, "release-gate driver tick failed"),
                Ok(true) => {}
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

fn lock_lease(cas_root: &Path) -> Result<Option<File>> {
    fs::create_dir_all(dir(cas_root))?;
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir(cas_root).join("driver.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&lease) {
        Ok(()) => Ok(Some(lease)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}
