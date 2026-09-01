mod dependencies;
pub(crate) mod lifecycle;
mod notes;
mod proposals;
mod query;
pub(crate) mod repo_context;
mod update;

/// Return the canonical identity of the project represented by a Cassy root.
pub(crate) fn current_project_id(cas_root: &std::path::Path) -> Option<String> {
    crate::cloud::resolve_canonical_id(cas_root)
}

pub(crate) fn task_belongs_to_project(task: &cas_types::Task, project_id: Option<&str>) -> bool {
    project_id.is_some_and(|project_id| task.origin_project.as_deref() == Some(project_id))
}

/// Task board reads retain legacy rows that have no origin attribution. Rows
/// with an origin are local only when that origin matches the current project.
pub(crate) fn task_visible_in_project(task: &cas_types::Task, project_id: Option<&str>) -> bool {
    task.origin_project.is_none() || task_belongs_to_project(task, project_id)
}

pub(crate) fn foreign_tasks_hidden_footer(hidden: usize) -> Option<String> {
    (hidden > 0)
        .then(|| format!("{hidden} foreign-origin tasks hidden (include_foreign=true to show)"))
}

#[cfg(test)]
mod origin_project_tests {
    use super::task_belongs_to_project;
    use cas_types::Task;

    #[test]
    fn ownership_filter_accepts_only_exact_current_project() {
        let mut local = Task::new("local".to_string(), "Local".to_string());
        local.origin_project = Some("acme/accounting".to_string());
        assert!(task_belongs_to_project(&local, Some("acme/accounting")));

        local.origin_project = Some("acme/other".to_string());
        assert!(!task_belongs_to_project(&local, Some("acme/accounting")));

        local.origin_project = None;
        assert!(!task_belongs_to_project(&local, Some("acme/accounting")));
        assert!(!task_belongs_to_project(&local, None));
    }
}

pub(crate) fn ensure_task_origin(
    task: &cas_types::Task,
    cas_root: &std::path::Path,
    action: &str,
) -> Result<(), rmcp::ErrorData> {
    let project_id = current_project_id(cas_root);
    if task_belongs_to_project(task, project_id.as_deref()) {
        return Ok(());
    }

    let origin = task
        .origin_project
        .as_deref()
        .unwrap_or("unassigned legacy row");
    let current = project_id
        .as_deref()
        .unwrap_or("unresolved current project");
    Err(rmcp::ErrorData {
        code: rmcp::model::ErrorCode::INVALID_PARAMS,
        message: std::borrow::Cow::from(format!(
            "Cannot {action} task {}: origin project `{origin}` does not match current project `{current}`. Foreign or unassigned legacy tasks are excluded from this project; use an authorized supervisor task update to reassign the origin explicitly.",
            task.id
        )),
        data: None,
    })
}

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

pub(crate) fn ensure_no_external_blockers(
    cas_root: &std::path::Path,
    task_id: &str,
    action: &str,
) -> Result<(), rmcp::ErrorData> {
    let blockers = cas_store::ExternalTaskDependencyStore::open(cas_root)
        .and_then(|store| store.list_blocking_for_task(task_id))
        .map_err(|error| rmcp::ErrorData {
            code: rmcp::model::ErrorCode::INTERNAL_ERROR,
            message: std::borrow::Cow::from(format!(
                "Failed to check external blocking dependencies for task {task_id}: {error}"
            )),
            data: None,
        })?;
    if blockers.is_empty() {
        return Ok(());
    }
    let rendered = blockers
        .iter()
        .map(|blocker| {
            format!(
                "{} ({}, proposal {})",
                blocker.target_task_id, blocker.resolution_state, blocker.proposal_id
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(rmcp::ErrorData {
        code: rmcp::model::ErrorCode::INVALID_PARAMS,
        message: std::borrow::Cow::from(format!(
            "Cannot {action} task {task_id}: cross-project blockers are unresolved: {rendered}. Reconcile after the target closes; a rejected handoff remains blocking until an operator removes or replaces it."
        )),
        data: None,
    })
}
