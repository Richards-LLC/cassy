use crate::mcp::tools::core::imports::*;

impl CasCore {
    pub async fn cas_task_dep_add(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // Verify both tasks exist
        let mut from_task = task_store.get(&req.from_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("From task not found: {e}")),
            data: None,
        })?;
        task_store.get(&req.to_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("To task not found: {e}")),
            data: None,
        })?;

        let dep_type = match req.dep_type.to_lowercase().as_str() {
            "related" => DependencyType::Related,
            "parent" | "parentchild" => DependencyType::ParentChild,
            "discovered" | "discoveredfrom" => DependencyType::DiscoveredFrom,
            _ => DependencyType::Blocks,
        };

        let dep = Dependency {
            from_id: req.from_id.clone(),
            to_id: req.to_id.clone(),
            dep_type,
            created_at: chrono::Utc::now(),
            created_by: Some("mcp".to_string()),
        };

        task_store.add_dependency(&dep).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to add dependency: {e}")),
            data: None,
        })?;

        // A late blocking edge must re-arm an open (including reopened) task.
        // Non-blocking dependency types never project lifecycle status.
        if dep_type == DependencyType::Blocks
            && from_task.status == TaskStatus::Open
            && task_store
                .get_blockers(&from_task.id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Dependency was added, but failed to evaluate blockers: {e}"
                    )),
                    data: None,
                })?
                .iter()
                .any(|blocker| blocker.id == req.to_id)
        {
            from_task.status = TaskStatus::Blocked;
            from_task.updated_at = chrono::Utc::now();
            task_store.update(&from_task).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Dependency was added, but failed to mark task blocked: {e}"
                )),
                data: None,
            })?;
        }

        Ok(Self::success(format!(
            "Added dependency: {}",
            describe_dependency(&dep)
        )))
    }

    /// Remove a dependency of a specific type between tasks (cas-6009).
    ///
    /// Uses `dep_type` to target exactly one dependency row, leaving any
    /// other-typed deps between the same pair untouched.
    pub async fn cas_task_dep_remove(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let dep_type = match req.dep_type.to_lowercase().as_str() {
            "related" => DependencyType::Related,
            "parent" | "parentchild" | "parent-child" => DependencyType::ParentChild,
            "discovered" | "discoveredfrom" | "discovered-from" => DependencyType::DiscoveredFrom,
            "extracted" | "extractedfrom" | "extracted-from" => DependencyType::ExtractedFrom,
            _ => DependencyType::Blocks,
        };

        let found = task_store
            .remove_dependency_of_type(&req.from_id, &req.to_id, dep_type)
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to remove dependency: {e}")),
                data: None,
            })?;

        if !found {
            if dep_type == DependencyType::Blocks {
                let external_store = cas_store::ExternalTaskDependencyStore::open(&self.cas_root)
                    .map_err(|error| {
                    Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                })?;
                if external_store
                    .remove(&req.from_id, &req.to_id)
                    .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
                {
                    let mut task = task_store.get(&req.from_id).map_err(|error| {
                        Self::error(ErrorCode::INVALID_PARAMS, error.to_string())
                    })?;
                    if task.status == TaskStatus::Blocked
                        && task_store
                            .get_blockers(&req.from_id)
                            .unwrap_or_default()
                            .is_empty()
                        && external_store
                            .list_blocking_for_task(&req.from_id)
                            .unwrap_or_default()
                            .is_empty()
                    {
                        task.status = TaskStatus::Open;
                        task.updated_at = chrono::Utc::now();
                        task_store.update(&task).map_err(|error| {
                            Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                        })?;
                    }
                    return Ok(Self::success(format!(
                        "Removed external dependency: {} was blocked_by {}.",
                        req.from_id, req.to_id
                    )));
                }
            }
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "No {:?} dependency found from {} to {}",
                    dep_type, req.from_id, req.to_id
                )),
                data: None,
            });
        }

        let removed = Dependency {
            from_id: req.from_id.clone(),
            to_id: req.to_id.clone(),
            dep_type,
            created_at: chrono::Utc::now(),
            created_by: None,
        };
        Ok(Self::success(format!(
            "Removed dependency: {}",
            describe_dependency(&removed)
        )))
    }
}

/// cas-ac2e: render a dependency edge in plain, unambiguous words instead of
/// a bare `from -> to` arrow.
///
/// BUG-dep-add-direction-ambiguous-output-2026-07-08.md: the edge
/// **semantics** (`from_id`/`to_id`, direction) are correct and untouched by
/// this change — `dep_add id=A to_id=B dep_type=blocks` has always created
/// "A is blocked_by B" (B must finish first). Only the OUTPUT was
/// misleading: `"Added Blocks dependency: A -> B"` reads naturally as "A
/// blocks B" (the arrow + the word "Blocks" together imply A is the
/// blocker), which is the *opposite* of the actual effect. This live-caused
/// a supervisor to file five dependency edges 180° backwards.
///
/// Every variant below states which task is which role in plain words, with
/// no arrow, so the direction can't be misread at a glance.
pub(crate) fn describe_dependency(dep: &Dependency) -> String {
    let a = &dep.from_id;
    let b = &dep.to_id;
    match dep.dep_type {
        DependencyType::Blocks => {
            format!("{a} will not start until {b} is done ({a} blocked_by {b}).")
        }
        DependencyType::ParentChild => {
            format!("{a} is a child of {b} ({b} is {a}'s parent).")
        }
        DependencyType::Related => {
            format!("{a} is related to {b}.")
        }
        DependencyType::DiscoveredFrom => {
            format!("{a} was discovered while working on {b} ({a} discovered_from {b}).")
        }
        DependencyType::ExtractedFrom => {
            format!("{a} was extracted from {b} ({a} extracted_from {b}).")
        }
    }
}

#[cfg(test)]
mod describe_dependency_tests {
    use super::*;

    #[test]
    fn blocks_states_blocked_by_in_plain_words_no_arrow() {
        let dep = Dependency::new(
            "cas-a".to_string(),
            "cas-b".to_string(),
            DependencyType::Blocks,
        );
        let msg = describe_dependency(&dep);
        assert!(!msg.contains("->"), "must not contain a bare arrow: {msg}");
        assert!(
            msg.contains("blocked_by"),
            "must state blocked_by in plain words: {msg}"
        );
        assert!(
            msg.contains("cas-a will not start until cas-b is done"),
            "must spell out which task waits on which: {msg}"
        );
    }

    #[test]
    fn parent_child_names_parent_and_child_explicitly() {
        let dep = Dependency::new(
            "cas-child".to_string(),
            "cas-parent".to_string(),
            DependencyType::ParentChild,
        );
        let msg = describe_dependency(&dep);
        assert!(!msg.contains("->"), "must not contain a bare arrow: {msg}");
        assert!(msg.contains("cas-child is a child of cas-parent"));
        assert!(msg.contains("cas-parent is cas-child's parent"));
    }

    #[test]
    fn related_has_no_arrow() {
        let dep = Dependency::new(
            "cas-a".to_string(),
            "cas-b".to_string(),
            DependencyType::Related,
        );
        assert!(!describe_dependency(&dep).contains("->"));
    }

    #[test]
    fn discovered_from_and_extracted_from_have_no_arrow() {
        for dep_type in [
            DependencyType::DiscoveredFrom,
            DependencyType::ExtractedFrom,
        ] {
            let dep = Dependency::new("cas-a".to_string(), "cas-b".to_string(), dep_type);
            let msg = describe_dependency(&dep);
            assert!(
                !msg.contains("->"),
                "{dep_type:?}: must not contain arrow: {msg}"
            );
        }
    }
}
