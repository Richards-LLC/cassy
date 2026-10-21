//! Versioned journey impact reports. Registry matching itself has no I/O.

use crate::{
    config::QaConfig,
    qa_registry::{Entry, EnvironmentStatus, Problem},
};
use anyhow::Result;
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

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

pub fn analyze(
    repo: &Path,
    qa: &QaConfig,
    changes: &[Change],
    base: Option<&str>,
    head: Option<&str>,
) -> Result<Impact> {
    let _ = (repo, changes, base, head);
    Ok(Impact {
        version: 1,
        source: "codemap_fallback".into(),
        registry: RegistryInfo {
            path: qa.registry.path.clone(),
            present: false,
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
    })
}

#[cfg(test)]
#[path = "qa_impact_tests.rs"]
mod tests;
