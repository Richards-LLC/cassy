use crate::mcp::tools::core::imports::*;

impl CasCore {
    // ========================================================================
    // Additional Task Tools
    // ========================================================================

    /// Delete a task
    pub async fn cas_task_delete(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // Verify task exists
        task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        task_store.delete(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!("Deleted task: {}", req.id)))
    }

    /// List dependencies for a task (both outgoing and incoming).
    ///
    /// cas-ac2e: renders plain-worded "blocked by:"/"blocks:" (and similar)
    /// sections instead of raw `from -> to` arrows — see
    /// BUG-dep-add-direction-ambiguous-output-2026-07-08.md. Edge
    /// direction/semantics are unchanged; only this rendering changed.
    pub async fn cas_task_dep_list(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // Outgoing: this task is `from_id` — i.e. what THIS task depends on /
        // is blocked_by / is a child of / etc.
        let outgoing = task_store.get_dependencies(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to get dependencies: {e}")),
            data: None,
        })?;

        // Incoming: this task is `to_id` — i.e. what depends on / is
        // blocked_by / is a child of THIS task.
        let incoming = task_store.get_dependents(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to get dependents: {e}")),
            data: None,
        })?;

        if outgoing.is_empty() && incoming.is_empty() {
            return Ok(Self::success(format!("No dependencies for {}", req.id)));
        }

        Ok(Self::success(render_dependency_sections(
            &req.id, &outgoing, &incoming,
        )))
    }
}

/// cas-ac2e: shared plain-words dependency renderer for `dep_list`.
///
/// `outgoing` = deps where `task_id == from_id` (what this task depends on).
/// `incoming` = deps where `task_id == to_id` (what depends on this task).
/// Every section states the relationship in words — no bare `A -> B` arrow.
fn render_dependency_sections(
    task_id: &str,
    outgoing: &[Dependency],
    incoming: &[Dependency],
) -> String {
    let mut output = format!("Dependencies for {task_id}:\n\n");

    let blocked_by: Vec<&str> = outgoing
        .iter()
        .filter(|d| d.dep_type == DependencyType::Blocks)
        .map(|d| d.to_id.as_str())
        .collect();
    if !blocked_by.is_empty() {
        output.push_str(&format!(
            "Blocked by (this task will not start until these are done): {}\n",
            blocked_by.join(", ")
        ));
    }

    let requires_start: Vec<&str> = outgoing
        .iter()
        .filter(|d| d.dep_type == DependencyType::RequiresStart)
        .map(|d| d.to_id.as_str())
        .collect();
    if !requires_start.is_empty() {
        output.push_str(&format!(
            "Requires start (hard gate before this task starts): {}\n",
            requires_start.join(", ")
        ));
    }

    let blocks: Vec<&str> = incoming
        .iter()
        .filter(|d| d.dep_type == DependencyType::Blocks)
        .map(|d| d.from_id.as_str())
        .collect();
    if !blocks.is_empty() {
        output.push_str(&format!(
            "Blocks (these will not start until this task is done): {}\n",
            blocks.join(", ")
        ));
    }

    let parents: Vec<&str> = outgoing
        .iter()
        .filter(|d| d.dep_type == DependencyType::ParentChild)
        .map(|d| d.to_id.as_str())
        .collect();
    if !parents.is_empty() {
        output.push_str(&format!("Parent: {}\n", parents.join(", ")));
    }

    let children: Vec<&str> = incoming
        .iter()
        .filter(|d| d.dep_type == DependencyType::ParentChild)
        .map(|d| d.from_id.as_str())
        .collect();
    if !children.is_empty() {
        output.push_str(&format!("Children: {}\n", children.join(", ")));
    }

    let related: Vec<&str> = outgoing
        .iter()
        .filter(|d| d.dep_type == DependencyType::Related)
        .map(|d| d.to_id.as_str())
        .chain(
            incoming
                .iter()
                .filter(|d| d.dep_type == DependencyType::Related)
                .map(|d| d.from_id.as_str()),
        )
        .collect();
    if !related.is_empty() {
        output.push_str(&format!("Related: {}\n", related.join(", ")));
    }

    let discovered_from: Vec<&str> = outgoing
        .iter()
        .filter(|d| d.dep_type == DependencyType::DiscoveredFrom)
        .map(|d| d.to_id.as_str())
        .collect();
    if !discovered_from.is_empty() {
        output.push_str(&format!(
            "Discovered from: {}\n",
            discovered_from.join(", ")
        ));
    }

    let discoveries: Vec<&str> = incoming
        .iter()
        .filter(|d| d.dep_type == DependencyType::DiscoveredFrom)
        .map(|d| d.from_id.as_str())
        .collect();
    if !discoveries.is_empty() {
        output.push_str(&format!(
            "Discovered while working on this task: {}\n",
            discoveries.join(", ")
        ));
    }

    let extracted_from: Vec<&str> = outgoing
        .iter()
        .filter(|d| d.dep_type == DependencyType::ExtractedFrom)
        .map(|d| d.to_id.as_str())
        .collect();
    if !extracted_from.is_empty() {
        output.push_str(&format!("Extracted from: {}\n", extracted_from.join(", ")));
    }

    let extractions: Vec<&str> = incoming
        .iter()
        .filter(|d| d.dep_type == DependencyType::ExtractedFrom)
        .map(|d| d.from_id.as_str())
        .collect();
    if !extractions.is_empty() {
        output.push_str(&format!("Extracted into: {}\n", extractions.join(", ")));
    }

    output
}

#[cfg(test)]
mod dep_list_render_tests {
    use super::*;

    fn dep(from: &str, to: &str, dep_type: DependencyType) -> Dependency {
        Dependency::new(from.to_string(), to.to_string(), dep_type)
    }

    #[test]
    fn blocked_by_and_blocks_sections_have_no_arrow_and_name_the_right_task() {
        // Task X (this task) is blocked_by Y: outgoing dep from_id=X to_id=Y.
        let outgoing = vec![dep("cas-x", "cas-y", DependencyType::Blocks)];
        // Task Z is blocked_by X: incoming dep from_id=Z to_id=X.
        let incoming = vec![dep("cas-z", "cas-x", DependencyType::Blocks)];

        let out = render_dependency_sections("cas-x", &outgoing, &incoming);
        assert!(!out.contains("->"), "must not render a bare arrow: {out}");
        assert!(out.to_lowercase().contains("blocked by"));
        assert!(out.contains("cas-y"), "blocked-by must name the blocker: {out}");
        assert!(out.to_lowercase().contains("blocks"));
        assert!(
            out.contains("cas-z"),
            "blocks must name the task waiting on this one: {out}"
        );
    }

    #[test]
    fn no_deps_never_reaches_renderer() {
        // Sanity: the handler short-circuits to "No dependencies for X"
        // before calling the renderer when both lists are empty — the
        // renderer itself doesn't need to special-case emptiness beyond
        // producing just the header.
        let out = render_dependency_sections("cas-x", &[], &[]);
        assert_eq!(out, "Dependencies for cas-x:\n\n");
    }

    #[test]
    fn parent_child_sections_are_labeled_plainly() {
        // X is a child of P: outgoing ParentChild from_id=X to_id=P.
        let outgoing = vec![dep("cas-x", "cas-p", DependencyType::ParentChild)];
        // C is a child of X: incoming ParentChild from_id=C to_id=X.
        let incoming = vec![dep("cas-c", "cas-x", DependencyType::ParentChild)];

        let out = render_dependency_sections("cas-x", &outgoing, &incoming);
        assert!(!out.contains("->"));
        assert!(out.contains("Parent: cas-p"));
        assert!(out.contains("Children: cas-c"));
    }
}
