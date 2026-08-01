mod dependencies;
pub(crate) mod lifecycle;
mod notes;
mod query;
pub(crate) mod repo_context;
mod update;

/// Reject lifecycle actions while a task still has open `blocks` dependencies.
///
/// `TaskStore::get_blockers` deliberately filters to `dep_type = 'blocks'`, so
/// parent-child, related, discovered-from, and extracted-from edges never enter
/// this gate. Keep this shared between `start` and manual `claim` so neither
/// path can acquire a lease or move the task to in-progress prematurely.
pub(crate) fn ensure_no_open_blockers(
    task_store: &dyn cas_store::TaskStore,
    task_id: &str,
    action: &str,
) -> Result<(), rmcp::ErrorData> {
    let mut blocker_ids = task_store
        .get_blockers(task_id)
        .map_err(|error| rmcp::ErrorData {
            code: rmcp::model::ErrorCode::INTERNAL_ERROR,
            message: std::borrow::Cow::from(format!(
                "Failed to check blocking dependencies for task {task_id}: {error}"
            )),
            data: None,
        })?
        .into_iter()
        .map(|task| task.id)
        .collect::<Vec<_>>();

    blocker_ids.sort();
    blocker_ids.dedup();
    if blocker_ids.is_empty() {
        return Ok(());
    }

    Err(rmcp::ErrorData {
        code: rmcp::model::ErrorCode::INVALID_PARAMS,
        message: std::borrow::Cow::from(format!(
            "Cannot {action} task {task_id}: blocking dependencies are still open: {}. \
             Close those blocker tasks first, or remove an incorrect `blocks` dependency \
             with `task action=dep_remove id={task_id} to_id=<blocker-id> dep_type=blocks`.",
            blocker_ids.join(", ")
        )),
        data: None,
    })
}
