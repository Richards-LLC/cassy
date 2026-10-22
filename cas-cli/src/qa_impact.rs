//! Versioned journey impact reports. Registry matching itself has no I/O.

use crate::{
    config::QaConfig,
    qa_registry::{Entry, EnvironmentStatus, Problem},
};
use anyhow::Result;
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

/// Read failures have a distinct CLI exit code without bypassing normal teardown.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ReadError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

#[derive(Debug, Clone)]
pub struct Change {
    pub status: String,
    pub file: String,
    pub old_file: Option<String>,
}

impl Change {
    pub fn modified(file: impl Into<String>) -> Self {
        Self {
            status: "M".into(),
            file: file.into(),
            old_file: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Impact {
    pub version: u32,
    pub source: String,
    pub registry: RegistryInfo,
    pub journeys: Vec<JourneyImpact>,
    pub areas: Vec<Area>,
    pub unowned: Vec<Unowned>,
    pub ignored: Vec<String>,
    pub upkeep: Upkeep,
    pub warnings: Vec<String>,
    /// GH #1346: the registry's QA safety policy, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<crate::qa_registry::Safety>,
}

#[derive(Debug, Serialize)]
pub struct RegistryInfo {
    pub path: String,
    pub present: bool,
    pub valid: bool,
    pub errors_from_diff: Vec<Problem>,
    pub debt_count: usize,
}

#[derive(Debug, Serialize)]
pub struct JourneyImpact {
    pub id: String,
    pub name: String,
    pub status: String,
    pub kind: Option<String>,
    pub share_pct: Option<f64>,
    pub basis: Option<String>,
    pub entry: Option<Entry>,
    pub env_status: BTreeMap<String, EnvironmentStatus>,
    pub invariants: Vec<InvariantImpact>,
    pub fixtures: Vec<String>,
    pub spec: Option<String>,
    pub known_failure: Option<String>,
    pub matches: Vec<Match>,
    pub evidence_required: String,
}

#[derive(Debug, Serialize)]
pub struct InvariantImpact {
    pub id: String,
    pub statement: String,
    pub check: String,
}

#[derive(Debug, Serialize)]
pub struct Match {
    pub file: String,
    pub glob: String,
    pub via: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodemapLabel {
    pub prefix: String,
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct Area {
    pub prefix: String,
    pub summary: String,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Unowned {
    pub file: String,
    pub surface: bool,
    pub codemap: Option<CodemapLabel>,
}

#[derive(Debug, Default, Serialize)]
pub struct Upkeep {
    pub codemap: UpkeepItem,
    pub product_overview: UpkeepItem,
    pub registry: UpkeepItem,
}

#[derive(Debug, Default, Serialize)]
pub struct UpkeepItem {
    pub required: bool,
    pub reasons: Vec<String>,
}

#[path = "qa_impact_io.rs"]
pub(crate) mod io;

pub fn analyze(
    repo: &Path,
    qa: &QaConfig,
    changes: &[Change],
    base: Option<&str>,
    head: Option<&str>,
) -> Result<Impact> {
    io::analyze(repo, qa, changes, base, head)
}

/// Pure registry/glob pass. Filesystem and legacy-selector adapters enrich this report.
pub fn impact_for_changes(
    registry: Option<&crate::qa_registry::Registry>,
    qa: &QaConfig,
    changes: &[Change],
    codemap: &str,
) -> Impact {
    impact_for_selected_changes(registry, qa, changes, codemap, None)
}

/// Selection never grants ownership. Some(empty) authoritatively selects none.
fn impact_for_selected_changes(
    registry: Option<&crate::qa_registry::Registry>,
    qa: &QaConfig,
    changes: &[Change],
    codemap: &str,
    selected: Option<&std::collections::BTreeSet<String>>,
) -> Impact {
    use crate::qa_registry::matches_path;
    use std::collections::{BTreeMap, BTreeSet};
    let mut report = Impact {
        version: 1,
        source: if registry.is_some() {
            "registry"
        } else {
            "codemap_fallback"
        }
        .into(),
        registry: RegistryInfo {
            path: qa.registry.path.clone(),
            present: registry.is_some(),
            valid: false,
            errors_from_diff: vec![],
            debt_count: 0,
        },
        journeys: vec![],
        areas: vec![],
        unowned: vec![],
        ignored: vec![],
        upkeep: Upkeep::default(),
        warnings: vec![],
        safety: registry.and_then(|r| r.safety.clone()),
    };
    let labels = codemap_labels(codemap);
    let files: BTreeSet<&str> = changes
        .iter()
        .flat_map(|change| std::iter::once(change.file.as_str()).chain(change.old_file.as_deref()))
        .collect();
    let mut remaining = Vec::new();
    for file in files {
        if registry.is_some_and(|r| matches_path(r.ignore.iter().map(String::as_str), file)) {
            report.ignored.push(file.to_owned());
        } else {
            remaining.push(file);
        }
    }
    let mut owned = BTreeSet::new();
    if let Some(registry) = registry {
        for journey in &registry.journeys {
            let mut patterns: Vec<(&str, String)> = journey
                .owners
                .globs()
                .into_iter()
                .map(|glob| (glob, "owners".into()))
                .collect();
            for name in &journey.owner_groups {
                if let Some(group) = registry.owner_groups.get(name) {
                    patterns.extend(
                        group
                            .globs
                            .iter()
                            .map(|glob| (glob.as_str(), format!("owner_groups.{name}"))),
                    );
                }
            }
            let mut matches = Vec::new();
            for file in &remaining {
                if !matches_path(patterns.iter().map(|(glob, _)| *glob), file) {
                    continue;
                }
                owned.insert(*file);
                for (glob, via) in &patterns {
                    if !glob.starts_with('!') && matches_path([*glob], file) {
                        matches.push(Match {
                            file: (*file).into(),
                            glob: (*glob).into(),
                            via: via.clone(),
                        });
                    }
                }
            }
            if selected.map_or(matches.is_empty(), |ids| !ids.contains(&journey.id)) {
                continue;
            }
            let invariants = journey
                .invariants
                .iter()
                .filter_map(|id| {
                    registry.invariants.get(id).map(|inv| InvariantImpact {
                        id: id.clone(),
                        statement: inv.statement.clone(),
                        check: inv.check.clone(),
                    })
                })
                .collect();
            report.journeys.push(JourneyImpact {
                id: journey.id.clone(),
                name: journey.name.clone().unwrap_or_else(|| journey.id.clone()),
                status: journey.status.clone(),
                kind: journey.kind.clone(),
                share_pct: journey
                    .traffic
                    .as_ref()
                    .and_then(|t| t.share_pct)
                    .filter(|n| n.is_finite()),
                basis: journey.traffic.as_ref().map(|t| t.basis.clone()),
                entry: journey.entry.clone(),
                env_status: journey.env_status.clone(),
                invariants,
                fixtures: journey.fixtures.clone(),
                // GH #1344: a planned spec is not a file a reviewer can open.
                spec: journey
                    .spec
                    .clone()
                    .filter(|_| journey.spec_status.as_deref() != Some("planned")),
                known_failure: journey.known_failure.clone(),
                matches,
                evidence_required: evidence_required(journey, report.safety.as_ref()),
            });
        }
        let seeded = registry.journeys.iter().filter(|j| j.seeded).count();
        if seeded > 0 {
            report.warnings.push(format!(
                "Registry has {seeded} seeded (unreviewed) journeys."
            ));
        }
    } else {
        report.warnings.push(
            "No journey registry: impact is by CODEMAP area only. Run cas qa journeys --seed."
                .into(),
        );
    }
    let surfaces: Vec<&str> = registry
        .map(|r| r.effective_surfaces(qa))
        .unwrap_or_else(|| {
            qa.user_facing_paths
                .iter()
                .chain(&qa.api_paths)
                .map(String::as_str)
                .collect()
        });
    let mut areas: BTreeMap<String, Area> = BTreeMap::new();
    for file in remaining {
        if owned.contains(file) {
            continue;
        }
        let label = labels
            .iter()
            .filter(|label| contains_path(&label.prefix, file))
            .max_by_key(|label| label.prefix.len())
            .cloned();
        if registry.is_none() {
            let label = label.clone().unwrap_or(CodemapLabel {
                prefix: String::new(),
                summary: "Unmapped files".into(),
            });
            areas
                .entry(label.prefix.clone())
                .or_insert_with(|| Area {
                    prefix: label.prefix,
                    summary: label.summary,
                    files: vec![],
                })
                .files
                .push(file.into());
        }
        report.unowned.push(Unowned {
            file: file.into(),
            surface: matches_path(surfaces.iter().copied(), file),
            codemap: label,
        });
    }
    if registry.is_none() {
        report.areas = areas.into_values().collect();
        report.areas.sort_by(|a, b| {
            b.files
                .len()
                .cmp(&a.files.len())
                .then_with(|| a.prefix.cmp(&b.prefix))
        });
    }
    sort_journeys(&mut report.journeys);
    report
}

fn sort_journeys(journeys: &mut [JourneyImpact]) {
    let rank = |status: &str| match status {
        "live" => 0,
        "dark" => 1,
        "dead" => 2,
        _ => 3,
    };
    journeys.sort_by(|a, b| {
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then_with(|| match (a.share_pct, b.share_pct) {
                (Some(a), Some(b)) => b.total_cmp(&a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn evidence_required(
    journey: &crate::qa_registry::Journey,
    safety: Option<&crate::qa_registry::Safety>,
) -> String {
    let evidence = walk_evidence(journey, safety);
    match (journey.spec_status.as_deref(), &journey.spec) {
        (Some("planned"), Some(spec)) if journey.status != "dead" => {
            format!("{evidence}; spec {spec} is planned, not written: no automated coverage yet")
        }
        _ => evidence,
    }
}

fn walk_evidence(
    journey: &crate::qa_registry::Journey,
    safety: Option<&crate::qa_registry::Safety>,
) -> String {
    let entry = journey
        .entry
        .as_ref()
        .map(|e| e.url.as_str())
        .unwrap_or("<entry not declared>");
    let invariants = if journey.invariants.is_empty() {
        "<no invariants declared>".into()
    } else {
        journey.invariants.join(", ")
    };
    let fixtures = if journey.fixtures.is_empty() {
        "none declared".into()
    } else {
        journey.fixtures.join(", ")
    };
    match (journey.status.as_str(), safety) {
        ("dark", _) => "prove it stays unreachable".into(),
        ("dead", _) => "none (NOT A ROUTE)".into(),
        (_, Some(safety)) if safety.fixture_only() => format!(
            "walk {entry} against stubs; prove zero non-loopback requests; check {invariants} on the stubbed responses; fixtures: {fixtures}"
        ),
        (_, Some(safety)) if safety.mode == "production-read-back" => format!(
            "walk {entry} on production with a fresh persona; read back {invariants} from the system of record without writing; fixtures: {fixtures}"
        ),
        _ => format!(
            "walk {entry} on staging with a fresh persona; read back {invariants} from the system of record; fixtures: {fixtures}"
        ),
    }
}

fn contains_path(prefix: &str, file: &str) -> bool {
    file == prefix
        || file
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Read the literal path entries in both common CODEMAP bullet formats.
pub fn codemap_labels(text: &str) -> Vec<CodemapLabel> {
    let mut labels = BTreeMap::new();
    for line in text.lines() {
        let Some((paths, summary)) = line.split_once(" — ").or_else(|| line.split_once(" - "))
        else {
            continue;
        };
        let quoted: Vec<&str> = paths
            .split('`')
            .enumerate()
            .filter_map(|(i, part)| (i % 2 == 1).then_some(part))
            .collect();
        let paths = if quoted.is_empty() {
            vec![paths.trim().trim_start_matches(['-', '*']).trim()]
        } else {
            quoted
        };
        for path in paths {
            let prefix = path.trim_start_matches("./").trim_end_matches('/');
            if prefix.is_empty() || prefix.contains(['*', '?']) || prefix.contains(' ') {
                continue;
            }
            labels.insert(prefix.to_owned(), summary.trim().to_owned());
        }
    }
    labels
        .into_iter()
        .map(|(prefix, summary)| CodemapLabel { prefix, summary })
        .collect()
}

#[cfg(test)]
#[path = "qa_impact_tests.rs"]
mod tests;
