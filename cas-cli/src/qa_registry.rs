//! Journey registry v1: YAML loading, validation and shared path-filter semantics.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

pub const DEFAULT_PATH: &str = "docs/qa/journeys.yaml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub version: Option<u32>,
    pub updated: Option<String>,
    pub seeded_from: Vec<String>,
    pub traffic_snapshot: Option<Mapping>,
    pub ignore: Vec<String>,
    /// None inherits qa.user_facing_paths + qa.api_paths; [] selects no surfaces.
    pub surfaces: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub owner_groups: BTreeMap<String, OwnerGroup>,
    pub invariants: BTreeMap<String, Invariant>,
    pub fixtures: Option<BTreeMap<String, Fixture>>,
    pub bug_families: BTreeMap<String, BugFamily>,
    pub journeys: Vec<Journey>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OwnerGroup {
    pub description: String,
    pub globs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Invariant {
    pub statement: String,
    pub check: String,
    pub bug: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fixture {
    pub description: String,
    pub provider: String,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
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
    pub fixtures: Vec<String>,
    pub traffic: Option<Traffic>,
    pub spec: Option<String>,
    pub known_failure: Option<String>,
    pub finisher: Option<String>,
    pub evidence: Option<Mapping>,
    pub seeded: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

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
    Ok(registry.validate(
        &paths
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    ))
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

fn valid_status(status: &str) -> bool {
    matches!(status, "live" | "dark" | "dead")
}

fn present(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.trim().is_empty())
}

impl Registry {
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

    /// Validate in memory against the caller's tracked-file snapshot (no subprocess).
    pub fn validate(&self, tracked: &[String]) -> ValidationReport {
        use std::collections::BTreeSet;
        let mut report = ValidationReport::default();
        if self.version != Some(1) {
            report.problem("version", "registry", "version must be 1");
        }
        let date_valid = self.updated.as_deref().is_some_and(|date| {
            date.len() == 10
                && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .is_ok_and(|parsed| parsed.format("%Y-%m-%d").to_string() == date)
        });
        if !date_valid {
            report.problem(
                "updated",
                "registry",
                "updated must be an ISO date (YYYY-MM-DD)",
            );
        }
        if self.journeys.is_empty() {
            report.problem("journeys", "registry", "journeys must be a non-empty list");
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
            if invariant.statement.trim().is_empty() || invariant.check.trim().is_empty() {
                report.problem(
                    "invariant",
                    &format!("invariants.{name}"),
                    "statement and check are required",
                );
            }
        }
        if let Some(fixtures) = &self.fixtures {
            for (name, fixture) in fixtures {
                if fixture.description.trim().is_empty() || fixture.provider.trim().is_empty() {
                    report.problem(
                        "fixture",
                        &format!("fixtures.{name}"),
                        "description and provider are required",
                    );
                }
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
            for invariant in &journey.invariants {
                if !self.invariants.contains_key(invariant) {
                    report.problem(
                        "unknown_invariant",
                        id,
                        format!("unknown invariant {invariant:?}"),
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
            if let Some(traffic) = &journey.traffic {
                if traffic.basis.trim().is_empty() {
                    report.problem(
                        "traffic_basis",
                        id,
                        "traffic.basis must say what the share counts",
                    );
                }
            }
        }
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
    fn qa_registry_invalid_registries_and_seeded_debt() {
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
            assert!(
                report
                    .problems
                    .iter()
                    .chain(&report.debt)
                    .all(|p| p.subject == "B2-home")
            );
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
