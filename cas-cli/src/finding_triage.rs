//! Severity × reach on the existing finding, task-label and spawn paths.
use std::{collections::BTreeSet, path::Path};

use cas_types::{FindingReach, FindingSeverity, FindingTriage, Priority, Task};

fn main_journeys(root: &Path) -> BTreeSet<String> {
    let path = crate::config::Config::load(root)
        .map(|c| c.qa().registry.path).unwrap_or_else(|_| crate::qa_registry::DEFAULT_PATH.into());
    let repo = if root.file_name().is_some_and(|name| name == ".cas") {
        root.parent().unwrap_or(root)
    } else { root };
    crate::qa_registry::load(&repo.join(path)).ok().map(|registry| {
        registry.main_journeys.into_iter().filter(|id| registry.journeys.iter().any(|journey| {
            journey.id == *id && matches!(journey.status.as_str(), "live" | "active")
        })).collect()
    }).unwrap_or_default()
}

pub(crate) fn normalize_finding(root: &Path, triage: &mut FindingTriage) -> bool {
    let main = triage.journey.as_ref().is_some_and(|id| main_journeys(root).contains(id));
    triage.normalize(main);
    main
}

fn label_value(labels: &[String], key: &str) -> Option<String> {
    labels.iter().rev().find_map(|label| label.strip_prefix(key).map(str::to_owned))
}

pub(crate) fn finding_labels(labels: &mut Vec<String>, triage: &FindingTriage) {
    labels.retain(|l| !l.starts_with("severity:") && !l.starts_with("reach:") && !l.starts_with("journey:"));
    let severity = serde_json::to_value(triage.severity).expect("severity serializes");
    let reach = serde_json::to_value(triage.reach).expect("reach serializes");
    labels.push(format!("severity:{}", severity.as_str().unwrap()));
    labels.push(format!("reach:{}", reach.as_str().unwrap()));
    if let Some(id) = &triage.journey { labels.push(format!("journey:{id}")); }
    if triage.repro.as_deref().is_some_and(|s| !s.trim().is_empty())
        && !labels.iter().any(|l| l == "runtime-repro")
    { labels.push("runtime-repro".into()); }
}

/// Unclassified legacy findings are theoretical too. Ordinary untriaged work
/// retains its existing priority. A caller's manual priority never wins over axes.
pub(crate) fn normalize_labels(root: &Path, labels: &mut Vec<String>, notes: &str) -> Option<Priority> {
    if !labels.iter().any(|l| l == "from-finding" || l.starts_with("severity:") || l.starts_with("reach:")) {
        return None;
    }
    let repro = notes.lines().filter_map(|line| line.split_once("Repro:").map(|(_, text)| text.trim()))
        .find(|text| !text.is_empty()).map(str::to_owned);
    labels.retain(|label| label != "runtime-repro");
    let reproduced = repro.is_some();
    let mut triage = FindingTriage {
        severity: label_value(labels, "severity:").and_then(|s| serde_json::from_value(serde_json::json!(s)).ok()).unwrap_or(FindingSeverity::Broken),
        reach: label_value(labels, "reach:").and_then(|s| serde_json::from_value(serde_json::json!(s)).ok())
            .unwrap_or(if reproduced { FindingReach::Secondary } else { FindingReach::Theoretical }),
        journey: label_value(labels, "journey:"),
        repro,
    };
    let main = normalize_finding(root, &mut triage);
    let priority = triage.priority(main);
    finding_labels(labels, &triage);
    labels.retain(|l| l != "triage-backlog");
    if priority.0 == 4 && triage.reach == FindingReach::Theoretical { labels.push("triage-backlog".into()); }
    Some(priority)
}

pub(crate) fn apply(root: &Path, task: &mut Task) {
    if let Some(priority) = normalize_labels(root, &mut task.labels, &task.notes) { task.priority = priority; }
}

pub(crate) fn focus_brief(root: &Path) -> String {
    crate::config::Config::load(root).ok().and_then(|c| c.factory().focus_mode)
        .filter(|s| !s.trim().is_empty()).map(|s| format!("\n\nOperator focus: {}", s.trim())).unwrap_or_default()
}

/// Advisory on the existing spawn receipt: it does not hold the spawn.
pub(crate) fn staffing_notice(root: &Path, ids: &[String]) -> String {
    let Ok(store) = crate::store::open_task_store_cached(root) else { return String::new(); };
    let Ok(mut tasks) = store.list(None) else { return String::new(); };
    for task in &mut tasks { apply(root, task); }
    let theoretical: Vec<_> = tasks.iter().filter(|t| ids.contains(&t.id)
        && t.labels.iter().any(|l| l == "reach:theoretical")).map(|t| t.id.as_str()).collect();
    if theoretical.is_empty() { return String::new(); }
    let project = crate::cloud::resolve_canonical_id(root);
    let unowned: Vec<_> = tasks.iter().filter(|t| !t.is_terminal() && t.assignee.is_none()
        && (t.origin_project.is_none() || t.origin_project == project)
        && t.labels.iter().any(|l| l == "reach:main")
        && t.labels.iter().any(|l| matches!(l.as_str(), "severity:broken" | "severity:money" | "severity:data_loss")))
        .map(|t| t.id.as_str()).collect();
    if unowned.is_empty() { return String::new(); }
    format!("\nTRIAGE: staffing theoretical {} while main-journey FAIL {} is unowned. Staff the visible failure first.", theoretical.join(", "), unowned.join(", "))
}
