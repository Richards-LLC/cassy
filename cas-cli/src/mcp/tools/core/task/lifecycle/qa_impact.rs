//! Automatic predicted/delivered journey context and the G8 ledger check.
use super::close_ops;
use crate::mcp::tools::core::imports::*;
use crate::qa_impact::{self, Change};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::path::Path;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn cell(value: &str) -> String {
    value.replace(['\n', '\r'], " ").replace('|', "\\|")
}

/// Shared by the task note and independent review brief.
pub(crate) fn render(impact: &Value) -> String {
    let mut out = String::new();
    if let Some(warnings) = impact["warnings"].as_array() {
        for warning in warnings {
            out.push_str(&format!("> WARNING: {}\n", cell(text(warning))));
        }
    }
    out.push_str("\n| Journey | Status | Share | Invariants and evidence required |\n| --- | --- | --- | --- |\n");
    if let Some(journeys) = impact["journeys"].as_array() {
        for journey in journeys {
            let invariants = journey["invariants"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|i| {
                            format!(
                                "{}: {} — {}",
                                text(&i["id"]),
                                text(&i["statement"]),
                                text(&i["check"])
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            let share = journey["share_pct"]
                .as_f64()
                .map(|n| format!("{n}%"))
                .unwrap_or_else(|| "unknown".into());
            out.push_str(&format!(
                "| {} | {} | {} | {} {} |\n",
                cell(text(&journey["id"])),
                cell(text(&journey["status"])),
                share,
                cell(&invariants),
                cell(text(&journey["evidence_required"]))
            ));
        }
    }
    if let Some(areas) = impact["areas"].as_array() {
        for area in areas {
            out.push_str(&format!(
                "\n- Walk the user-facing entry of CODEMAP area `{}`: {}\n",
                cell(text(&area["prefix"])),
                cell(text(&area["summary"]))
            ));
        }
    }
    if let Some(unowned) = impact["unowned"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        out.push_str(&format!(
            "\n{} unowned file(s); see state_patch.qa_impact for the full impact.\n",
            unowned.len()
        ));
    }
    out.push_str("\nRecord one ledger row per live journey: PASS, FAIL, NOT A ROUTE, or NOT EXERCISED: <reason>.\n");
    out
}

pub(crate) fn predicted(core: &CasCore, task: &Task) -> String {
    let result = (|| -> Result<Value> {
        let repo = if let Some(target) = task.deliverables.work_target.as_ref() {
            super::super::repo_context::resolve_repo_context(&core.cas_root, target)
                .map_err(|e| anyhow!(e))?
                .repo_root
        } else {
            close_ops::resolve_close_gate_repo_root(&core.cas_root).map_err(|e| anyhow!(e))?
        };
        let tracked = qa_impact::io::git(&repo, &["ls-files", "-z", "--cached", "--full-name"])?;
        let content = format!(
            "{}\n{}\n{}\n{}",
            task.title,
            task.description,
            task.design,
            task.proof_targets.join("\n")
        );
        let mut paths = std::collections::BTreeSet::new();
        for word in content.split_whitespace() {
            let path =
                word.trim_matches(|c: char| matches!(c, '`' | '\'' | '"' | '(' | ')' | ',' | ';'));
            let path = path
                .split(':')
                .next()
                .unwrap_or(path)
                .trim_start_matches("./");
            if !path.contains('/') && !path.contains('.') {
                continue;
            }
            for file in tracked.split('\0').filter(|file| !file.is_empty()) {
                if file == path
                    || file.starts_with(&format!("{}/", path.trim_end_matches('/')))
                    || crate::qa_registry::matches_path([path], file)
                {
                    paths.insert(file.to_string());
                }
            }
        }
        let changes: Vec<_> = paths.into_iter().map(Change::modified).collect();
        let report = qa_impact::analyze(&repo, &core.load_config().qa(), &changes, None, None)?;
        Ok(serde_json::to_value(report)?)
    })();
    match result {
        Ok(report) => format!(
            "Predicted QA impact (paths named in title, description, design and proof targets; not a delivery diff):\n{}",
            render(&report)
        ),
        Err(error) => format!("Predicted QA impact unavailable (non-blocking): {error:#}"),
    }
}

impl CasCore {
    /// Cache the committed delivery using close's task attribution. A lane's
    /// later work and the caller's working-tree metadata are never selected.
    pub(crate) fn attach_delivery_qa_impact(
        &self,
        task: &Task,
        repo: &Path,
        target: &str,
        head: &str,
    ) -> Result<Value> {
        let store = self
            .open_task_store()
            .map_err(|e| anyhow!(e.message.to_string()))?;
        if let Some(state) = store.get_execution_state(&task.id)?
            && let Some(impact) = state.get("qa_impact")
            && impact["head_sha"].as_str() == Some(head)
        {
            return Ok(impact.clone());
        }
        let latest = cas_store::get_latest_worker_delivery(&self.cas_root, &task.id)?
            .map(|(r, _)| r.commit_sha);
        let history = self
            .open_agent_store()
            .ok()
            .and_then(|s| s.get_lease_history(&task.id, None).ok())
            .unwrap_or_default();
        let window = close_ops::resolve_task_commit_receipt_window(
            task.created_at,
            &history,
            close_ops::task_commit_identity(task, latest),
        );
        let target = close_ops::freshest_target_ref(repo, target);
        let paths = close_ops::task_attribution::paths(repo, &target, &window, Some(head))
            .ok_or_else(|| anyhow!("delivery paths could not be attributed"))?;
        let base = close_ops::task_attribution::delivery_base(repo, &target, &window, Some(head));
        let mut changes = match base.as_deref() {
            Some(base) => qa_impact::io::changes_for_diff(repo, base, Some(head))?,
            None => Vec::new(),
        };
        changes.retain(|c| {
            paths.contains(&c.file) || c.old_file.as_ref().is_some_and(|p| paths.contains(p))
        });
        for path in paths {
            if !changes
                .iter()
                .any(|c| c.file == path || c.old_file.as_deref() == Some(path.as_str()))
            {
                changes.push(Change::modified(path));
            }
        }
        let report = qa_impact::analyze(
            repo,
            &self.load_config().qa(),
            &changes,
            base.as_deref(),
            Some(head),
        )?;
        let mut value = serde_json::to_value(report)?;
        value["head_sha"] = head.into();
        value["base_sha"] = base.into();
        store
            .patch_execution_state(&task.id, &serde_json::json!({"qa_impact": value}))
            .context("store authoritative QA impact")?;
        store.append_note(
            &task.id,
            &format!(
                "[{}] Authoritative QA impact @{head}:\n{}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                render(&value)
            ),
        )?;
        Ok(value)
    }
}

/// One ledger row: its id cell, its verdict cell and all its cells.
struct Row<'a> {
    id: &'a str,
    verdict: &'a str,
    cells: Vec<&'a str>,
}

/// Header names of a ledger table's verdict columns, in priority order: the
/// evidence ledger's `verdict`, the independent-QA journeys table's `Run`, and
/// short `Result` or `Status` tables. See [`names_column`] for compound
/// headers such as `Verdict / label`.
const VERDICT_HEADERS: [&str; 4] = ["verdict", "run", "result", "status"];

fn table_cells(line: &str) -> Vec<&str> {
    line.trim_matches('|')
        .split('|')
        .map(|c| c.trim().trim_matches('`').trim())
        .collect()
}

fn separator(cells: &[&str]) -> bool {
    cells
        .iter()
        .all(|c| c.contains('-') && c.chars().all(|ch| matches!(ch, '-' | ':')))
}

/// Whether a header cell names `name`: exactly, or followed by a separator
/// (`Verdict / label`, `Result/source`, `Run (n)`, `Status: prod`), so
/// `Status page`, `Result before run` or `Run id` does not. `verdict` is
/// unambiguous, so any `Verdict <qualifier>` counts (`verdict t1`).
/// Emphasis is ignored.
fn names_column(cell: &str, name: &str) -> bool {
    let cell = cell
        .trim_matches(|c: char| c == '*' || c == '_')
        .to_ascii_lowercase();
    cell.strip_prefix(name).is_some_and(|rest| {
        rest.is_empty()
            || rest.trim_start().starts_with(['/', '(', ':'])
            || (name == "verdict" && rest.starts_with(char::is_whitespace))
    })
}

/// The verdict-named columns of a header row, in [`VERDICT_HEADERS`] order.
fn verdict_columns(cells: &[&str]) -> Vec<usize> {
    VERDICT_HEADERS
        .iter()
        .flat_map(|name| {
            cells
                .iter()
                .enumerate()
                .filter(move |(_, cell)| names_column(cell, name))
                .map(|(index, _)| index)
        })
        .collect()
}

/// Ledger rows outside code fences. Only verdict columns are read. A table
/// header (the row above a `|---|` separator, or a row with a cell that is
/// exactly `Verdict`) names them by [`VERDICT_HEADERS`]; a row reads the first one
/// whose cell holds a verdict, so a `Run` column of run ids beside `Result`
/// still counts. A headed table without one has no verdict column. Only a
/// headerless row falls back by position: the fifth cell of a row with five or
/// more cells, else the second. A list row reads `- ID: verdict`.
fn ledger_rows(ledger: &str) -> Vec<Row<'_>> {
    let lines: Vec<&str> = ledger.lines().map(str::trim).collect();
    let mut fenced = false;
    // `Some(columns)` inside a headed table; `None` before any header.
    let mut header: Option<Vec<usize>> = None;
    let mut rows = Vec::new();
    for (index, line) in lines.iter().copied().enumerate() {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if !line.contains('|') {
            header = None;
            let row = line.trim_start_matches("- ");
            if let Some((id, verdict)) = row.split_once(':') {
                let (id, verdict) = (id.trim(), verdict.trim());
                rows.push(Row {
                    id,
                    verdict,
                    cells: vec![id, verdict],
                });
            }
            continue;
        }
        let cells = table_cells(line);
        if separator(&cells) {
            continue;
        }
        let above_separator = lines
            .get(index + 1)
            .is_some_and(|next| next.contains('|') && separator(&table_cells(next)));
        if above_separator || cells.iter().any(|c| c.eq_ignore_ascii_case("verdict")) {
            header = Some(verdict_columns(&cells));
            continue;
        }
        let positional = [if cells.len() >= 5 { 4 } else { 1 }];
        let columns = header.as_deref().unwrap_or(&positional);
        let verdict = columns
            .iter()
            .filter_map(|column| cells.get(*column).copied())
            .find(|cell| disposition(cell, &cells).is_some())
            .or_else(|| {
                columns
                    .first()
                    .and_then(|column| cells.get(*column).copied())
            });
        if let Some(verdict) = verdict {
            rows.push(Row {
                id: cells[0],
                verdict,
                cells,
            });
        }
    }
    rows
}

/// What one verdict cell records for its journey.
enum Disposition {
    /// PASS or FAIL: a run of the journey.
    Run,
    /// NOT A ROUTE or NOT EXERCISED: <reason>.
    Recorded,
    /// BLOCKED_EXTERNAL: neither PASS nor FAIL, so the journey is still owed.
    Owed(Option<crate::qa_release::blocked::BlockedExternal>),
}

/// Reads one verdict cell; bold (`**PASS**`) reads as plain.
fn disposition(cell: &str, cells: &[&str]) -> Option<Disposition> {
    use crate::qa_release::blocked::{self, BlockedExternal};
    let cell = cell.trim_start_matches('*');
    let after = |verdict: &str| {
        cell.strip_prefix(verdict)
            .map(|rest| rest.trim_start_matches('*'))
    };
    if cell.starts_with(blocked::VERDICT) {
        return Some(Disposition::Owed(BlockedExternal::from_row(cell, cells)));
    }
    let word = |verdict: &str| {
        after(verdict).is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with(|c: char| c.is_whitespace() || matches!(c, ':' | '—' | '-'))
        })
    };
    if word("PASS") || word("FAIL") {
        Some(Disposition::Run)
    } else if word("NOT A ROUTE")
        || after("NOT EXERCISED")
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|reason| {
                let reason = reason.trim().trim_start_matches('*').trim();
                !reason.is_empty() && reason != "<reason>"
            })
    {
        Some(Disposition::Recorded)
    } else {
        None
    }
}

/// G8 reads the park-time report, never a new diff. Legacy catalog adoption
/// remains advisory; enforcement arms when a YAML registry exists.
pub(crate) fn ledger_refusal(impact: &Value, head: &str, ledger: &str) -> Option<String> {
    use crate::qa_release::independence::{Runs, identity};
    if impact["registry"]["present"] != true {
        return None;
    }
    let live: Vec<_> = impact["journeys"]
        .as_array()?
        .iter()
        .filter(|j| j["status"] == "live")
        .filter_map(|j| Some((j["id"].as_str()?, j["independence_key"].as_str())))
        .collect();
    if live.is_empty() {
        return None;
    }
    if impact["head_sha"].as_str() != Some(head) {
        return Some("QA RECORD REJECTED: journey impact is for a different delivery head; refresh the QA dispatch for this round before recording approval.".into());
    }
    let rows = ledger_rows(ledger);
    let (mut missing, mut owed, mut dependent) = (Vec::new(), Vec::new(), Vec::new());
    for (id, key) in live {
        let recorded: Vec<_> = rows
            .iter()
            .filter(|row| row.id == id)
            .filter_map(|row| Some((row, disposition(row.verdict, &row.cells)?)))
            .collect();
        if recorded.is_empty() {
            missing.push(id);
        }
        for (row, disposition) in &recorded {
            if let Disposition::Owed(blocked) = disposition {
                let cell = (row.cells.len() >= 5).then(|| row.cells[1]);
                owed.push(match blocked {
                    Some(blocked) => blocked.owed(id, cell),
                    None => format!(
                        "{id} is BLOCKED_EXTERNAL without `blocked: provider=<provider> error=<error>`"
                    ),
                });
            }
        }
        if let Some(key) = key {
            let runs = Runs::count(
                recorded
                    .iter()
                    .filter(|(_, d)| matches!(d, Disposition::Run))
                    .map(|(row, _)| identity(key, &row.cells)),
            );
            dependent.extend(runs.refusal(id, key, 0));
        }
    }
    let mut refusals = Vec::new();
    if !missing.is_empty() {
        refusals.push(format!("QA RECORD REJECTED: the ledger has no row for live journeys {}. Add one row per journey: PASS, FAIL, NOT A ROUTE, or NOT EXERCISED: <reason>.", missing.join(", ")));
    }
    if !owed.is_empty() {
        refusals.push(format!("QA RECORD REJECTED: owed: {}: neither PASS nor FAIL; re-walk once the provider recovers.", owed.join("; ")));
    }
    if !dependent.is_empty() {
        refusals.push(format!("QA RECORD REJECTED: {}. A duplicate or unidentified run is not independent; record one distinct <key>=<id> per run.", dependent.join("; ")));
    }
    (!refusals.is_empty()).then(|| refusals.join("\n"))
}
