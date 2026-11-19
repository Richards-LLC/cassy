//! cas-bc25: a repair for failing tests must run the whole test class, not
//! just the tests CI named. The daemon records the failing tests it saw as
//! data; a verdict or repair spawn on that task then needs a class selector,
//! and park needs receipts that cover it.
use std::path::Path;

use cas_store::TaskStore;
use serde_json::{Value, json};

use crate::factory_worker_check::{named_filter, stability::rust::filter_matches};

/// Names kept per task; the CI log holds the rest.
const MAX_RECORDED: usize = 20;

/// Remember the failing tests a CI run or merge-queue ejection named.
pub(crate) fn record_failing_tests(root: &Path, task_id: &str, names: &[String]) {
    let names: Vec<&String> = names.iter().take(MAX_RECORDED).collect();
    if names.is_empty() {
        return;
    }
    if let Err(error) = crate::store::open_task_store(root)
        .and_then(|store| store.patch_execution_state(task_id, &json!({"failing_tests": names})).map_err(Into::into))
    {
        tracing::warn!(%task_id, %error, "could not record failing tests for the repair class audit");
    }
}

fn state_list(state: &Value, field: &str) -> Vec<String> {
    state.get(field).and_then(Value::as_array).into_iter().flatten()
        .filter_map(|name| name.as_str().map(str::to_owned)).collect()
}

/// The recorded class selector for the task, if a verdict set one.
pub(crate) fn recorded_class(store: &dyn TaskStore, task_id: &str) -> Option<String> {
    let state = store.get_execution_state(task_id).ok()??;
    state.get("class_audit")?.as_str().map(str::to_owned)
}

/// Refuse a verdict or repair spawn that leaves recorded failing tests without
/// a class, and return the selector in force: the one offered now, else the
/// stored one. Callers persist exactly that, so a repeat verdict that omits the
/// class cannot erase the stored obligation.
pub(crate) fn require_class(store: &dyn TaskStore, task_id: &str, offered: Option<&str>) -> Result<Option<String>, String> {
    let state = store.get_execution_state(task_id).ok().flatten().unwrap_or_else(|| json!({}));
    let failing = state_list(&state, "failing_tests");
    let class = offered.map(str::trim).filter(|class| !class.is_empty())
        .map(str::to_owned).or_else(|| recorded_class(store, task_id));
    if failing.is_empty() {
        return Ok(class);
    }
    match class {
        Some(class) if named_filter(&class) && failing.iter().all(|name| filter_matches(&class, name)) => Ok(Some(class)),
        _ => Err(format!(
            "CLASS_AUDIT_REQUIRED: task {task_id} has recorded failing tests ({}); a repair that fixes only those leaves their class unrun. Name the whole class as a nextest filter that selects them, e.g. class_audit=\"test(ui::) | test(gates::)\" on request_changes. Park will then require a passing capped receipt that runs it.",
            failing.join(", "),
        )),
    }
}

/// Whether passing receipts at the tip (given as their filters) run every
/// operand of the class. Uses the capped runner's own filter matcher, probed
/// with names the operand selects, so exact `test(=name)` and substring
/// `test(name)` keep their meaning: an exact receipt covers an exact operand,
/// and only a substring receipt covers a substring operand's longer names.
pub(crate) fn receipts_cover_class(receipt_filters: &[String], class: &str) -> bool {
    class.split('|').map(str::trim).all(|operand| {
        let value = operand.trim_start_matches("test(").trim_end_matches(')');
        let probes = match value.strip_prefix('=') {
            Some(exact) => vec![exact.to_string()],
            None => vec![value.to_string(), format!("{value}_x"), format!("x_{value}")],
        };
        receipt_filters.iter().any(|filter| probes.iter().all(|probe| filter_matches(filter, probe)))
    })
}
