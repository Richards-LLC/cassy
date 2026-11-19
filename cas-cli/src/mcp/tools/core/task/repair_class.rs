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
/// a class. `offered` is a selector supplied now; otherwise the stored one counts.
pub(crate) fn require_class(store: &dyn TaskStore, task_id: &str, offered: Option<&str>) -> Result<(), String> {
    let state = store.get_execution_state(task_id).ok().flatten().unwrap_or_else(|| json!({}));
    let failing = state_list(&state, "failing_tests");
    if failing.is_empty() {
        return Ok(());
    }
    let class = offered.map(str::trim).filter(|class| !class.is_empty())
        .map(str::to_owned).or_else(|| recorded_class(store, task_id));
    match class {
        Some(class) if named_filter(&class) && failing.iter().all(|name| filter_matches(&class, name)) => Ok(()),
        _ => Err(format!(
            "CLASS_AUDIT_REQUIRED: task {task_id} has recorded failing tests ({}); a repair that fixes only those leaves their class unrun. Name the whole class as a nextest filter that selects them, e.g. class_audit=\"test(ui::) | test(gates::)\" on request_changes. Park will then require a passing capped receipt that runs it.",
            failing.join(", "),
        )),
    }
}

/// Whether passing receipts at the tip (given as their filters) run every
/// operand of the class. Uses the capped runner's own filter matcher.
pub(crate) fn receipts_cover_class(receipt_filters: &[String], class: &str) -> bool {
    class.split('|').map(|operand| operand.trim().trim_start_matches("test(").trim_end_matches(')').trim_start_matches('='))
        .all(|operand| receipt_filters.iter().any(|filter| filter_matches(filter, operand)))
}
