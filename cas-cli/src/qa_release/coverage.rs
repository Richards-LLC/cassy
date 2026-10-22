//! Per-PR coverage (finding 25): a green gate says nothing about a fix no row
//! walked. Computed once at preflight from the candidate range; a PR with no
//! exercising row and no recorded manual row keeps the gate OWED.
use super::pack::Row;
use super::verdict::Owed;
use crate::config::QaConfig;
use crate::qa_impact::{self, Change};
use crate::qa_registry::Registry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// One PR merged into the candidate range and the paths it changed.
#[derive(Debug, Clone)]
pub struct PrChange {
    pub number: u64,
    pub commit: String,
    pub changes: Vec<Change>,
}

/// Which gate rows exercise one PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCoverage {
    pub pr: u64,
    pub commit: String,
    /// Journeys whose owners match the PR's paths (`cas qa impact`).
    pub journeys: Vec<String>,
    /// Rows of those journeys.
    pub rows: Vec<String>,
    /// A manual row recorded as covering this PR (`covers_prs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_row: Option<String>,
}

impl PrCoverage {
    pub fn exercised(&self) -> bool {
        !self.rows.is_empty() || self.manual_row.is_some()
    }

    /// What the gate owes for an unexercised PR.
    pub fn owed(&self) -> Option<Owed> {
        (!self.exercised()).then(|| Owed {
            subject: format!("PR #{}", self.pr),
            cause: "no gate row exercises its paths; walk one or record a manual row covering it"
                .into(),
        })
    }
}

/// The PR number a first-parent commit subject names: GitHub's
/// `Merge pull request #N from …` or a squash subject ending in `(#N)`.
fn pr_number(subject: &str) -> Option<u64> {
    let digits = |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten();
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        return digits(rest.split_whitespace().next()?);
    }
    digits(subject.trim_end().strip_suffix(')')?.rsplit_once("(#")?.1)
}

/// The PRs on `head`'s first-parent history since `base`, each with its diff
/// against its first parent. Local git only.
pub fn candidate_prs(repo: &Path, base: &str, head: &str) -> anyhow::Result<Vec<PrChange>> {
    let git = qa_impact::io::git;
    let log = git(
        repo,
        &["log", "--first-parent", "--format=%H%x00%P%x00%s", &format!("{base}..{head}"), "--"],
    )?;
    let mut prs = Vec::new();
    for line in log.lines() {
        let mut fields = line.splitn(3, '\0');
        let (Some(commit), Some(parents), Some(subject)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let (Some(number), Some(parent)) = (pr_number(subject), parents.split_whitespace().next()) else {
            continue;
        };
        prs.push(PrChange {
            number,
            commit: commit.into(),
            changes: qa_impact::io::changes_for_diff(repo, parent, Some(commit))?,
        });
    }
    prs.reverse();
    Ok(prs)
}

/// Maps each PR to the rows that exercise it, via the registry's owner globs.
pub fn coverage(prs: &[PrChange], registry: Option<&Registry>, qa: &QaConfig, rows: &[Row]) -> Vec<PrCoverage> {
    prs.iter()
        .map(|pr| {
            let journeys: BTreeSet<String> = qa_impact::impact_for_changes(registry, qa, &pr.changes, "")
                .journeys
                .into_iter()
                .map(|journey| journey.id)
                .collect();
            PrCoverage {
                pr: pr.number,
                commit: pr.commit.clone(),
                rows: rows
                    .iter()
                    .filter(|row| journeys.contains(&row.journey))
                    .map(|row| row.id.clone())
                    .collect(),
                manual_row: rows
                    .iter()
                    .find(|row| row.covers_prs.contains(&pr.number))
                    .map(|row| row.id.clone()),
                journeys: journeys.into_iter().collect(),
            }
        })
        .collect()
}
