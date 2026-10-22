//! Journey registry v1: YAML loading, validation and shared path-filter semantics.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

pub const DEFAULT_PATH: &str = "docs/qa/journeys.yaml";

// Serde's derived flatten buffers extras as Content, which cannot represent
// YAML tags. Read known fields with their normal types and extras directly as
// YAML Values, as qa_readback::Declaration does. Keep defaults and duplicate
// known-field errors without repeating the visitor for every registry record.
macro_rules! deserialize_with_extra {
    ($record:ident { $($field:ident),* $(,)? }) => {
        impl<'de> Deserialize<'de> for $record {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct RecordVisitor;

                impl<'de> serde::de::Visitor<'de> for RecordVisitor {
                    type Value = $record;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!(stringify!($record), " mapping"))
                    }

                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                        let mut record = $record::default();
                        let mut seen = std::collections::BTreeSet::new();
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($field) => {
                                    if !seen.insert(stringify!($field)) {
                                        return Err(serde::de::Error::duplicate_field(stringify!($field)));
                                    }
                                    record.$field = map.next_value()?;
                                },)*
                                _ => { record.extra.insert(key, map.next_value()?); }
                            }
                        }
                        Ok(record)
                    }
                }

                deserializer.deserialize_map(RecordVisitor)
            }
        }
    };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    #[default]
    Globs,
    LegacyCatalog,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Registry {
    pub version: Option<u32>,
    pub updated: Option<String>,
    pub selection: Selection,
    /// Tracked policy wins; local qa.registry.enforce is fallback only.
    pub enforce: Option<crate::config::RegistryEnforcement>,
    pub seeded_from: Vec<String>,
    pub traffic_snapshot: Option<Mapping>,
    pub ignore: Vec<String>,
    /// None inherits qa.user_facing_paths + qa.api_paths; [] selects no surfaces.
    pub surfaces: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub owner_groups: BTreeMap<String, OwnerGroup>,
    pub invariants: BTreeMap<String, Invariant>,
    /// Criteria graded by a person, or declared before their checker exists.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub manual_checks: BTreeMap<String, ManualCheck>,
    pub fixtures: Option<BTreeMap<String, Fixture>>,
    /// Shared provider budgets the release gate meters (`kind: rate|quota|balance`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, Resource>,
    pub bug_families: BTreeMap<String, BugFamily>,
    pub journeys: Vec<Journey>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(Registry {
    version,
    updated,
    selection,
    enforce,
    seeded_from,
    traffic_snapshot,
    ignore,
    surfaces,
    kinds,
    owner_groups,
    invariants,
    manual_checks,
    fixtures,
    resources,
    bug_families,
    journeys,
});

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OwnerGroup {
    pub description: String,
    pub globs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Invariant {
    pub statement: String,
    pub check: String,
    pub bug: Option<Value>,
    /// Declared criterion whose executable checker is not written yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_checker: Option<PendingChecker>,
    /// What counts, so a grader can apply the criterion without a judgment call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// Graded against each fixture's `labels` entry instead of one universal rule.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub labeled: bool,
    /// How a measured value is graded against its baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compare: Option<CompareSpec>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(Invariant {
    statement,
    check,
    bug,
    pending_checker,
    definition,
    labeled,
    compare,
});

const INVARIANT_FIELDS: [&str; 7] = [
    "statement",
    "check",
    "bug",
    "pending_checker",
    "definition",
    "labeled",
    "compare",
];
const MANUAL_CHECK_FIELDS: [&str; 7] = [
    "statement",
    "check",
    "reason",
    "pending_checker",
    "definition",
    "labeled",
    "compare",
];
const FIXTURE_FIELDS: [&str; 5] = ["description", "provider", "frozen", "recipe", "labels"];
const RESOURCE_FIELDS: [&str; 4] = ["kind", "per_min", "concurrency", "per_day"];

/// A registry `resources` entry. It declares a budget; it never names a command.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Resource {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_day: Option<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(Resource {
    kind,
    per_min,
    concurrency,
    per_day
});

/// A [`Resource`]'s validated budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// At most `per_min` units in any minute, `concurrency` requests in flight.
    Rate { per_min: u32, concurrency: Option<u32> },
    /// At most `per_day` units in any 24 hours.
    Quota { per_day: u32 },
    /// A consumable the adapter reports through `balance` and tops up through `grant`.
    Balance,
}

impl Resource {
    pub fn limit(&self) -> Result<Limit, String> {
        let positive = |name: &str, value: Option<u32>| match value {
            Some(0) => Err(format!("{name} must be positive")),
            value => Ok(value),
        };
        let (per_min, concurrency, per_day) = (
            positive("per_min", self.per_min)?,
            positive("concurrency", self.concurrency)?,
            positive("per_day", self.per_day)?,
        );
        match (self.kind.as_str(), per_min, concurrency, per_day) {
            ("rate", Some(per_min), concurrency, None) => Ok(Limit::Rate { per_min, concurrency }),
            ("rate", ..) => Err("kind rate needs per_min and takes no per_day".into()),
            ("quota", None, None, Some(per_day)) => Ok(Limit::Quota { per_day }),
            ("quota", ..) => Err("kind quota needs only per_day".into()),
            ("balance", None, None, None) => Ok(Limit::Balance),
            ("balance", ..) => Err("kind balance takes no limits; the adapter reports it".into()),
            (kind, ..) => Err(format!("kind {kind:?} must be rate, quota or balance")),
        }
    }
}

/// Numeric grading mode: `exact`, or `{banded: {min_ratio | max_delta}}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compare {
    /// Any difference from the baseline fails (coverage counts, presence).
    Exact,
    /// Wording noise inside the band passes; the measured % stays visible.
    Banded(Band),
}

/// A criterion's `compare` as written. A malformed value stays a validation
/// problem on that criterion instead of failing the whole registry load,
/// which also feeds impact and close-time checks.
#[derive(Debug, Clone, PartialEq)]
pub enum CompareSpec {
    Valid(Compare),
    Invalid { raw: Value, error: String },
}

impl CompareSpec {
    pub fn valid(&self) -> Option<&Compare> {
        match self {
            Self::Valid(compare) => Some(compare),
            Self::Invalid { .. } => None,
        }
    }
}

/// Singleton-map form (`exact`, `{banded: {...}}`), not serde_yaml's `!banded` tags.
impl<'de> Deserialize<'de> for CompareSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(deserializer)?;
        Ok(
            match serde_yaml::with::singleton_map_recursive::deserialize(raw.clone()) {
                Ok(compare) => Self::Valid(compare),
                Err(error) => Self::Invalid {
                    raw,
                    error: error.to_string(),
                },
            },
        )
    }
}

impl Serialize for CompareSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Valid(compare) => {
                serde_yaml::with::singleton_map_recursive::serialize(compare, serializer)
            }
            Self::Invalid { raw, .. } => raw.serialize(serializer),
        }
    }
}

/// Exactly one bound: `min_ratio` is a floor of `baseline * min_ratio`;
/// `max_delta` allows `|measured - baseline| <= max_delta`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Band {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delta: Option<f64>,
}

/// A graded measurement as the ledger records it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Grade {
    pub pass: bool,
    pub measured: f64,
    pub baseline: f64,
    /// `measured / baseline` as a percentage; absent for a zero baseline.
    pub pct_of_baseline: Option<f64>,
}

impl Compare {
    pub fn grade(&self, measured: f64, baseline: f64) -> Grade {
        let pass = match self {
            Self::Exact => measured == baseline,
            Self::Banded(band) => {
                band.min_ratio
                    .is_none_or(|ratio| measured >= baseline * ratio)
                    && band
                        .max_delta
                        .is_none_or(|delta| (measured - baseline).abs() <= delta)
            }
        };
        Grade {
            pass,
            measured,
            baseline,
            pct_of_baseline: (baseline != 0.0).then(|| measured / baseline * 100.0),
        }
    }

    fn problem(&self) -> Option<&'static str> {
        let Self::Banded(band) = self else {
            return None;
        };
        match (band.min_ratio, band.max_delta) {
            (Some(ratio), None) if ratio > 0.0 && ratio <= 1.0 => None,
            (None, Some(delta)) if delta.is_finite() && delta >= 0.0 => None,
            (Some(_), None) => Some("banded min_ratio must be in (0, 1]"),
            (None, Some(_)) => Some("banded max_delta must be a non-negative number"),
            _ => Some("banded needs exactly one of min_ratio or max_delta"),
        }
    }
}

/// Automation debt with an accountable owner; overdue once `due` has passed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PendingChecker {
    pub owner: String,
    pub due: String,
}

impl Journey {
    /// Live in `env`: its `env_status` entry, else the journey's `status`.
    /// The release gate scopes its walk the same way.
    pub fn live_in(&self, env: &str) -> bool {
        self.env_status.get(env).map_or(self.status == "live", |s| s.status == "live")
    }

    /// A golden set: the journey expands into cells × runs.
    pub fn is_golden_set(&self) -> bool {
        self.fixture_set.is_some() || !self.matrix.is_empty() || self.runs.is_some_and(|runs| runs > 1)
    }
}

/// The frozen artifact a golden-set record grades.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetArtifact {
    pub sha256: String,
    pub storage_key: String,
}

/// One golden-set record: a fixture id, its frozen artifact and the expected
/// value of each labeled criterion (cas-c4b5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetRecord {
    pub id: String,
    pub frozen: SetArtifact,
    #[serde(default)]
    pub labels: BTreeMap<String, Value>,
}

/// A plain token: letters, digits, `_` and `-` (row ids embed it).
fn plain_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

/// Reads a journey's `fixture_set`: JSONL records, or a TSV whose header
/// names `id`, `sha256`, `storage_key` and optionally `labels` (a JSON object
/// cell; empty is unlabeled). Every malformed record is named.
pub fn load_fixture_set(project_root: &Path, path: &str) -> Result<Vec<SetRecord>, Vec<String>> {
    let relative = Path::new(path);
    if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(vec![format!("fixture_set {path:?} must stay inside the repository")]);
    }
    let text = std::fs::read_to_string(project_root.join(relative))
        .map_err(|e| vec![format!("fixture_set {path:?} is unreadable: {e}")])?;
    let mut problems = Vec::new();
    let mut records = Vec::new();
    if path.ends_with(".jsonl") {
        for (index, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            match serde_json::from_str::<SetRecord>(line) {
                Ok(record) => records.push(record),
                Err(e) => problems.push(format!("{path}:{} is not a {{id, frozen, labels}} record: {e}", index + 1)),
            }
        }
    } else if path.ends_with(".tsv") {
        let mut lines = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty());
        let header: Vec<&str> = lines.next().map(|(_, h)| h.split('\t').map(str::trim).collect()).unwrap_or_default();
        let column = |name: &str| header.iter().position(|h| *h == name);
        let (Some(id), Some(sha), Some(key)) = (column("id"), column("sha256"), column("storage_key")) else {
            return Err(vec![format!("{path} header needs id, sha256 and storage_key columns")]);
        };
        let labels = column("labels");
        for (index, line) in lines {
            let cells: Vec<&str> = line.split('\t').collect();
            let cell = |at: usize| cells.get(at).map_or("", |c| c.trim());
            let parsed = match labels.map(cell).filter(|l| !l.is_empty()) {
                Some(json) => serde_json::from_str::<BTreeMap<String, Value>>(json),
                None => Ok(BTreeMap::new()),
            };
            match parsed {
                Ok(labels) => records.push(SetRecord {
                    id: cell(id).into(),
                    frozen: SetArtifact { sha256: cell(sha).into(), storage_key: cell(key).into() },
                    labels,
                }),
                Err(e) => problems.push(format!("{path}:{} labels is not a JSON object: {e}", index + 1)),
            }
        }
    } else {
        return Err(vec![format!("fixture_set {path:?} must be a .tsv or .jsonl file")]);
    }
    let mut seen = std::collections::BTreeSet::new();
    for record in &records {
        if !plain_token(&record.id) {
            problems.push(format!("{path} record id {:?} must be 1-64 letters, digits, _ or -", record.id));
        } else if !seen.insert(record.id.as_str()) {
            problems.push(format!("{path} repeats record id {}", record.id));
        }
        let frozen = &record.frozen;
        if frozen.sha256.len() != 64 || !frozen.sha256.bytes().all(|b| b.is_ascii_hexdigit()) || frozen.storage_key.trim().is_empty() {
            problems.push(format!("{path} record {} needs a frozen sha256 (64 hex) and storage_key", record.id));
        }
    }
    if records.is_empty() && problems.is_empty() {
        problems.push(format!("{path} has no records"));
    }
    if problems.is_empty() { Ok(records) } else { Err(problems) }
}

/// Who turns a hand-walked journey (no `spec`, or walked through the release
/// gate's `record`) into automation: cas-160c's manual-check debt, per journey.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Automation {
    pub owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

impl Automation {
    /// `owner X[, task T][, due D]`.
    pub fn summary(&self) -> String {
        let mut summary = format!("owner {}", self.owner);
        if let Some(task) = &self.task {
            summary.push_str(&format!(", task {task}"));
        }
        if let Some(due) = &self.due {
            summary.push_str(&format!(", due {due}"));
        }
        summary
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ManualCheck {
    pub statement: String,
    pub check: String,
    /// `human_judgment` stays manual; `checker_unwritten` needs `pending_checker`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_checker: Option<PendingChecker>,
    /// What counts, so a grader can apply the criterion without a judgment call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    /// Graded against each fixture's `labels` entry instead of one universal rule.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub labeled: bool,
    /// How a measured value is graded against its baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compare: Option<CompareSpec>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(ManualCheck {
    statement,
    check,
    reason,
    pending_checker,
    definition,
    labeled,
    compare,
});

#[derive(Debug, Clone, Default, Serialize)]
pub struct Fixture {
    pub description: String,
    #[serde(flatten)]
    pub source: Option<FixtureSource>,
    /// Expected value per labeled criterion ID for this fixture.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl<'de> Deserialize<'de> for Fixture {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FixtureVisitor;

        impl<'de> serde::de::Visitor<'de> for FixtureVisitor {
            type Value = Fixture;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("fixture mapping")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Fixture, M::Error> {
                let mut fixture = Fixture::default();
                let mut seen = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    // Repeated sources belong in extra for a local exclusivity problem.
                    if matches!(key.as_str(), "description" | "labels") && !seen.insert(key.clone())
                    {
                        return Err(serde::de::Error::custom(format!("duplicate field {key:?}")));
                    }
                    match key.as_str() {
                        "description" => fixture.description = map.next_value()?,
                        "labels" => fixture.labels = map.next_value()?,
                        "provider" | "frozen" | "recipe" => {
                            let raw = map.next_value::<Value>()?;
                            let source = match key.as_str() {
                                "provider" => {
                                    serde_yaml::from_value(raw.clone()).map(FixtureSource::Provider)
                                }
                                "frozen" => {
                                    serde_yaml::from_value(raw.clone()).map(FixtureSource::Frozen)
                                }
                                _ => serde_yaml::from_value(raw.clone()).map(FixtureSource::Recipe),
                            };
                            // Missing/malformed and multiple sources remain local
                            // fixture validation problems, preserving their YAML.
                            if fixture.source.is_none()
                                && let Ok(source) = source
                            {
                                fixture.source = Some(source);
                            } else {
                                fixture.extra.insert(key, raw);
                            }
                        }
                        _ => {
                            fixture.extra.insert(key, map.next_value()?);
                        }
                    }
                }
                Ok(fixture)
            }
        }

        deserializer.deserialize_map(FixtureVisitor)
    }
}

/// Exactly one source, flattened to preserve the original `provider` form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureSource {
    Provider(String),
    Frozen(FrozenFixture),
    Recipe(FixtureRecipe),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FrozenFixture {
    pub sha256: String,
    pub storage_key: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(FrozenFixture {
    sha256,
    storage_key
});

#[derive(Debug, Clone, Default, Serialize)]
pub struct FixtureRecipe {
    pub name: String,
    pub envs: Vec<String>,
    pub provides: Vec<String>,
    pub ttl_mins: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(FixtureRecipe {
    name,
    envs,
    provides,
    ttl_mins
});

impl FixtureRecipe {
    pub fn valid(&self) -> bool {
        let names = |values: &[String]| {
            !values.is_empty()
                && values.iter().all(|s| !s.trim().is_empty())
                && values
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == values.len()
        };
        !self.name.trim().is_empty()
            && names(&self.envs)
            && names(&self.provides)
            && self.ttl_mins > 0
            && self.extra.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BugFamily {
    pub title: String,
    pub found: Option<Value>,
    pub fixed_by: Vec<String>,
    pub tasks: Vec<String>,
    pub invariants: Vec<String>,
    pub journeys: Vec<String>,
}

/// Gabber's backend/frontend map and a flat glob list have the same meaning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Owners {
    List(Vec<String>),
    Map(BTreeMap<String, Vec<String>>),
}

impl Default for Owners {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

impl Owners {
    pub fn globs(&self) -> Vec<&str> {
        match self {
            Self::List(globs) => globs.iter().map(String::as_str).collect(),
            Self::Map(sides) => sides.values().flatten().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Journey {
    pub id: String,
    pub name: Option<String>,
    pub status: String,
    pub status_reason: Option<String>,
    pub env_status: BTreeMap<String, EnvironmentStatus>,
    pub kind: Option<String>,
    pub entry: Option<Entry>,
    pub steps: Vec<String>,
    pub owner_groups: Vec<String>,
    pub owners: Owners,
    pub invariants: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual_checks: Vec<String>,
    pub fixtures: Vec<String>,
    pub traffic: Option<Traffic>,
    pub spec: Option<String>,
    /// The automation owner while the journey is hand-walked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation: Option<Automation>,
    /// Golden set: a tracked `.tsv` or `.jsonl` file of `{id, frozen, labels}`
    /// records, one per fixture the matrix crosses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture_set: Option<String>,
    /// Golden-set axes, for example `{route: [...], source: [...]}`; a cell
    /// takes one value of every axis.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub matrix: BTreeMap<String, Vec<String>>,
    /// Independent runs each golden-set cell needs (default 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<u32>,
    pub known_failure: Option<String>,
    pub finisher: Option<String>,
    pub evidence: Option<Mapping>,
    /// Subject kinds and invariant IDs the gate may read back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readback: Option<crate::qa_readback::Declaration>,
    /// Flags whose target-environment values the journey's evidence depends on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on_flags: Vec<String>,
    /// Result identity that makes runs independent, e.g. `post_audit_v2_runs.id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub independence_key: Option<String>,
    /// Units of each declared resource one run of the journey uses.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub consumes: BTreeMap<String, u32>,
    /// A main journey: the release gate walks it on every gate, impacted or not.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub release_always: bool,
    pub seeded: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

deserialize_with_extra!(Journey {
    id,
    name,
    status,
    status_reason,
    env_status,
    kind,
    entry,
    steps,
    owner_groups,
    owners,
    invariants,
    manual_checks,
    fixtures,
    traffic,
    spec,
    automation,
    fixture_set,
    matrix,
    runs,
    known_failure,
    finisher,
    evidence,
    readback,
    depends_on_flags,
    independence_key,
    consumes,
    release_always,
    seeded,
});

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvironmentStatus {
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub url: String,
    pub trigger: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Traffic {
    #[serde(deserialize_with = "nullable_share")]
    pub share_pct: Option<f64>,
    #[serde(default)]
    pub basis: String,
    #[serde(default)]
    pub source: Option<String>,
}

fn nullable_share<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<f64>, D::Error> {
    Option::<f64>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub code: String,
    pub subject: String,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct ValidationReport {
    pub problems: Vec<Problem>,
    pub debt: Vec<Problem>,
}

/// Parse delivered-head bytes without consulting the working tree or Git.
pub fn parse(contents: &str) -> Result<Registry> {
    serde_yaml::from_str(contents).context("invalid journey registry YAML")
}

pub fn load(path: &Path) -> Result<Registry> {
    parse(&std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("load {}", path.display()))
}

pub fn check(project_root: &Path, registry_path: &Path) -> Result<ValidationReport> {
    let registry = load(&project_root.join(registry_path))?;
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z", "--cached", "--full-name"])
        .current_dir(project_root)
        .output()
        .context("git ls-files for journey registry")?;
    anyhow::ensure!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let paths = String::from_utf8(output.stdout).context("tracked paths must be UTF-8")?;
    let tracked: Vec<String> = paths.split('\0').filter(|p| !p.is_empty()).map(str::to_owned).collect();
    let mut report = registry.validate(&tracked);
    registry.validate_golden_sets(project_root, &tracked, &mut report);
    Ok(report)
}

/// Compile GitHub-style path filters while treating route brackets literally.
fn pattern(raw: &str) -> Result<glob::Pattern, String> {
    let raw = raw.strip_prefix('!').unwrap_or(raw);
    if raw.is_empty() {
        return Err("empty path filter".to_owned());
    }
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '[' => escaped.push_str("[[]"),
            ']' => escaped.push_str("[]]"),
            _ => escaped.push(ch),
        }
    }
    glob::Pattern::new(&escaped).map_err(|error| error.to_string())
}

fn pattern_matches(pattern: &glob::Pattern, path: &str) -> bool {
    pattern.matches_with(
        path,
        glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        },
    )
}

/// Ordered path filters: the last matching positive/negative entry wins.
/// Invalid filters never match; `validate` reports their errors separately.
pub fn matches_path<'a>(globs: impl IntoIterator<Item = &'a str>, path: &str) -> bool {
    let mut matched = false;
    for raw in globs {
        if pattern(raw).is_ok_and(|p| pattern_matches(&p, path)) {
            matched = !raw.starts_with('!');
        }
    }
    matched
}

impl ValidationReport {
    fn problem(&mut self, code: &str, subject: &str, message: impl Into<String>) {
        self.problems.push(Problem {
            code: code.into(),
            subject: subject.into(),
            message: message.into(),
        });
    }

    fn debt(&mut self, code: &str, subject: &str, message: &str) {
        self.debt.push(Problem {
            code: code.into(),
            subject: subject.into(),
            message: message.into(),
        });
    }

    fn globs<'a>(
        &mut self,
        subject: &str,
        globs: impl IntoIterator<Item = &'a str>,
        tracked: &[String],
        check_orphans: bool,
    ) {
        for raw in globs {
            match pattern(raw) {
                Err(error) => self.problem("invalid_glob", subject, format!("{raw:?}: {error}")),
                Ok(p) if check_orphans && !tracked.iter().any(|path| pattern_matches(&p, path)) => {
                    self.problem(
                        "orphan_glob",
                        subject,
                        format!("glob {raw:?} matches no tracked file"),
                    );
                }
                Ok(_) => {}
            }
        }
    }
}

fn iso_date(date: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .filter(|parsed| date.len() == 10 && parsed.format("%Y-%m-%d").to_string() == date)
}

impl ValidationReport {
    /// Reject tagged extras and near-misses of known fields, so invalid
    /// metadata and typos become local problems instead of load failures.
    fn extra_fields(&mut self, subject: &str, extra: &BTreeMap<String, Value>, known: &[&str]) {
        fn tagged(value: &Value) -> bool {
            match value {
                Value::Tagged(_) => true,
                Value::Sequence(items) => items.iter().any(tagged),
                Value::Mapping(items) => items
                    .iter()
                    .any(|(key, value)| tagged(key) || tagged(value)),
                _ => false,
            }
        }
        for (key, value) in extra {
            // A second source is an exclusivity error, not a misspelling.
            if known.contains(&key.as_str()) {
                continue;
            }
            // At most two edits, and no more than a third of the field, so
            // short free-form keys (`tag` vs `bug`) stay free-form.
            if let Some(field) = known.iter().find(|field| {
                let distance =
                    crate::sync::skill_suggest::levenshtein(&key.to_ascii_lowercase(), field);
                distance <= 2 && distance * 3 <= field.len()
            }) {
                self.problem(
                    "unknown_field",
                    subject,
                    format!("unknown field {key:?}; did you mean {field}?"),
                );
            } else if tagged(value) {
                self.problem(
                    "unknown_field",
                    subject,
                    format!(
                        "unknown field {key:?}; YAML tags are not supported in extension fields"
                    ),
                );
            }
        }
    }

    fn compare(&mut self, subject: &str, compare: Option<&CompareSpec>) {
        match compare {
            Some(CompareSpec::Invalid { error, .. }) => self.problem(
                "compare",
                subject,
                format!(
                    "invalid compare ({error}); use exact or {{banded: {{min_ratio | max_delta}}}}"
                ),
            ),
            Some(CompareSpec::Valid(compare)) => {
                if let Some(message) = compare.problem() {
                    self.problem("compare", subject, message);
                }
            }
            None => {}
        }
    }

    /// Validate a pending checker; its owner/due summary when it is well formed.
    fn pending_checker(
        &mut self,
        subject: &str,
        pending: &PendingChecker,
        today: chrono::NaiveDate,
    ) -> Option<String> {
        match iso_date(&pending.due) {
            Some(due) if !pending.owner.trim().is_empty() => {
                let summary = format!("owner {}, due {}", pending.owner, pending.due);
                if due < today {
                    self.problem(
                        "overdue_checker",
                        subject,
                        format!("checker is overdue ({summary}); write it or move the date"),
                    );
                }
                Some(summary)
            }
            _ => {
                self.problem(
                    "pending_checker",
                    subject,
                    "pending_checker needs owner and due (YYYY-MM-DD)",
                );
                None
            }
        }
    }
}

impl ValidationReport {
    /// A live journey without a spec is hand-walked: debt with its automation
    /// owner, or unowned debt. A `due` date is checked like a pending checker.
    fn hand_walk(&mut self, subject: &str, automation: Option<&Automation>, today: chrono::NaiveDate) {
        let Some(automation) = automation.filter(|a| !a.owner.trim().is_empty()) else {
            self.debt(
                "hand_walk_unowned",
                subject,
                "hand-walked journey (no spec) has no automation owner; set automation: {owner, task?, due?}",
            );
            return;
        };
        if let Some(due) = &automation.due {
            match iso_date(due) {
                None => {
                    self.problem("automation", subject, "automation.due must be a YYYY-MM-DD date");
                    return;
                }
                Some(due) if due < today => self.problem(
                    "overdue_automation",
                    subject,
                    format!("automation is overdue ({}); automate it or move automation.due", automation.summary()),
                ),
                Some(_) => {}
            }
        }
        self.debt("hand_walk", subject, &format!("hand-walked: automation {}", automation.summary()));
    }
}

fn valid_status(status: &str) -> bool {
    matches!(status, "live" | "dark" | "dead")
}

fn present(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.trim().is_empty())
}

impl Registry {
    /// Golden sets (finding 18): the matrix, runs and the tracked fixture set
    /// must be well formed; a record without a label for one of the journey's
    /// labeled criteria is `unlabeled_cell` debt.
    fn validate_golden_sets(&self, project_root: &Path, tracked: &[String], report: &mut ValidationReport) {
        for journey in &self.journeys {
            let id = journey.id.as_str();
            if journey.runs == Some(0) {
                report.problem("runs", id, "runs must be at least 1");
            }
            for (axis, values) in &journey.matrix {
                if !plain_token(axis) || values.is_empty() || !values.iter().all(|v| plain_token(v)) {
                    report.problem(
                        "matrix",
                        id,
                        format!("matrix.{axis} needs a non-empty list; axes and values are 1-64 letters, digits, _ or -"),
                    );
                }
            }
            let Some(path) = &journey.fixture_set else { continue };
            if !tracked.contains(path) {
                report.problem("fixture_set", id, format!("fixture_set {path:?} is not tracked"));
                continue;
            }
            let records = match load_fixture_set(project_root, path) {
                Ok(records) => records,
                Err(problems) => {
                    for problem in problems {
                        report.problem("fixture_set", id, problem);
                    }
                    continue;
                }
            };
            let labeled: Vec<&String> = journey
                .invariants
                .iter()
                .chain(&journey.manual_checks)
                .filter(|c| self.criterion(c).is_some_and(|(_, labeled)| labeled))
                .collect();
            for record in &records {
                let missing: Vec<&str> = labeled
                    .iter()
                    .filter(|c| record.labels.get(c.as_str()).is_none_or(Value::is_null))
                    .map(|c| c.as_str())
                    .collect();
                if !missing.is_empty() {
                    report.debt(
                        "unlabeled_cell",
                        &format!("{id}.{}", record.id),
                        &format!("no label for {} in {path}", missing.join(", ")),
                    );
                }
            }
        }
    }

    /// Resolve shared groups after the journey's direct owners in declared group order.
    pub fn owner_globs<'a>(&'a self, journey: &'a Journey) -> Vec<&'a str> {
        let mut globs = journey.owners.globs();
        for name in &journey.owner_groups {
            if let Some(group) = self.owner_groups.get(name) {
                globs.extend(group.globs.iter().map(String::as_str));
            }
        }
        globs
    }

    pub fn effective_surfaces<'a>(&'a self, qa: &'a crate::config::QaConfig) -> Vec<&'a str> {
        match &self.surfaces {
            Some(globs) => globs.iter().map(String::as_str).collect(),
            None => qa
                .user_facing_paths
                .iter()
                .chain(&qa.api_paths)
                .map(String::as_str)
                .collect(),
        }
    }

    /// `(definition, labeled)` for an invariant or manual-check ID.
    fn criterion(&self, id: &str) -> Option<(Option<&str>, bool)> {
        self.invariants
            .get(id)
            .map(|c| (c.definition.as_deref(), c.labeled))
            .or_else(|| {
                self.manual_checks
                    .get(id)
                    .map(|c| (c.definition.as_deref(), c.labeled))
            })
    }

    /// Labeled criteria need a definition and a label on every fixture a journey pairs them with.
    fn validate_labels(&self, report: &mut ValidationReport) {
        let criteria = self.invariants.keys().map(|id| ("invariants", id));
        for (kind, id) in criteria.chain(self.manual_checks.keys().map(|id| ("manual_checks", id)))
        {
            if let Some((definition, true)) = self.criterion(id)
                && !present(definition)
            {
                report.problem(
                    "missing_definition",
                    &format!("{kind}.{id}"),
                    "labeled criteria need a definition of what counts",
                );
            }
        }
        let Some(fixtures) = &self.fixtures else {
            return;
        };
        for (name, fixture) in fixtures {
            for criterion in fixture.labels.keys() {
                match self.criterion(criterion) {
                    None => report.problem(
                        "unknown_label",
                        &format!("fixtures.{name}"),
                        format!("label for unknown criterion {criterion:?}"),
                    ),
                    Some((_, false)) => report.problem(
                        "label_without_labeled",
                        &format!("fixtures.{name}"),
                        format!("{criterion} has a label but is not labeled: true"),
                    ),
                    Some((_, true)) => {}
                }
            }
        }
        let mut owed = std::collections::BTreeMap::new();
        for journey in &self.journeys {
            for fixture in &journey.fixtures {
                let Some(labels) = fixtures.get(fixture).map(|f| &f.labels) else {
                    continue;
                };
                for criterion in journey.invariants.iter().chain(&journey.manual_checks) {
                    if self
                        .criterion(criterion)
                        .is_some_and(|(_, labeled)| labeled)
                        && labels.get(criterion).is_none_or(Value::is_null)
                    {
                        owed.entry((fixture.as_str(), criterion.as_str()))
                            .or_insert(journey.id.as_str());
                    }
                }
            }
        }
        for ((fixture, criterion), journey) in owed {
            report.debt(
                "unlabeled_fixture",
                &format!("fixtures.{fixture}"),
                &format!("no label for labeled criterion {criterion} (journey {journey})"),
            );
        }
    }

    /// Validate in memory against the caller's tracked-file snapshot (no subprocess).
    pub fn validate(&self, tracked: &[String]) -> ValidationReport {
        use std::collections::BTreeSet;
        let mut report = ValidationReport::default();
        report.extra_fields("registry", &self.extra, &[]);
        if self.version != Some(1) {
            report.problem("version", "registry", "version must be 1");
        }
        let today = chrono::Utc::now().date_naive();
        if self.updated.as_deref().and_then(iso_date).is_none() {
            report.problem(
                "updated",
                "registry",
                "updated must be an ISO date (YYYY-MM-DD)",
            );
        }
        if self.journeys.is_empty() {
            report.problem("journeys", "registry", "journeys must be a non-empty list");
        }
        if self.selection == Selection::LegacyCatalog {
            for path in ["docs/qa/journeys.md", "scripts/journeys-for-diff.py"] {
                if !tracked.iter().any(|file| file == path) {
                    report.problem(
                        "untracked_spec",
                        "selection",
                        format!("legacy_catalog requires tracked {path}"),
                    );
                }
            }
        }
        let ids: BTreeSet<_> = self.journeys.iter().map(|j| j.id.as_str()).collect();
        let mut seen = BTreeSet::new();
        for (name, group) in &self.owner_groups {
            let subject = format!("owner_groups.{name}");
            if group.description.trim().is_empty() || group.globs.is_empty() {
                report.problem(
                    "owner_group",
                    &subject,
                    "description and non-empty globs are required",
                );
            }
            report.globs(
                &subject,
                group.globs.iter().map(String::as_str),
                tracked,
                true,
            );
        }
        for (name, invariant) in &self.invariants {
            let subject = format!("invariants.{name}");
            if invariant.statement.trim().is_empty() || invariant.check.trim().is_empty() {
                report.problem("invariant", &subject, "statement and check are required");
            }
            report.extra_fields(&subject, &invariant.extra, &INVARIANT_FIELDS);
            report.compare(&subject, invariant.compare.as_ref());
            if let Some(pending) = &invariant.pending_checker {
                if let Some(summary) = report.pending_checker(&subject, pending, today) {
                    report.debt(
                        "pending_checker",
                        &subject,
                        &format!("checker pending: {summary}"),
                    );
                }
            }
        }
        for (name, resource) in &self.resources {
            let subject = format!("resources.{name}");
            report.extra_fields(&subject, &resource.extra, &RESOURCE_FIELDS);
            if let Err(message) = resource.limit() {
                report.problem("resource", &subject, message);
            }
        }
        for (name, manual) in &self.manual_checks {
            let subject = format!("manual_checks.{name}");
            if manual.statement.trim().is_empty() || manual.check.trim().is_empty() {
                report.problem("manual_check", &subject, "statement and check are required");
            }
            if self.invariants.contains_key(name) {
                report.problem(
                    "duplicate_criterion",
                    &subject,
                    "id is also an invariant; criterion ids must be unique",
                );
            }
            report.extra_fields(&subject, &manual.extra, &MANUAL_CHECK_FIELDS);
            report.compare(&subject, manual.compare.as_ref());
            match (manual.reason.as_deref(), &manual.pending_checker) {
                (Some("human_judgment"), Some(_)) => report.problem(
                    "contradiction",
                    &subject,
                    "human_judgment cannot have a pending_checker; use checker_unwritten",
                ),
                (Some("human_judgment"), None) => report.debt(
                    "manual_human_judgment",
                    &subject,
                    "manual check: needs human judgment",
                ),
                (Some("checker_unwritten"), Some(pending)) => {
                    if let Some(summary) = report.pending_checker(&subject, pending, today) {
                        report.debt(
                            "manual_checker_unwritten",
                            &subject,
                            &format!("manual check: checker unwritten, {summary}"),
                        );
                    }
                }
                (Some("checker_unwritten"), None) => report.problem(
                    "pending_checker",
                    &subject,
                    "reason checker_unwritten needs pending_checker {owner, due}",
                ),
                (None, _) => report.debt(
                    "manual_unclassified",
                    &subject,
                    "manual check has no reason; set human_judgment or checker_unwritten",
                ),
                (Some(other), _) => report.problem(
                    "manual_reason",
                    &subject,
                    format!("unknown reason {other:?}; use human_judgment or checker_unwritten"),
                ),
            }
        }
        if let Some(fixtures) = &self.fixtures {
            for (name, fixture) in fixtures {
                let subject = format!("fixtures.{name}");
                let valid = match fixture.source.as_ref() {
                    None => false,
                    Some(FixtureSource::Provider(provider)) => {
                        let provider = provider.trim();
                        if provider.to_ascii_lowercase().starts_with("http://")
                            || provider.to_ascii_lowercase().starts_with("https://")
                        {
                            report.debt("live_url_fixture", &subject,
                                "live URL can change or disappear; use frozen {sha256, storage_key}");
                        }
                        !provider.is_empty()
                    }
                    Some(FixtureSource::Frozen(frozen)) => {
                        report.extra_fields(
                            &format!("{subject}.frozen"),
                            &frozen.extra,
                            &["sha256", "storage_key"],
                        );
                        frozen.sha256.len() == 64
                            && frozen.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                            && !frozen.storage_key.trim().is_empty()
                            && frozen.extra.is_empty()
                    }
                    Some(FixtureSource::Recipe(recipe)) => {
                        report.extra_fields(
                            &format!("{subject}.recipe"),
                            &recipe.extra,
                            &["name", "envs", "provides", "ttl_mins"],
                        );
                        recipe.valid()
                    }
                };
                let multiple = ["provider", "frozen", "recipe"]
                    .iter()
                    .any(|key| fixture.extra.contains_key(*key));
                if fixture.description.trim().is_empty() || !valid || multiple {
                    report.problem("fixture", &subject,
                        "description and exactly one valid provider, frozen {sha256, storage_key}, or recipe {name, envs, provides, ttl_mins} are required");
                }
                report.extra_fields(&subject, &fixture.extra, &FIXTURE_FIELDS);
            }
        }
        report.globs(
            "ignore",
            self.ignore.iter().map(String::as_str),
            tracked,
            false,
        );
        if let Some(surfaces) = &self.surfaces {
            report.globs(
                "surfaces",
                surfaces.iter().map(String::as_str),
                tracked,
                true,
            );
        }
        for journey in &self.journeys {
            let id = journey.id.as_str();
            report.extra_fields(id, &journey.extra, &[]);
            if id.is_empty()
                || !id.as_bytes()[0].is_ascii_alphanumeric()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            {
                report.problem("id", id, "id must match ^[A-Za-z0-9][A-Za-z0-9._-]*$");
            }
            if !seen.insert(id) {
                report.problem("duplicate_id", id, "duplicate journey id");
            }
            if !valid_status(&journey.status) {
                report.problem("status", id, "status must be live, dark or dead");
            } else if journey.status != "live" && !present(journey.status_reason.as_deref()) {
                report.problem(
                    "status_reason",
                    id,
                    "dark/dead journeys need a status_reason",
                );
            }
            for (env, status) in &journey.env_status {
                if !matches!(env.as_str(), "staging" | "prod") {
                    report.problem(
                        "env_status",
                        id,
                        format!("unknown env_status key {env:?}; use staging or prod"),
                    );
                }
                if !valid_status(&status.status) {
                    report.problem(
                        "env_status",
                        id,
                        format!("env_status.{env}.status must be live, dark or dead"),
                    );
                }
            }
            if let (Some(kind), Some(kinds)) = (&journey.kind, &self.kinds) {
                if !kinds.contains(kind) {
                    report.problem("kind", id, format!("kind {kind:?} is not in kinds"));
                }
            }
            for group in &journey.owner_groups {
                if !self.owner_groups.contains_key(group) {
                    report.problem(
                        "unknown_owner_group",
                        id,
                        format!("unknown owner group {group:?}"),
                    );
                }
            }
            if let Some(readback) = &journey.readback {
                for message in readback.problems() {
                    report.problem("readback", id, message);
                }
            }
            for flag in &journey.depends_on_flags {
                let valid = !flag.is_empty()
                    && flag.len() <= 128
                    && flag.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-:/".contains(&b));
                if !valid {
                    report.problem(
                        "depends_on_flags",
                        id,
                        format!("flag {flag:?} must be 1-128 letters, digits or ._-:/"),
                    );
                }
            }
            if let Some(key) = &journey.independence_key {
                let valid = !key.is_empty()
                    && key.len() <= 128
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
                if !valid {
                    report.problem(
                        "independence_key",
                        id,
                        format!("independence_key {key:?} must be 1-128 letters, digits or ._-"),
                    );
                }
            }
            for (resource, amount) in &journey.consumes {
                if !self.resources.contains_key(resource) {
                    report.problem("consumes", id, format!("unknown resource {resource:?}"));
                } else if *amount == 0 {
                    report.problem("consumes", id, format!("{resource} must consume at least 1"));
                }
            }
            for invariant in &journey.invariants {
                if !self.invariants.contains_key(invariant) {
                    report.problem(
                        "unknown_invariant",
                        id,
                        format!("unknown invariant {invariant:?}"),
                    );
                }
            }
            for manual in &journey.manual_checks {
                if !self.manual_checks.contains_key(manual) {
                    report.problem(
                        "unknown_manual_check",
                        id,
                        format!("unknown manual check {manual:?}"),
                    );
                }
            }
            if let Some(fixtures) = &self.fixtures {
                for fixture in &journey.fixtures {
                    if !fixtures.contains_key(fixture) {
                        report.problem(
                            "unknown_fixture",
                            id,
                            format!("unknown fixture {fixture:?}"),
                        );
                    }
                }
            }
            if let Some(finisher) = &journey.finisher {
                if !ids.contains(finisher.as_str()) {
                    report.problem(
                        "unknown_finisher",
                        id,
                        format!("unknown finisher {finisher:?}"),
                    );
                }
            }
            report.globs(
                id,
                journey.owners.globs(),
                tracked,
                journey.status != "dead",
            );
            if let Some(spec) = &journey.spec {
                if !tracked.contains(spec) {
                    report.problem(
                        "untracked_spec",
                        id,
                        format!("spec {spec:?} is not tracked"),
                    );
                }
            }
            if journey.seeded {
                report.debt("seeded", id, "seeded journey needs curation");
            }
            if ["staging", "prod"].iter().any(|env| journey.live_in(env)) && !present(journey.spec.as_deref()) {
                report.hand_walk(id, journey.automation.as_ref(), today);
            }
            if journey.status == "live" {
                if !journey.entry.as_ref().is_some_and(|entry| {
                    !entry.url.trim().is_empty() && !entry.trigger.trim().is_empty()
                }) {
                    report.problem(
                        "missing_entry",
                        id,
                        "live journeys need entry.url and entry.trigger",
                    );
                }
                if !self
                    .owner_globs(journey)
                    .iter()
                    .any(|glob| !glob.starts_with('!') && !glob.is_empty())
                {
                    report.problem(
                        "missing_owners",
                        id,
                        "live journeys need owners or an owner group with globs",
                    );
                }
                if !journey.seeded {
                    if journey.steps.is_empty()
                        || journey.steps.iter().any(|step| step.trim().is_empty())
                    {
                        report.problem("missing_steps", id, "live journeys need non-empty steps");
                    }
                    if journey.invariants.is_empty() {
                        report.problem("missing_invariants", id, "live journeys need invariants");
                    }
                    if journey.traffic.is_none() {
                        report.problem(
                            "missing_traffic",
                            id,
                            "live journeys need traffic.share_pct and traffic.basis",
                        );
                    }
                }
                if journey
                    .traffic
                    .as_ref()
                    .is_some_and(|traffic| traffic.share_pct.is_none())
                {
                    report.debt(
                        "unmeasured_traffic",
                        id,
                        "live journey traffic share_pct is null",
                    );
                }
            }
            // A null share is unmeasured debt: there is no count for a basis
            // to describe, which keeps gabber's `{share_pct: null}` rows valid.
            if let Some(traffic) = &journey.traffic {
                if traffic.share_pct.is_some() && traffic.basis.trim().is_empty() {
                    report.problem(
                        "traffic_basis",
                        id,
                        "traffic.basis must say what the share counts",
                    );
                }
            }
        }
        self.validate_labels(&mut report);
        for (name, family) in &self.bug_families {
            let subject = format!("bug_families.{name}");
            if family.title.trim().is_empty() {
                report.problem("bug_family", &subject, "title is required");
            }
            for invariant in &family.invariants {
                if !self.invariants.contains_key(invariant) {
                    report.problem(
                        "unknown_invariant",
                        &subject,
                        format!("unknown invariant {invariant:?}"),
                    );
                }
            }
            for journey in &family.journeys {
                if !ids.contains(journey.as_str()) {
                    report.problem(
                        "unknown_journey",
                        &subject,
                        format!("unknown journey {journey:?}"),
                    );
                }
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const GABBER: &str = r#"
version: 1
updated: "2026-10-21"
seeded_from: [routes, incident-ledgers]
traffic_snapshot: {as_of: "2026-10-20", window_days: 30, accounts: 100}
ignore: ["**/*.test.ts"]
surfaces: ["pages/**"]
kinds: [signup]
owner_groups:
  account:
    description: Shared account logic
    globs: ["server/auth/**"]
invariants:
  INV-CLAIM:
    statement: One claimed workspace
    check: Count workspaces and read claimed_at
    bug: {task: cas-1234}
fixtures:
  fresh: {description: New account, provider: account-seed}
bug_families:
  claim:
    title: Workspace was unclaimed
    found: "2026-10-20"
    fixed_by: [3573, abc123]
    tasks: [cas-1234]
    invariants: [INV-CLAIM]
    journeys: [B2-home]
journeys:
  - id: B2-home
    name: Home signup
    status: live
    env_status: {staging: {status: live, note: Available}, prod: {status: live}}
    kind: signup
    entry: {url: /, trigger: Get started}
    steps: [Enter email, Claim workspace]
    owner_groups: [account]
    owners: {frontend: ["pages/[id].vue"], backend: ["server/auth/**"]}
    invariants: [INV-CLAIM]
    fixtures: [fresh]
    traffic: {share_pct: 80, basis: New accounts, source: Analytics}
    spec: tests/signup.spec.ts
    finisher: B2-home
    evidence: {verdict: PASS}
"#;

    /// GABBER plus a pending invariant checker and one manual check per reason.
    fn with_criteria(due: &str) -> String {
        GABBER
            .replace(
                "    bug: {task: cas-1234}\n",
                &format!("    bug: {{task: cas-1234}}\n    pending_checker: {{owner: qa-team, due: \"{due}\"}}\n    compare: exact\n"),
            )
            .replace(
                "    fixtures: [fresh]\n",
                "    manual_checks: [MANUAL-LOOK, MANUAL-COUNT, MANUAL-OLD]\n    fixtures: [fresh]\n",
            )
            + &format!(
                r#"manual_checks:
  MANUAL-LOOK: {{statement: Slides read well, check: Look at 390px, reason: human_judgment}}
  MANUAL-COUNT:
    statement: One run per click
    check: Count runs
    reason: checker_unwritten
    pending_checker: {{owner: qa-team, due: "{due}"}}
    compare: {{banded: {{min_ratio: 0.9}}}}
  MANUAL-OLD: {{statement: Legacy entry, check: Walk it}}
"#
            )
    }

    fn repo(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "-q"]);
        for path in [
            DEFAULT_PATH,
            "pages/[id].vue",
            "server/auth/signup.ts",
            "tests/signup.spec.ts",
        ] {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(
                file,
                if path == DEFAULT_PATH {
                    contents
                } else {
                    "fixture"
                },
            )
            .unwrap();
        }
        git(&["add", "."]);
        dir
    }

    #[test]
    fn qa_registry_fixture_union() {
        let yaml = GABBER.replace(
            "  fresh: {description: New account, provider: account-seed}",
            &format!(r#"  fresh: {{description: New account, provider: account-seed}}
  media: {{description: Frozen media, frozen: {{sha256: "{}", storage_key: golden/video.mp4}}}}
  persona: {{description: Past due account, recipe: {{name: past_due, envs: [staging], provides: [account_id], ttl_mins: 30}}}}
  live: {{description: Live media, provider: "https://example.test/media/1"}}"#, "a".repeat(64)),
        );
        let dir = repo(&yaml);
        let report = check(dir.path(), Path::new(DEFAULT_PATH)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(
            report
                .debt
                .iter()
                .any(|d| d.code == "live_url_fixture" && d.subject == "fixtures.live")
        );
        let registry = load(&dir.path().join(DEFAULT_PATH)).unwrap();
        let roundtrip = serde_yaml::to_string(&registry).unwrap();
        assert!(
            parse(&roundtrip)
                .unwrap()
                .validate(&[])
                .problems
                .iter()
                .all(|p| p.code != "fixture")
        );
        for (from, to) in [
            ("golden/video.mp4", "''"),
            ("ttl_mins: 30", "ttl_mins: 0"),
            ("envs: [staging]", "envs: []"),
            ("provides: [account_id]", "provides: []"),
            ("sha256: \"", "sha256: \"z"),
        ] {
            let invalid = parse(&yaml.replace(from, to)).unwrap().validate(&[]);
            assert!(
                invalid.problems.iter().any(|p| p.code == "fixture"),
                "{invalid:?}"
            );
        }
        let mixed = yaml.replace("provider: account-seed}", "provider: account-seed, recipe: {name: seed, envs: [staging], provides: [id], ttl_mins: 1}}");
        let mut parse_failures = Vec::new();
        let fresh = "{description: New account, provider: account-seed}";
        let mut cases = vec![
            ("empty fixture", GABBER.replace(fresh, "{}"), "fixture", ""),
            (
                "no source",
                GABBER.replace(fresh, "{description: New account}"),
                "fixture",
                "",
            ),
            (
                "source typo",
                GABBER.replace("provider:", "provder:"),
                "unknown_field",
                "did you mean provider",
            ),
            (
                "frozen typo",
                yaml.replace("storage_key:", "storage_ke:"),
                "unknown_field",
                "did you mean storage_key",
            ),
            (
                "recipe typo",
                yaml.replace("ttl_mins:", "ttl_min:"),
                "unknown_field",
                "did you mean ttl_mins",
            ),
            (
                "multiple sources",
                yaml.replace(
                    "provider: account-seed}",
                    "provider: account-seed, frozen: {sha256: bad, storage_key: key}}",
                ),
                "fixture",
                "",
            ),
        ];
        cases.push((
            "consumes an undeclared resource",
            GABBER.replace("    fixtures: [fresh]\n", "    fixtures: [fresh]\n    consumes: {credits: 10}\n"),
            "consumes",
            "unknown resource \"credits\"",
        ));
        cases.push((
            "independence key with a space",
            GABBER.replace(
                "    fixtures: [fresh]\n",
                "    fixtures: [fresh]\n    independence_key: \"post audit id\"\n",
            ),
            "independence_key",
            "post audit id",
        ));
        cases.push((
            "flag with a space",
            GABBER.replace(
                "    fixtures: [fresh]\n",
                "    fixtures: [fresh]\n    depends_on_flags: [\"audio evidence\"]\n",
            ),
            "depends_on_flags",
            "audio evidence",
        ));
        for (name, value) in [
            ("tagged readback extra", "!custom value"),
            ("map readback extra", "{1: value}"),
        ] {
            cases.push((name, GABBER.replace("    fixtures: [fresh]\n", &format!(
                "    fixtures: [fresh]\n    readback: {{subjects: [account_id], invariants: [INV-CLAIM], subjets: {value}}}\n"
            )), "readback", "unknown field"));
        }
        for (name, contents, code, hint) in cases {
            match parse(&contents) {
                Ok(registry) => {
                    let report = registry.validate(&[]);
                    if !report
                        .problems
                        .iter()
                        .any(|p| p.code == code && p.message.contains(hint))
                    {
                        parse_failures.push(format!("{name}: missing {code}/{hint}: {report:?}"));
                    }
                    if report
                        .problems
                        .iter()
                        .any(|p| p.message.contains("did you mean frozen"))
                    {
                        parse_failures.push(format!(
                            "{name}: source collision mislabeled as typo: {report:?}"
                        ));
                    }
                }
                Err(error) => parse_failures.push(format!("{name}: {error:#}")),
            }
        }
        assert!(
            parse_failures.is_empty(),
            "registry parse must survive validation errors:\n{}",
            parse_failures.join("\n")
        );
        assert!(
            parse(&mixed)
                .unwrap()
                .validate(&[])
                .problems
                .iter()
                .any(|p| p.code == "fixture")
        );
    }

    #[test]
    fn qa_registry_tagged_extras() {
        let fresh = "{description: New account, provider: account-seed}";
        let cases = [
            (format!("{GABBER}\nnote: !custom value\n"), "registry"),
            (
                GABBER.replace("    bug: {task: cas-1234}", "    bug: {task: cas-1234}\n    note: !custom value"),
                "invariants.INV-CLAIM",
            ),
            (
                format!("{GABBER}\nmanual_checks:\n  LOOK: {{statement: Looks right, check: Inspect, reason: human_judgment, note: !custom value}}\n"),
                "manual_checks.LOOK",
            ),
            (
                GABBER.replace(fresh, "{description: New account, provider: account-seed, note: !custom value}"),
                "fixtures.fresh",
            ),
            (
                GABBER.replace(fresh, &format!("{{description: Frozen account, frozen: {{sha256: '{}', storage_key: account.json, note: !custom value}}}}", "a".repeat(64))),
                "fixtures.fresh.frozen",
            ),
            (
                GABBER.replace(fresh, "{description: Seed account, recipe: {name: seed, envs: [staging], provides: [account_id], ttl_mins: 1, note: !custom value}}"),
                "fixtures.fresh.recipe",
            ),
            (
                GABBER.replace("    fixtures: [fresh]", "    fixtures: [fresh]\n    note: {1: [!custom value]}"),
                "B2-home",
            ),
        ];
        let mut failures = Vec::new();
        for (contents, subject) in cases {
            // Exercise the same file loader as check/impact/seed consumers.
            let dir = repo(&contents);
            let result = load(&dir.path().join(DEFAULT_PATH)).and_then(|registry| {
                // Unknown metadata must survive seed's serialization path too.
                parse(&serde_yaml::to_string(&registry)?)
            });
            match result {
                Ok(registry) => {
                    let report = registry.validate(&[]);
                    if !report.problems.iter().any(|problem| {
                        problem.code == "unknown_field"
                            && problem.subject == subject
                            && problem.message.contains("unknown field \"note\"")
                    }) {
                        failures.push(format!("{subject}: missing unknown_field: {report:?}"));
                    }
                }
                Err(error) => failures.push(format!("{subject}: {error:#}")),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));

        // Plain metadata stays valid. Duplicate ordinary fields still fail
        // parsing, while repeated sources remain local fixture problems.
        let plain = GABBER
            .replace(
                fresh,
                "{description: New account, provider: account-seed, note: {1: value}}",
            )
            .replace(
                "    fixtures: [fresh]",
                "    fixtures: [fresh]\n    note: [value]",
            )
            + "\nnote: value\n";
        let dir = repo(&plain);
        let report = check(dir.path(), Path::new(DEFAULT_PATH)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        for duplicate in [
            GABBER.replace("version: 1", "version: 1\nversion: 1"),
            GABBER.replace(
                "provider: account-seed}",
                "provider: account-seed, description: Other}",
            ),
            GABBER.replace(
                "provider: account-seed}",
                "provider: account-seed, labels: {}, labels: {}}",
            ),
        ] {
            assert!(parse(&duplicate).is_err(), "duplicate known field accepted");
        }
        let repeated = GABBER.replace(
            "provider: account-seed}",
            "provider: account-seed, provider: other}",
        );
        let dir = repo(&repeated);
        let report = check(dir.path(), Path::new(DEFAULT_PATH))
            .expect("repeated sources must not fail the whole registry parse");
        assert!(
            matches!(report.problems.as_slice(), [problem]
                if problem.code == "fixture" && problem.subject == "fixtures.fresh"
                    && problem.message.contains("exactly one")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn qa_registry_gabber_fixture_validates() {
        let dir = repo(GABBER);
        let key = "qa.registry.path";
        let custom_path = "docs/qa/custom.yaml";
        let mut config = crate::config::Config::default();
        assert_eq!(config.get(key).as_deref(), Some(DEFAULT_PATH));
        assert!(crate::config::meta::registry().get(key).is_some());
        config.set(key, custom_path).unwrap();
        assert!(config.list().contains(&(key.into(), custom_path.into())));
        let cas_dir = dir.path().join(".cas");
        std::fs::create_dir(&cas_dir).unwrap();
        config.save(&cas_dir).unwrap();
        let config = crate::config::Config::load(&cas_dir).unwrap();
        assert_eq!(config.get(key).as_deref(), Some(custom_path));
        std::fs::rename(dir.path().join(DEFAULT_PATH), dir.path().join(custom_path)).unwrap();
        assert!(
            Command::new("git")
                .args(["add", "--all"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let report = check(dir.path(), Path::new(&config.qa().registry.path)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(report.debt.is_empty());
        let registry = load(&dir.path().join(custom_path)).unwrap();
        assert_eq!(
            registry.journeys[0].owners.globs(),
            ["server/auth/**", "pages/[id].vue"]
        );
    }

    #[test]
    fn qa_registry_pending_checkers_and_manual_checks_are_debt_by_reason() {
        let yaml = with_criteria("2999-12-31");
        let registry = parse(&yaml).unwrap();
        let pending = registry.invariants["INV-CLAIM"]
            .pending_checker
            .as_ref()
            .unwrap();
        assert_eq!(
            (pending.owner.as_str(), pending.due.as_str()),
            ("qa-team", "2999-12-31")
        );
        assert_eq!(
            registry.manual_checks["MANUAL-LOOK"].reason.as_deref(),
            Some("human_judgment")
        );
        assert_eq!(
            registry.journeys[0].manual_checks,
            ["MANUAL-LOOK", "MANUAL-COUNT", "MANUAL-OLD"]
        );
        assert_eq!(
            registry.invariants["INV-CLAIM"]
                .compare
                .as_ref()
                .and_then(CompareSpec::valid)
                .copied(),
            Some(Compare::Exact)
        );
        assert_eq!(
            registry.manual_checks["MANUAL-COUNT"]
                .compare
                .as_ref()
                .and_then(CompareSpec::valid)
                .copied(),
            Some(Compare::Banded(Band {
                min_ratio: Some(0.9),
                max_delta: None
            }))
        );
        let report = check(repo(&yaml).path(), Path::new(DEFAULT_PATH)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        let debt: Vec<_> = report
            .debt
            .iter()
            .map(|p| (p.code.as_str(), p.subject.as_str()))
            .collect();
        assert_eq!(
            debt,
            [
                ("pending_checker", "invariants.INV-CLAIM"),
                ("manual_checker_unwritten", "manual_checks.MANUAL-COUNT"),
                ("manual_human_judgment", "manual_checks.MANUAL-LOOK"),
                ("manual_unclassified", "manual_checks.MANUAL-OLD"),
            ]
        );
        assert!(
            report.debt[0]
                .message
                .contains("owner qa-team, due 2999-12-31")
        );

        // Finding 21: a live journey without a spec is hand-walk debt naming
        // its automation owner, or unowned debt.
        let hand = yaml.replace(
            "    spec: tests/signup.spec.ts\n",
            "    automation: {owner: qa-team, task: cas-9999, due: \"2999-12-31\"}\n",
        );
        let owned = check(repo(&hand).path(), Path::new(DEFAULT_PATH)).unwrap();
        assert!(owned.problems.is_empty(), "{:?}", owned.problems);
        let hand_walk = |report: &ValidationReport| -> Vec<String> {
            report
                .debt
                .iter()
                .filter(|d| d.code.starts_with("hand_walk"))
                .map(|d| format!("{} {} {}", d.code, d.subject, d.message))
                .collect()
        };
        assert_eq!(
            hand_walk(&owned),
            ["hand_walk B2-home hand-walked: automation owner qa-team, task cas-9999, due 2999-12-31"]
        );
        // Live only in staging (per-env status, as the gate scopes it) still
        // counts, and a malformed due date names automation.due.
        let unowned = yaml
            .replace("    spec: tests/signup.spec.ts\n", "")
            .replace("    status: live\n", "    status: dark\n    status_reason: Staging pilot\n")
            .replace("prod: {status: live}", "prod: {status: dark}");
        let unowned = check(repo(&unowned).path(), Path::new(DEFAULT_PATH)).unwrap();
        assert_eq!(hand_walk(&unowned)[0].split(' ').take(2).collect::<Vec<_>>(), ["hand_walk_unowned", "B2-home"]);
        let bad_due = hand.replace("cas-9999, due: \"2999-12-31\"", "cas-9999, due: \"soon\"");
        let bad_due = check(repo(&bad_due).path(), Path::new(DEFAULT_PATH)).unwrap();
        assert_eq!(
            bad_due.problems.iter().map(|p| (p.code.as_str(), p.message.as_str())).collect::<Vec<_>>(),
            [("automation", "automation.due must be a YYYY-MM-DD date")]
        );

        // Manual rows carry minutes and the owner; the gate verdict ignores them.
        use crate::qa_release::pack::{HandWalk, Pack, Row};
        let manual = |id: &str, journey: &str, minutes: u32| -> Row {
            serde_json::from_value(serde_json::json!({
                "id": id, "journey": journey, "verdict": {"verdict": "PASS"}, "manual_minutes": minutes
            }))
            .unwrap()
        };
        let mut pack = Pack::new("gabber", &"a".repeat(40), Vec::new());
        pack.rows = vec![manual("home-1", "B2-home", 25), manual("promo", "promo", 30), manual("home-2", "B2-home", 20)];
        pack.hand_walk = Some(HandWalk::measure(&pack.rows, Some(&parse(&hand).unwrap())));
        assert_eq!(
            pack.hand_walk.as_ref().unwrap().summary(3).unwrap(),
            "75 manual minutes over 3 rows; top: `B2-home` 45 min (owner qa-team, task cas-9999, due 2999-12-31), `promo` 30 min (unowned)"
        );
        assert_eq!(pack.derive_gate().verdict, crate::qa_release::verdict::GateVerdict::Clean);
    }

    /// Finding 18: a golden set's tracked TSV or JSONL loads into records;
    /// a malformed set is a problem and an unlabeled record is debt.
    #[test]
    fn qa_registry_golden_set_loader() {
        let yaml = GABBER
            .replace(
                "    check: Count workspaces and read claimed_at\n",
                "    check: Count workspaces and read claimed_at\n    definition: Claimed workspaces\n    labeled: true\n",
            )
            .replace(
                "  fresh: {description: New account, provider: account-seed}\n",
                "  fresh: {description: New account, provider: account-seed, labels: {INV-CLAIM: 1}}\n",
            )
            .replace(
                "    spec: tests/signup.spec.ts\n",
                "    spec: tests/signup.spec.ts\n    fixture_set: tests/golden.tsv\n    matrix: {route: [web, app]}\n    runs: 2\n",
            );
        let sha = "a".repeat(64);
        let golden = |name: &str, contents: &str| {
            let dir = repo(&yaml.replace("tests/golden.tsv", name));
            std::fs::write(dir.path().join(name), contents).unwrap();
            assert!(Command::new("git").args(["add", "."]).current_dir(dir.path()).status().unwrap().success());
            dir
        };
        let tsv = golden(
            "tests/golden.tsv",
            &format!("id\tsha256\tstorage_key\tlabels\nA\t{sha}\tgolden/a.mp4\t{{\"INV-CLAIM\": 3}}\nB\t{sha}\tgolden/b.mp4\t\n"),
        );
        let records = load_fixture_set(tsv.path(), "tests/golden.tsv").unwrap();
        assert_eq!(records.iter().map(|r| (r.id.as_str(), r.labels.len())).collect::<Vec<_>>(), [("A", 1), ("B", 0)]);
        assert_eq!(records[0].frozen.storage_key, "golden/a.mp4");
        let report = check(tsv.path(), Path::new(DEFAULT_PATH)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        let cells: Vec<_> = report.debt.iter().filter(|d| d.code == "unlabeled_cell").map(|d| d.subject.as_str()).collect();
        assert_eq!(cells, ["B2-home.B"]);

        let line = |id: &str| format!(r#"{{"id":"{id}","frozen":{{"sha256":"{sha}","storage_key":"golden/{id}.mp4"}},"labels":{{"INV-CLAIM":1}}}}"#);
        let jsonl = golden("tests/golden.jsonl", &format!("{}\n{}\n", line("A"), line("A")));
        let report = check(jsonl.path(), Path::new(DEFAULT_PATH)).unwrap();
        let problems: Vec<_> = report.problems.iter().map(|p| (p.code.as_str(), p.message.as_str())).collect();
        assert_eq!(problems, [("fixture_set", "tests/golden.jsonl repeats record id A")]);
    }

    #[test]
    fn qa_registry_labeled_criteria_grade_against_fixture_labels() {
        let yaml = GABBER
            .replace(
                "    check: Count workspaces and read claimed_at\n",
                "    check: Count workspaces and read claimed_at\n    definition: An authored text overlay, not text inside a photo\n    labeled: true\n",
            )
            .replace(
                "  fresh: {description: New account, provider: account-seed}\n",
                "  fresh: {description: New account, provider: account-seed, labels: {INV-CLAIM: {authored_text_slides: [2]}}}\n  jersey: {description: Photo carousel, provider: frozen}\n  typo: {description: Stale, provider: frozen, labels: {INV-GONE: 1}}\n",
            )
            .replace("    fixtures: [fresh]\n", "    fixtures: [fresh, jersey]\n");
        let registry = parse(&yaml).unwrap();
        let fresh = &registry.fixtures.as_ref().unwrap()["fresh"];
        assert_eq!(
            serde_yaml::to_string(&fresh.labels["INV-CLAIM"]).unwrap(),
            "authored_text_slides:\n- 2\n"
        );
        let report = check(repo(&yaml).path(), Path::new(DEFAULT_PATH)).unwrap();
        let rows = |rows: &[Problem]| {
            rows.iter()
                .map(|p| format!("{} {} {}", p.code, p.subject, p.message))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(&report.problems),
            ["unknown_label fixtures.typo label for unknown criterion \"INV-GONE\""]
        );
        assert_eq!(
            rows(&report.debt),
            [
                "unlabeled_fixture fixtures.jersey no label for labeled criterion INV-CLAIM (journey B2-home)"
            ]
        );
        let undefined = yaml.replace(
            "    definition: An authored text overlay, not text inside a photo\n",
            "",
        );
        let report = check(repo(&undefined).path(), Path::new(DEFAULT_PATH)).unwrap();
        assert_eq!(report.problems[0].code, "missing_definition");
    }

    #[test]
    fn qa_registry_compare_grades_exact_and_banded() {
        let floor = Compare::Banded(Band {
            min_ratio: Some(0.9),
            max_delta: None,
        });
        let noise = floor.grade(1862.0, 1871.0);
        assert!(noise.pass);
        assert_eq!(format!("{:.2}", noise.pct_of_baseline.unwrap()), "99.52");
        assert!(!floor.grade(1600.0, 1871.0).pass);
        assert!(Compare::Exact.grade(15.0, 15.0).pass);
        let drift = Compare::Exact.grade(1862.0, 1871.0);
        assert!(!drift.pass);
        assert_eq!(drift.pct_of_baseline, noise.pct_of_baseline);
        let delta = Compare::Banded(Band {
            min_ratio: None,
            max_delta: Some(20.0),
        });
        assert!(delta.grade(1853.0, 1871.0).pass);
        assert!(!delta.grade(1891.5, 1871.0).pass);
        assert_eq!(Compare::Exact.grade(0.0, 0.0).pct_of_baseline, None);
    }

    #[test]
    fn qa_registry_invalid_registries_and_seeded_debt() {
        let journey = &GABBER[GABBER.find("  - id: B2-home").unwrap()..];
        let two_journeys = format!("{GABBER}{}", journey.replace("id: B2-home", "id: B3-home"));
        let no_invariants = GABBER.replace(
            "    invariants: [INV-CLAIM]\n    fixtures:",
            "    fixtures:",
        );
        let cases = [
            (
                "orphan glob",
                GABBER.replace("pages/[id].vue", "pages/missing.vue"),
                vec!["orphan_glob"],
                vec![],
            ),
            (
                "unknown invariant",
                GABBER.replace(
                    "    invariants: [INV-CLAIM]\n    fixtures:",
                    "    invariants: [INV-UNKNOWN]\n    fixtures:",
                ),
                vec!["unknown_invariant"],
                vec![],
            ),
            (
                "live invariants",
                no_invariants.clone(),
                vec!["missing_invariants"],
                vec![],
            ),
            (
                "seeded exemption",
                no_invariants
                    .replace(
                        "    steps: [Enter email, Claim workspace]\n",
                        "    seeded: true\n",
                    )
                    .replace(
                        "    traffic: {share_pct: 80, basis: New accounts, source: Analytics}\n",
                        "",
                    ),
                vec![],
                vec!["seeded"],
            ),
            (
                "unmeasured share needs no basis",
                GABBER.replace(
                    "{share_pct: 80, basis: New accounts, source: Analytics}",
                    "{share_pct: null}",
                ),
                vec![],
                vec!["unmeasured_traffic"],
            ),
            (
                "measured share needs a basis",
                GABBER.replace(
                    "{share_pct: 80, basis: New accounts, source: Analytics}",
                    "{share_pct: 80}",
                ),
                vec!["traffic_basis"],
                vec![],
            ),
            (
                "overdue pending checker fails",
                with_criteria("2020-01-01"),
                vec!["overdue_checker", "overdue_checker"],
                vec![
                    "pending_checker",
                    "manual_checker_unwritten",
                    "manual_human_judgment",
                    "manual_unclassified",
                ],
            ),
            (
                "unwritten checker needs an owner and due date",
                with_criteria("2999-12-31")
                    .replace(
                        "    pending_checker: {owner: qa-team, due: \"2999-12-31\"}\n    compare: {banded",
                        "    compare: {banded",
                    )
                    .replace(
                        "MANUAL-OLD: {statement: Legacy entry, check: Walk it}",
                        "MANUAL-OLD: {statement: Legacy entry, check: Walk it, reason: later}",
                    )
                    .replace(
                        "[MANUAL-LOOK, MANUAL-COUNT, MANUAL-OLD]",
                        "[MANUAL-LOOK, MANUAL-GONE]",
                    ),
                vec!["pending_checker", "manual_reason", "unknown_manual_check"],
                vec!["pending_checker", "manual_human_judgment"],
            ),
            (
                "human judgment cannot have a pending checker",
                with_criteria("2999-12-31").replace(
                    "reason: human_judgment}",
                    "reason: human_judgment, pending_checker: {owner: qa, due: \"2999-12-31\"}}",
                ),
                vec!["contradiction"],
                vec![
                    "pending_checker",
                    "manual_checker_unwritten",
                    "manual_unclassified",
                ],
            ),
            (
                "misspelled criterion and fixture fields",
                GABBER
                    .replace(
                        "    check: Count workspaces and read claimed_at\n",
                        "    check: Count workspaces and read claimed_at\n    defintion: Typo\n",
                    )
                    .replace(
                        "provider: account-seed}",
                        "provider: account-seed, lables: {INV-CLAIM: 1}}",
                    ),
                vec!["unknown_field", "unknown_field"],
                vec![],
            ),
            (
                "invalid band",
                GABBER.replace(
                    "    check: Count workspaces and read claimed_at\n",
                    "    check: Count workspaces and read claimed_at\n    compare: {banded: {min_ratio: 1.5}}\n",
                ),
                vec!["compare"],
                vec![],
            ),
            (
                "malformed compare is one problem per criterion, not a load failure",
                with_criteria("2999-12-31")
                    .replace("    compare: exact\n", "    compare: fuzzy\n")
                    .replace("{banded: {min_ratio: 0.9}}", "{banded: {min_rato: 0.9}}"),
                vec!["compare", "compare"],
                vec![
                    "pending_checker",
                    "manual_checker_unwritten",
                    "manual_human_judgment",
                    "manual_unclassified",
                ],
            ),
            (
                "criterion id shared by an invariant and a manual check",
                format!(
                    "{GABBER}manual_checks:\n  INV-CLAIM: {{statement: Dup, check: Dup, reason: human_judgment}}\n"
                ),
                vec!["duplicate_criterion"],
                vec!["manual_human_judgment"],
            ),
            (
                "labeled manual check: null label owes once across journeys",
                two_journeys
                    .replace(
                        "    fixtures: [fresh]\n",
                        "    manual_checks: [MANUAL-SLIDES]\n    fixtures: [fresh]\n",
                    )
                    .replace(
                        "provider: account-seed}",
                        "provider: account-seed, labels: {MANUAL-SLIDES: null, INV-CLAIM: 1}}",
                    )
                    + "manual_checks:\n  MANUAL-SLIDES: {statement: Text slides, check: Count overlays, reason: human_judgment, labeled: true, definition: Authored overlays only}\n",
                vec!["label_without_labeled"],
                vec!["manual_human_judgment", "unlabeled_fixture"],
            ),
        ];
        for (name, yaml, problems, debt) in cases {
            let dir = repo(&yaml);
            let report = check(dir.path(), Path::new(DEFAULT_PATH)).unwrap();
            assert_eq!(
                report
                    .problems
                    .iter()
                    .map(|p| p.code.as_str())
                    .collect::<Vec<_>>(),
                problems,
                "{name}: {:?}",
                report.problems
            );
            assert_eq!(
                report
                    .debt
                    .iter()
                    .map(|p| p.code.as_str())
                    .collect::<Vec<_>>(),
                debt,
                "{name}"
            );
            assert!(report.problems.iter().chain(&report.debt).all(|p| {
                p.subject == "B2-home"
                    || ["invariants.", "manual_checks.", "fixtures."]
                        .iter()
                        .any(|prefix| p.subject.starts_with(prefix))
            }));
            if name.starts_with("malformed compare") {
                let rows: Vec<_> = report
                    .problems
                    .iter()
                    .map(|p| {
                        (
                            p.subject.as_str(),
                            p.message.contains("fuzzy") || p.message.contains("min_rato"),
                        )
                    })
                    .collect();
                assert_eq!(
                    rows,
                    [
                        ("invariants.INV-CLAIM", true),
                        ("manual_checks.MANUAL-COUNT", true)
                    ]
                );
            }
            if let Some(owed) = report.debt.iter().find(|p| p.code == "unlabeled_fixture") {
                assert_eq!(
                    owed.message,
                    "no label for labeled criterion MANUAL-SLIDES (journey B2-home)"
                );
            }
        }
    }

    #[test]
    fn qa_registry_bracket_literal_path_filters() {
        assert!(matches_path(["pages/[id].vue"], "pages/[id].vue"));
        assert!(!matches_path(["pages/[id].vue"], "pages/i.vue"));
        assert!(!matches_path(["pages/*.vue"], "pages/nested/index.vue"));
        assert!(matches_path(["pages/**/*.vue"], "pages/[id].vue"));
        assert!(matches_path(["pages/**/*.vue"], "pages/nested/index.vue"));
        let filters = ["pages/**", "!pages/private/**", "pages/private/public.vue"];
        assert!(!matches_path(filters, "pages/private/account.vue"));
        assert!(matches_path(filters, "pages/private/public.vue"));
    }
}
