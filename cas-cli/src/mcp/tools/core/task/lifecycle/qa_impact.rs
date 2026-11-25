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

pub(super) fn deferred_summary(impact: &Value) -> String {
    impact["g61_deferred_journeys"].as_array().into_iter().flatten().map(|row| {
        format!("\nG-61 deferred to release batch QA: {} ({}, {}).", text(&row["id"]), text(&row["spec"]), text(&row["reason"]))
    }).collect()
}

/// Shared by the task note and independent review brief.
pub(crate) fn render(impact: &Value) -> String {
    let mut out = String::new();
    if let Some(warnings) = impact["warnings"].as_array() {
        for warning in warnings {
            out.push_str(&format!("> WARNING: {}\n", cell(text(warning))));
        }
    }
    if let Some(safety) = impact
        .get("safety")
        .filter(|value| value.is_object())
        .and_then(|value| serde_json::from_value::<crate::qa_registry::Safety>(value.clone()).ok())
    {
        out.push_str(&format!("> {}\n", cell(&safety.brief())));
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
    out.push_str(&deferred_summary(impact));
    out.push_str("\nRecord one ledger row per live journey: PASS, FAIL, NOT A ROUTE, or NOT EXERCISED: <reason>.\n");
    out
}

pub(crate) fn predicted_value(core: &CasCore, task: &Task) -> Result<Value> {
    let repo = super::super::repo_context::resolve_task_metadata_repo(&core.cas_root, task)
        .map_err(|error| anyhow!(error))?;
    Ok(serde_json::to_value(crate::qa_impact::predicted(&repo, &core.load_config().qa(), task)?)?)
}

pub(crate) fn predicted(core: &CasCore, task: &Task) -> String {
    let result = predicted_value(core, task);
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
        deferred: Option<&[crate::qa_registry::DeferredSpec]>,
    ) -> Result<Value> {
        let store = self
            .open_task_store()
            .map_err(|e| anyhow!(e.message.to_string()))?;
        if let Some(state) = store.get_execution_state(&task.id)?
            && let Some(impact) = state.get("qa_impact")
            && impact["head_sha"].as_str() == Some(head)
            && deferred.is_none_or(|rows| impact["g61_deferred_journeys"] == serde_json::json!(rows))
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
            close_ops::task_commit_identity_in_repo(repo, task, latest).map_err(|e| anyhow!(e))?,
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
        if let Some(deferred) = deferred {
            value["g61_deferred_journeys"] = serde_json::to_value(deferred)?;
        }
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

/// G8 reads the park-time report, never a new diff. Legacy catalog adoption
/// remains advisory; enforcement arms when a YAML registry exists.
// gate-audit[forward]: G-118 The qa_record decision tags this pure renderer output.
pub(crate) fn ledger_refusal(impact: &Value, head: &str, ledger: &str) -> Option<String> {
    if impact["registry"]["present"] != true {
        return None;
    }
    let live: Vec<_> = impact["journeys"]
        .as_array()?
        .iter()
        .filter(|j| j["status"] == "live")
        .filter_map(|j| j["id"].as_str())
        .collect();
    if live.is_empty() {
        return None;
    }
    if impact["head_sha"].as_str() != Some(head) {
        return Some("QA RECORD REJECTED: journey impact is for a different delivery head; refresh the QA dispatch for this round before recording approval.".into());
    }
    let parsed = crate::qa_ledger::parse(ledger, 1);
    if !parsed.ok { return Some(format!("QA RECORD REJECTED: {}", parsed.refusal())); }
    let missing: Vec<_> = live.into_iter().filter(|id| {
        !parsed.rows.iter().any(|row| row.id == *id || row.cell == *id)
    }).collect();
    (!missing.is_empty()).then(|| format!("QA RECORD REJECTED: the ledger has no row for live journeys {}. Add one row per journey: PASS, FAIL, NOT A ROUTE, or NOT EXERCISED: <reason>.", missing.join(", ")))
}

#[cfg(test)]
#[path = "qa_impact_tests.rs"]
mod checkout_tests;
