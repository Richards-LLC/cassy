//! Golden-set matrix (finding 18). A journey with a `fixture_set`, a `matrix`
//! or `runs` above 1 expands into cells: every fixture record crossed with
//! one value of every axis. Each cell is walked `runs` times, one row per
//! run, so the gate owes cells × runs rows. Within a cell, runs count only
//! when their `independence_key` identities differ (cas-d863): a duplicate
//! or unidentified run is rejected and the cell stays OWED.
use super::independence::Runs;
use super::pack::Row;
use super::verdict::{NotExercisedCause, RowVerdict};
use crate::qa_registry::{Journey, SetRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One golden-set cell: a fixture record (when the journey has a set) and one
/// value of every matrix axis.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// `<record>.<value>.<value>`, in axis order; empty for a runs-only journey.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<SetRecord>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, String>,
}

impl Cell {
    /// `fixture=<id> <axis>=<value> …`, the row's `cell`.
    pub fn label(&self) -> String {
        self.record
            .iter()
            .map(|r| format!("fixture={}", r.id))
            .chain(self.axes.iter().map(|(axis, value)| format!("{axis}={value}")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A journey's cells and the runs each needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Expansion {
    pub cells: Vec<Cell>,
    pub runs: u32,
}

impl Expansion {
    pub fn rows(&self) -> u64 {
        self.cells.len() as u64 * u64::from(self.runs)
    }
}

/// Expands a golden-set journey, refusing one whose cells × runs exceeds
/// `max_cells` before anything is walked.
pub fn expand(project_root: &Path, journey: &Journey, max_cells: u32) -> Result<Expansion, String> {
    let records: Vec<Option<SetRecord>> = match &journey.fixture_set {
        Some(path) => crate::qa_registry::load_fixture_set(project_root, path)
            .map_err(|problems| format!("{}: {}", journey.id, problems.join("; ")))?
            .into_iter()
            .map(Some)
            .collect(),
        None => vec![None],
    };
    let mut cells: Vec<Cell> = records
        .into_iter()
        .map(|record| Cell { id: record.as_ref().map(|r| r.id.clone()).unwrap_or_default(), record, axes: BTreeMap::new() })
        .collect();
    for (axis, values) in &journey.matrix {
        cells = cells
            .iter()
            .flat_map(|cell| {
                values.iter().map(move |value| {
                    let mut next = cell.clone();
                    next.id = if next.id.is_empty() { value.clone() } else { format!("{}.{value}", next.id) };
                    next.axes.insert(axis.clone(), value.clone());
                    next
                })
            })
            .collect();
    }
    let expansion = Expansion { cells, runs: journey.runs.unwrap_or(1).max(1) };
    if expansion.rows() > u64::from(max_cells) {
        return Err(format!(
            "{} expands to {} cells × {} runs = {} rows, over qa.release.max_cells {max_cells}; narrow the matrix or raise the limit",
            journey.id,
            expansion.cells.len(),
            expansion.runs,
            expansion.rows()
        ));
    }
    Ok(expansion)
}

/// `<journey>.<cell>.r<run>` (`<journey>.r<run>` for a runs-only journey).
pub fn row_id(journey: &str, cell: &Cell, run: u32) -> String {
    if cell.id.is_empty() { format!("{journey}.r{run}") } else { format!("{journey}.{}.r{run}", cell.id) }
}

/// One golden-set journey's coverage in the pack: cells × runs required, and
/// the run rows that count (walked to PASS or FAIL as an independent run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub journey: String,
    pub cells: u32,
    pub runs: u32,
    pub required: u32,
    pub counted: u32,
    /// Rows that do not count yet, owed with their row verdicts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

impl Coverage {
    pub fn line(&self) -> String {
        format!(
            "`{}`: {}/{} runs ({} cells × {} runs){}",
            super::printable(&self.journey),
            self.counted,
            self.required,
            self.cells,
            self.runs,
            if self.missing.is_empty() { String::new() } else { format!("; owed: {}", self.missing.len()) }
        )
    }
}

/// One golden journey's planned rows: `(row id, cell label)`.
pub struct Planned<'a> {
    pub journey: &'a Journey,
    pub rows: Vec<(String, String)>,
}

/// Rejects duplicate and unidentified runs within each cell of a journey with
/// an `independence_key` (the later duplicate becomes NOT_EXERCISED, so the
/// cell stays OWED), then reports cells × runs coverage per golden journey
/// against its planned rows; a planned row with no verdict is missing.
pub fn settle(rows: &mut [Row], golden: &[Planned<'_>]) -> Vec<Coverage> {
    let mut coverage = Vec::new();
    for Planned { journey, rows: planned } in golden {
        let runs = journey.runs.unwrap_or(1).max(1);
        if let Some(key) = &journey.independence_key {
            let mut by_cell: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for (index, row) in rows.iter().enumerate().filter(|(_, r)| r.journey == journey.id) {
                by_cell.entry(row.cell.clone().unwrap_or_default()).or_default().push(index);
            }
            for (cell, mut indexes) in by_cell {
                indexes.sort_by_key(|&i| rows[i].run);
                let walked = |row: &Row| matches!(row.verdict, RowVerdict::Pass | RowVerdict::Fail { .. });
                let counted = Runs::count(
                    indexes.iter().filter(|&&i| walked(&rows[i])).map(|&i| rows[i].identity.as_deref()),
                );
                let Some(refusal) = counted.refusal(&format!("{} [{cell}]", journey.id), key, runs as usize) else {
                    continue;
                };
                let mut seen = std::collections::BTreeSet::new();
                for &index in &indexes {
                    let row = &mut rows[index];
                    if !walked(row) {
                        continue;
                    }
                    let fresh = row.identity.as_deref().is_some_and(|id| seen.insert(id.to_owned()));
                    if !fresh {
                        row.verdict = RowVerdict::NotExercised {
                            cause: NotExercisedCause::Automation,
                            detail: format!("run rejected: {refusal}"),
                        };
                    }
                }
            }
        }
        let counts = |id: &str| {
            rows.iter().any(|r| r.id == id && matches!(r.verdict, RowVerdict::Pass | RowVerdict::Fail { .. }))
        };
        let cells = planned.iter().map(|(_, cell)| cell).collect::<std::collections::BTreeSet<_>>().len();
        let missing: Vec<String> = planned.iter().filter(|(id, _)| !counts(id)).map(|(id, _)| id.clone()).collect();
        coverage.push(Coverage {
            journey: journey.id.clone(),
            cells: cells as u32,
            runs,
            required: planned.len() as u32,
            counted: (planned.len() - missing.len()) as u32,
            missing,
        });
    }
    coverage
}

/// Grades the adapter's `measured` values against the cell's labels with each
/// criterion's `compare` (cas-aff3). A band miss is FAIL naming the % of the
/// baseline; an unlabeled criterion is registry debt, not graded here.
pub fn grade_measured(
    registry: &crate::qa_registry::Registry,
    cell: &Cell,
    measured: &BTreeMap<String, f64>,
) -> Option<RowVerdict> {
    use crate::qa_registry::Compare;
    let labels = &cell.record.as_ref()?.labels;
    let mut misses = Vec::new();
    for (criterion, value) in measured {
        let Some(baseline) = labels.get(criterion).and_then(serde_yaml::Value::as_f64) else { continue };
        let compare = registry
            .invariants
            .get(criterion)
            .and_then(|c| c.compare.as_ref())
            .or_else(|| registry.manual_checks.get(criterion).and_then(|c| c.compare.as_ref()))
            .and_then(|c| c.valid().copied())
            .unwrap_or(Compare::Exact);
        let grade = compare.grade(*value, baseline);
        if !grade.pass {
            let pct = grade.pct_of_baseline.map_or("n/a".into(), |pct| format!("{pct:.1}%"));
            misses.push(format!("{criterion} measured {value} is {pct} of baseline {baseline}"));
        }
    }
    (!misses.is_empty()).then(|| RowVerdict::Fail { reason: format!("band miss: {}", misses.join("; ")) })
}
