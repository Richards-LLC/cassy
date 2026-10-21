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
#[serde(default)]
pub struct Traffic {
    pub share_pct: Option<f64>,
    pub basis: String,
    pub source: Option<String>,
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

/// Ordered path filters: the last matching positive/negative entry wins.
pub fn matches_path<'a>(globs: impl IntoIterator<Item = &'a str>, path: &str) -> bool {
    let _ = (globs, path);
    false
}

impl Registry {
    pub fn validate(&self, _tracked: &[String]) -> ValidationReport {
        ValidationReport::default()
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
    fixed_by: [abc123]
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
        let report = check(dir.path(), Path::new(DEFAULT_PATH)).unwrap();
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(report.debt.is_empty());
        let registry = load(&dir.path().join(DEFAULT_PATH)).unwrap();
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
