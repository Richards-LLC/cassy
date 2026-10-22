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
    preflight_served, printable, short,
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
/// The detail of a row the freeze TTL timed out.
const TTL_DETAIL: &str = "not walked before the freeze ended";
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
    /// The golden-set cell this row walks (finding 18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<super::matrix::Cell>,
    /// Which of the cell's runs this row is, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u32>,
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
    /// Sealed at `freeze_until` with rows still unwalked, not by its last row.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ttl_sealed: bool,
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
            short(&self.sha),
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

/// Reads the run for `sha`. A run whose recorded SHA is not the 40-hex SHA
/// its file is named for is refused, so no caller slices a malformed SHA.
pub fn load(cas_root: &Path, sha: &str) -> Result<Option<Run>> {
    let path = run_path(cas_root, sha)?;
    let run: Run = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("release-gate run file")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        full_sha(&run.sha).is_ok() && run.sha.eq_ignore_ascii_case(sha),
        "release-gate run file {} records sha {:?}, not its own",
        path.display(),
        printable(&run.sha)
    );
    Ok(Some(run))
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

/// Every `<40-hex>.json` run file: the runs that load and, by path, the
/// errors of those that do not.
fn scan(cas_root: &Path) -> (Vec<Run>, Vec<(PathBuf, String)>) {
    let (mut runs, mut bad) = (Vec::new(), Vec::new());
    let Ok(entries) = fs::read_dir(dir(cas_root)) else {
        return (runs, bad);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        // Other JSON here (promotion's freeze-status.json) is not a run.
        let Some(sha) = path.file_stem().and_then(|s| s.to_str()).filter(|s| full_sha(s).is_ok()) else {
            continue;
        };
        match load(cas_root, sha) {
            Ok(Some(run)) => runs.push(run),
            Ok(None) => {}
            Err(error) => bad.push((path, format!("{error:#}"))),
        }
    }
    (runs, bad)
}

/// Run files that cannot be read, for `status`. The freeze stays fail-open
/// on them by design: row-end SERVED_DRIFT still guards the verdicts.
pub fn unreadable(cas_root: &Path) -> Vec<(PathBuf, String)> {
    scan(cas_root).1
}

/// The project's live run, if any (at most one). An unreadable run file is
/// warned about once per process.
pub fn active(cas_root: &Path) -> Result<Option<Run>> {
    static WARNED: std::sync::OnceLock<Mutex<BTreeSet<PathBuf>>> = std::sync::OnceLock::new();
    let (runs, bad) = scan(cas_root);
    for (path, error) in bad {
        let warned = WARNED.get_or_init(Default::default);
        if warned.lock().unwrap_or_else(|p| p.into_inner()).insert(path.clone()) {
            tracing::warn!(path = %path.display(), %error, "unreadable release-gate run file skipped");
        }
    }
    Ok(runs.into_iter().find(|run| run.state.live()))
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

/// What `--json` prints: the run's fields at top level plus `hand_walk`,
/// the debt summary the text status shows.
pub fn status_json(cas_root: &Path, run: &Run) -> Result<Value> {
    let mut value = serde_json::to_value(run)?;
    value["hand_walk"] = serde_json::to_value(hand_walk(cas_root, run))?;
    Ok(value)
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
    // Two journeys sharing an id would share every row id; --check reports
    // it, and the gate refuses it.
    let mut ids = BTreeSet::new();
    if let Some(twice) = registry.journeys.iter().find(|j| !ids.insert(j.id.as_str())) {
        bail!(
            "journey id {} is declared twice in the registry; run cas qa journeys --check and rename one",
            printable(&twice.id)
        );
    }
    let mut rows = Vec::new();
    for journey in registry.journeys.iter().filter(|j| j.live_in(&config.gate_env)) {
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
        if journey.is_golden_set() {
            let expansion = super::matrix::expand(root, journey, config.max_cells).map_err(anyhow::Error::msg)?;
            for cell in &expansion.cells {
                for run in 1..=expansion.runs {
                    rows.push(PlannedRow {
                        id: super::matrix::row_id(&journey.id, cell, run),
                        journey: journey.id.clone(),
                        suite: suite.clone(),
                        manual,
                        why: why.into(),
                        cell: Some(cell.clone()),
                        run: Some(run),
                    });
                }
            }
            continue;
        }
        rows.push(PlannedRow {
            id: journey.id.clone(),
            journey: journey.id.clone(),
            suite,
            manual,
            why: why.into(),
            cell: None,
            run: None,
        });
    }
    // One walk fills one row id: two journeys planning the same id would let
    // a walk of one stand in for the other, so refuse before the freeze.
    let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
    for row in &rows {
        if let Some(other) = owners.insert(&row.id, &row.journey).filter(|other| *other != row.journey) {
            bail!(
                "row id {} is planned by journeys {} and {}; rename one before starting the gate",
                printable(&row.id),
                printable(other),
                printable(&row.journey)
            );
        }
    }
    if let Some(bad) = rows.iter().find(|r| valid_row_id(&r.id).is_err()) {
        bail!("{}", valid_row_id(&bad.id).unwrap_err());
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
        identity: None,
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
    let mut journeys: Vec<Journey> = Vec::new();
    for planned in &plan {
        if !journeys.iter().any(|j| j.id == planned.journey)
            && let Some(journey) = registry.journeys.iter().find(|j| j.id == planned.journey)
        {
            journeys.push(journey.clone());
        }
    }
    let flags = super::flags::read(root, &config, &env, &sha, &journeys)
        .map_err(|e| anyhow::anyhow!("FLAGS UNKNOWN: {e}"))?;
    // A golden set needs consumes × runs × cells (finding 18).
    let uses: Vec<PlannedUse<'_>> = journeys
        .iter()
        .map(|journey| {
            let rows = plan.iter().filter(|p| p.journey == journey.id);
            let runs = rows.clone().filter_map(|p| p.run).max().unwrap_or(1);
            let cells = (rows.count() as u32 / runs).max(1);
            PlannedUse { journey, runs, cells }
        })
        .collect();
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
        ttl_sealed: false,
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

/// The run `record` targets. Without `--sha` it is the live run; when the
/// TTL has just sealed that run, say so, because a retry with its SHA reseals.
fn record_target(cas_root: &Path, sha: Option<&str>) -> Result<String> {
    if sha.is_some() || active(cas_root)?.is_some() {
        return Ok(resolve(cas_root, sha)?.sha);
    }
    let latest = scan(cas_root).0.into_iter().max_by_key(|run| run.updated_at);
    match latest {
        Some(run) if run.state == State::Sealed && run.ttl_sealed => bail!(
            "the run for {} was sealed by the TTL; retry the record with --sha {} to reseal",
            short(&run.sha),
            run.sha
        ),
        _ => Ok(resolve(cas_root, None)?.sha),
    }
}

impl Record {
    /// The verdict `--verdict` and `--reason` describe; validated before any
    /// evidence is copied. A PASS is graded later, from its frames.
    fn parse_verdict(&self) -> Result<RowVerdict> {
        let reason = || self.reason.as_deref().map(printable).filter(|r| !r.trim().is_empty());
        Ok(match self.verdict.to_ascii_uppercase().as_str() {
            "PASS" => RowVerdict::Pass,
            "FAIL" => RowVerdict::Fail { reason: reason().context("a FAIL needs --reason")? },
            "NOT_A_ROUTE" => RowVerdict::NotARoute,
            "NOT_EXERCISED" => not_exercised(
                self.cause.unwrap_or(NotExercisedCause::OperatorSkip),
                reason().context("NOT_EXERCISED needs --reason")?,
            ),
            other => bail!("--verdict {} is not PASS, FAIL, NOT_A_ROUTE or NOT_EXERCISED", printable(other)),
        })
    }
}

/// Records a manual row. On a live run it fills a planned manual row (or adds
/// a row that covers a PR). On a sealed OWED run it replaces an owed row
/// (BLOCKED_EXTERNAL, NOT_EXERCISED, SERVED_DRIFT…) and reseals as a new pack
/// generation. A FAILED or CLEAN gate is final; a fix needs a new SHA.
pub fn record(cas_root: &Path, sha: Option<&str>, input: &Record) -> Result<Run> {
    let config = config(cas_root)?;
    let root = project_root(cas_root)?;
    let (registry, _) = registry(cas_root)?;
    valid_row_id(&input.row)?;
    let verdict = input.parse_verdict()?;
    let sha = record_target(cas_root, sha)?;
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
    // Whether `input` may be written to `run`, and whether that reseals.
    // Checked before the walk and again in the write step: a TTL seal may
    // land in between, and its timed-out row is then owed and reseals.
    let admit = |run: &Run| -> Result<bool> {
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
            _ => {
                // The TTL tick timed the row out but has not sealed yet.
                let ttl = run.row(&input.row).is_some_and(|r| {
                    matches!(&r.verdict, RowVerdict::NotExercised { cause: NotExercisedCause::TimedOut, detail } if detail == TTL_DETAIL)
                });
                ensure!(!ttl, "the TTL timed row {} out; retry after the run seals to reseal it", input.row);
                ensure!(
                    planned.is_none_or(|p| p.manual) && run.row(&input.row).is_none_or(|_| planned.is_none()),
                    "row {} is walked by the driver or already recorded",
                    input.row
                );
            }
        }
        ensure!(
            planned.is_some() || !input.covers_prs.is_empty() || !input.covers_commits.is_empty(),
            "row {} is not planned; an extra manual row must name --covers-pr or --covers-commit",
            input.row
        );
        Ok(run.state == State::Sealed)
    };
    admit(&load(cas_root, &sha)?.context("no release-gate run")?)?;
    // Evidence is staged outside media/ and moved into media/<row>/ only
    // under the run lock, after the write-step admit: a refused or racing
    // record never touches a recorded row's files.
    let dir = pack::pack_dir(cas_root, &sha).map_err(anyhow::Error::msg)?;
    let staged = Staged::new(&dir, input.evidence.as_deref())?;
    let walked = walk_manual(root, &config, &sha, &journey, input, verdict, &staged)?;
    #[cfg(test)]
    tests_hook::before_write();
    let mut reseal = false;
    let written = update(cas_root, &sha, |run| {
        reseal = admit(run)?;
        staged.commit(&dir, &input.row)?;
        run.put(walked);
        if reseal {
            run.note(format!("RECORD {} after seal; resealing", input.row));
        }
        Ok(run.clone())
    });
    let mut run = written?;
    if reseal || (run.state == State::Manual && run.pending().next().is_none()) {
        run = seal(cas_root, &config, &sha)?;
    }
    Ok(run)
}

fn walk_manual(
    root: &Path,
    config: &ReleaseConfig,
    sha: &str,
    journey: &Journey,
    input: &Record,
    verdict: RowVerdict,
    staged: &Staged,
) -> Result<Row> {
    let frames = staged.frames(&input.row);
    let mut readbacks = Vec::new();
    let verdict = match verdict {
        RowVerdict::Pass => {
            ensure!(!frames.is_empty(), "a PASS needs --evidence with at least one frame");
            let outcome = journey.readback.as_ref().map(|_| {
                super::readback(root, config, &config.gate_env, sha, journey, &input.subjects)
                    .unwrap_or_else(crate::qa_readback::Outcome::Missing)
            });
            readbacks.extend(readback_record(outcome.as_ref()));
            RowVerdict::grade(Ok(()), frames.len(), outcome)
        }
        other => other,
    };
    let mut walked = row(&input.row, &journey.id, verdict);
    walked.frames = frames;
    walked.readbacks = readbacks;
    walked.manual_minutes = input.minutes;
    walked.identity = identity(journey, &input.subjects);
    walked.covers_prs = input.covers_prs.clone();
    walked.covers_commits = input.covers_commits.clone();
    Ok(walked)
}

/// A row id names one directory, `media/<row>/`: it starts with a letter or
/// digit (so never `.`, `..` or a dot directory such as the `.slack`
/// transcode dir) and continues with letters, digits, `_`, `.` or `-`.
pub(crate) fn valid_row_id(row: &str) -> Result<()> {
    ensure!(
        row.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
            && row.len() <= 200
            && row.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
        "row id {} must start with a letter or digit and continue with letters, digits, `_`, `.` or `-`",
        printable(row)
    );
    Ok(())
}

/// `parent/name` as a real directory: an existing entry must be a directory
/// and not a link; a missing one is created. Never creates through a link.
fn real_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) => ensure!(meta.file_type().is_dir(), "{} is not a real directory under the pack", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&path)?,
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

/// Evidence copied into a private staging directory beside the pack's
/// `media/`, moved into `media/<row>/` by [`Staged::commit`]. Dropping it
/// removes whatever was not committed.
struct Staged {
    dir: Option<PathBuf>,
    /// Plain relative paths of the staged regular files, sorted.
    files: Vec<PathBuf>,
}

impl Staged {
    fn new(pack: &Path, evidence: Option<&Path>) -> Result<Self> {
        let Some(evidence) = evidence else {
            return Ok(Self { dir: None, files: Vec::new() });
        };
        ensure!(fs::symlink_metadata(evidence).is_ok_and(|m| m.is_dir()), "--evidence must be a directory");
        fs::create_dir_all(pack)?;
        let root = pack.join(format!(".staged-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root)?;
        let mut staged = Self { dir: Some(root.clone()), files: Vec::new() };
        // The row's whole new media directory, swapped in by `commit`.
        let dir = root.join("row");
        fs::create_dir(&dir)?;
        for entry in walkdir::WalkDir::new(evidence).follow_links(false).sort_by_file_name() {
            let entry = entry?;
            if entry.file_type().is_dir() {
                continue;
            }
            ensure!(entry.file_type().is_file(), "{} is not a regular file", entry.path().display());
            let relative = entry.path().strip_prefix(evidence)?.to_path_buf();
            ensure!(
                relative.to_str().is_some() && relative.components().all(|c| matches!(c, std::path::Component::Normal(_))),
                "evidence path {} is not a plain UTF-8 relative path",
                relative.display()
            );
            fs::create_dir_all(dir.join(&relative).parent().context("staged path")?)?;
            fs::copy(entry.path(), dir.join(&relative))?;
            staged.files.push(relative);
        }
        Ok(staged)
    }

    /// The pack-relative frame names these files take under `media/<row>/`.
    fn frames(&self, row: &str) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|f| f.to_str())
            .map(|f| format!("{}/{row}/{}", pack::MEDIA_DIR, f.replace('\\', "/")))
            .collect()
    }

    /// Replaces `media/<row>/` with the staged directory, all or nothing:
    /// the old directory moves aside, the staged one moves in by a single
    /// rename, and any failure puts the old one back. No directory is ever
    /// created or entered through a link.
    fn commit(&self, pack: &Path, row: &str) -> Result<()> {
        let Some(root) = &self.dir else { return Ok(()) };
        valid_row_id(row)?;
        ensure!(
            fs::symlink_metadata(pack).is_ok_and(|m| m.file_type().is_dir()),
            "{} is not a real directory",
            pack.display()
        );
        let media = real_dir(pack, pack::MEDIA_DIR)?;
        let target = media.join(row);
        let old = root.join("old");
        let had_old = match fs::symlink_metadata(&target) {
            Ok(meta) => {
                ensure!(meta.file_type().is_dir(), "{} is not a real directory under the pack", target.display());
                fs::rename(&target, &old)?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let restore = |error: anyhow::Error| -> anyhow::Error {
            if fs::symlink_metadata(&target).is_ok() {
                let _ = fs::rename(&target, root.join("refused"));
            }
            if had_old {
                let _ = fs::rename(&old, &target);
            }
            error
        };
        fs::rename(root.join("row"), &target).map_err(|e| restore(e.into()))?;
        for frame in self.frames(row) {
            pack::media_path(pack, &frame).map_err(|e| restore(anyhow::Error::msg(e)))?;
        }
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests_hook {
    use std::cell::RefCell;
    thread_local! {
        static BEFORE_WRITE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }
    /// Runs `hook` once between `record`'s first admit and its write step.
    pub(crate) fn set_before_write(hook: impl FnOnce() + 'static) {
        BEFORE_WRITE.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }
    pub(super) fn before_write() {
        if let Some(hook) = BEFORE_WRITE.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
    }
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
    /// Golden-set measurements by criterion, graded against the cell's labels.
    #[serde(default)]
    measured: BTreeMap<String, f64>,
}

/// The run's `independence_key` value, when the journey names a key and the
/// run reported it.
fn identity(journey: &Journey, subjects: &BTreeMap<String, String>) -> Option<String> {
    let key = journey.independence_key.as_ref()?;
    subjects.get(key).filter(|id| !id.is_empty()).map(|id| printable(id))
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
                "cell": planned.cell,
                "run": planned.run,
            }),
        };
        let answer = super::resources::call_metered(self.root, self.config, &meter, journey, Verb::Run, request);
        walked.verdict = match answer {
            Ok(data) => self.grade(&mut walked, journey, planned, data),
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

    fn grade(&self, walked: &mut Row, journey: &Journey, planned: &PlannedRow, data: Value) -> RowVerdict {
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
        walked.identity = identity(journey, &answer.subjects);
        let verdict = RowVerdict::grade(runner, walked.frames.len(), readback);
        match (&verdict, &planned.cell) {
            (RowVerdict::Pass, Some(cell)) => {
                super::matrix::grade_measured(self.registry, journey, cell, &answer.measured).unwrap_or(verdict)
            }
            _ => verdict,
        }
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
    // Suites run in name order; none starts once the freeze has ended.
    let suites: BTreeSet<String> = run.pending().filter_map(|p| p.suite.clone()).collect();
    for suite in suites.into_iter().filter(|_| Utc::now() < run.freeze_until) {
        match walk_suite(cas_root, &config, &registry, &sha, &suite, cancel)? {
            Stop::Done => {}
            Stop::Cancelled | Stop::Aborted => return Ok(()),
        }
    }
    let run = load(cas_root, &sha)?.context("run vanished")?;
    let automated_done = run.pending().all(|p| p.manual);
    let expired = Utc::now() >= run.freeze_until;
    // At the TTL every row still without a verdict, automated or manual, is
    // timed out: merges are allowed again, so nothing more is walked.
    if run.state.live() && (expired || (automated_done && run.pending().next().is_none())) {
        update(cas_root, &sha, |run| {
            let timed_out: Vec<PlannedRow> = run.pending().cloned().collect();
            run.ttl_sealed = !timed_out.is_empty();
            for p in timed_out {
                run.put(row(&p.id, &p.journey, not_exercised(NotExercisedCause::TimedOut, TTL_DETAIL)));
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
    let run = load(cas_root, sha)?.context("run vanished")?;
    let clock = ChronoDuration::minutes(i64::from(settings.wall_clock_mins));
    let end = (started + clock).min(run.freeze_until);
    let left = (end - Utc::now()).to_std().unwrap_or(Duration::ZERO);
    let deadline = Instant::now() + left;
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
        // Golden sets: every planned row names its cell and run, a duplicate
        // run is rejected, and coverage counts cells × runs (finding 18).
        for row in &mut pack.rows {
            if let Some(planned) = run.plan.iter().find(|p| p.id == row.id && p.cell.is_some()) {
                row.cell = planned.cell.as_ref().map(super::matrix::Cell::label);
                row.run = planned.run;
            }
        }
        let golden: Vec<super::matrix::Planned<'_>> = registry
            .journeys
            .iter()
            .filter_map(|journey| {
                let rows: Vec<(String, super::matrix::Cell)> = run
                    .plan
                    .iter()
                    .filter(|p| p.journey == journey.id)
                    .filter_map(|p| Some((p.id.clone(), p.cell.clone()?)))
                    .collect();
                let labeled = registry.labeled_criteria(journey);
                (!rows.is_empty()).then_some(super::matrix::Planned { journey, rows, labeled })
            })
            .collect();
        pack.matrix = super::matrix::settle(&mut pack.rows, &golden);
        for row in &pack.rows {
            if run.row(&row.id).is_some_and(|r| r.verdict != row.verdict) {
                run.put(row.clone());
            }
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
        // A reseal is a record's, not the TTL's: no retry hint applies now.
        if run.generation > 1 {
            run.ttl_sealed = false;
        }
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
    notes.extend(super::slack::seal_note(config));
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
    struct CancelOnDrop(std::sync::Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    tokio::spawn(async move {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(cancel.clone());
        let lease = std::sync::Arc::new(Mutex::new(None));
        loop {
            let root = cas_root.clone();
            let flag = cancel.clone();
            let held = lease.clone();
            let result = tokio::task::spawn_blocking(move || {
                if super::configured(&root).is_none() {
                    return Ok(false);
                }
                drive(&root, &mut held.lock().unwrap_or_else(|p| p.into_inner()), &flag).map(|_| true)
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

/// One driver interval: takes the project's driver lease if no server holds
/// it (so another `cas serve` takes over when the holder exits), then ticks.
/// On acquiring it, replays expired fixture cleanups once. Returns whether
/// this process holds the lease and ticked.
pub(crate) fn drive(cas_root: &Path, lease: &mut Option<File>, cancel: &AtomicBool) -> Result<bool> {
    if lease.is_none() {
        let Some(acquired) = lock_lease(cas_root)? else {
            return Ok(false);
        };
        *lease = Some(acquired);
        // A crashed holder left fixture handles: replay their teardown.
        if let (Some(config), Some(project)) = (super::configured(cas_root), cas_root.parent())
            && let Err(error) = super::fixtures::sweep(project, cas_root, &config)
        {
            tracing::warn!(%error, "release-gate fixture sweep failed");
        }
    }
    tick(cas_root, cancel)?;
    post_sealed_runs(cas_root);
    Ok(true)
}

/// Posts every sealed generation missing from its pack's `slack.json`
/// (cas-efb8): seals made by `record` in the CLI, which has no proxy, are
/// posted here, and a failed post retries on the next tick.
fn post_sealed_runs(cas_root: &Path) {
    let Some(config) = super::configured(cas_root).filter(|c| !c.slack_channel.trim().is_empty()) else {
        return;
    };
    for run in scan(cas_root).0.into_iter().filter(|run| run.state == State::Sealed) {
        let Ok(dir) = pack::pack_dir(cas_root, &run.sha) else {
            continue;
        };
        let notes = super::slack::post_sealed(&config, &dir);
        if !notes.is_empty()
            && let Err(error) = update(cas_root, &run.sha, |run| {
                notes.into_iter().for_each(|note| run.note(note));
                Ok(())
            })
        {
            tracing::warn!(%error, "release-gate Slack note not saved");
        }
    }
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
