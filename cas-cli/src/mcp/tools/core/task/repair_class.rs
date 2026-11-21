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
    if names.is_empty() {
        return;
    }
    if let Err(error) = crate::store::open_task_store(root)
        .and_then(|_store| {
            let connection = cas_store::shared_db::shared_connection(&root.join("cas.db"))?;
            cas_store::shared_db::with_immediate_write_txn_pooled(&connection, |conn|
                record_failing_tests_with_conn(conn, task_id, names)).map_err(Into::into)
        })
    {
        tracing::warn!(%task_id, %error, "could not record failing tests for the repair class audit");
    }
}

fn merged_names(task_id: &str, state: &Value, names: &[String]) -> Vec<String> {
    let mut merged = Vec::new();
    for name in state_list(state, "failing_tests").into_iter().chain(names.iter().cloned()) {
        if !merged.contains(&name) { merged.push(name); }
    }
    let omitted = merged.len().saturating_sub(MAX_RECORDED);
    merged.truncate(MAX_RECORDED);
    if omitted > 0 {
        tracing::warn!(%task_id, omitted, limit = MAX_RECORDED, "truncated failing tests for the repair class audit");
    }
    merged
}

/// Join the verdict transaction without reopening its shared connection.
pub(crate) fn record_failing_tests_with_conn(conn: &rusqlite::Connection, task_id: &str, names: &[String]) -> cas_store::Result<()> {
    use rusqlite::OptionalExtension;
    if names.is_empty() { return Ok(()); }
    let state = conn.query_row("SELECT state FROM task_execution_states WHERE task_id=?1",
        [task_id], |row| row.get::<_, String>(0)).optional()?
        .map(|encoded| serde_json::from_str::<Value>(&encoded)).transpose()?
        .unwrap_or_else(|| json!({}));
    let patch = json!({"failing_tests": merged_names(task_id, &state, names)});
    let state = cas_types::merge_task_execution_state_patch(&state, &patch)
        .map_err(cas_store::StoreError::Parse)?;
    conn.execute("INSERT INTO task_execution_states (task_id,state,updated_at) VALUES (?1,?2,?3)
        ON CONFLICT(task_id) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at",
        rusqlite::params![task_id, serde_json::to_string(&state)?, chrono::Utc::now().to_rfc3339()])?;
    Ok(())
}

/// Qualified Rust test identifiers consumed by the class-audit selector.
pub(crate) fn is_test_identifier(name: &str) -> bool {
    name.split("::").all(|part| {
        let mut chars = part.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
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
    // Coverage understands `|` only, and an all-exact class is just the failing
    // names restated, so neither can name a class.
    let names_a_class = |class: &str| !class.contains('&')
        && class.split('|').any(|operand| !operand.trim().trim_start_matches("test(").starts_with('='));
    match class {
        Some(class) if named_filter(&class) && names_a_class(&class)
            && failing.iter().all(|name| filter_matches(&class, name)) => Ok(Some(class)),
        _ => Err(format!(
            "CLASS_AUDIT_REQUIRED: task {task_id} has recorded failing tests ({}); a repair that fixes only those leaves their class unrun. Name the whole class as a nextest filter that selects them using `|` and at least one substring operand (not only `test(=name)` and no `&`), e.g. class_audit=\"test(ui::) | test(gates::)\" on request_changes. Park will then require a passing capped receipt that runs it.",
            failing.join(", "),
        )),
    }
}

/// The CLASS_AUDIT_REQUIRED refusal a repair of this task would meet with no
/// class offered, or `None` when no failing tests are recorded or a recorded
/// class covers them. An unreadable store reads as no gap, as `require_class`.
pub(crate) fn class_audit_gap(root: &Path, task_id: &str) -> Option<String> {
    let store = crate::store::open_task_store(root).ok()?;
    require_class(store.as_ref(), task_id, None).err()
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
