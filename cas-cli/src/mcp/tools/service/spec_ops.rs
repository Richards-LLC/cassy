use crate::mcp::tools::service::imports::*;

mod advanced;

impl CasService {
    pub(super) async fn spec_create(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};
        use cas_types::{Scope, Spec, SpecType};

        let title = req
            .title
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "title required"))?;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        let id = store.generate_id().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to generate spec ID: {e}"),
            )
        })?;

        let scope: Scope = req
            .scope
            .as_deref()
            .unwrap_or("project")
            .parse()
            .unwrap_or(Scope::Project);

        let spec_type: SpecType = req
            .spec_type
            .as_deref()
            .unwrap_or("epic")
            .parse()
            .unwrap_or(SpecType::Epic);

        let mut spec = Spec::with_scope(id.clone(), title, scope);
        spec.spec_type = spec_type;
        spec.summary = req.summary.unwrap_or_default();
        spec.goals = Self::parse_comma_list(&req.goals);
        spec.in_scope = Self::parse_comma_list(&req.in_scope);
        spec.out_of_scope = Self::parse_comma_list(&req.out_of_scope);
        spec.users = Self::parse_comma_list(&req.users);
        spec.technical_requirements = Self::parse_comma_list(&req.technical_requirements);
        spec.acceptance_criteria = Self::parse_comma_list(&req.acceptance_criteria);
        spec.design_notes = req.design_notes.unwrap_or_default();
        spec.additional_notes = req.additional_notes.unwrap_or_default();
        spec.task_id = req.task_id;
        spec.source_ids = Self::parse_comma_list(&req.source_ids);
        spec.tags = Self::parse_comma_list(&req.tags);

        store.add(&spec).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to create spec: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Created spec: {} - {}\n\nType: {}\nStatus: {}",
            id, spec.title, spec.spec_type, spec.status
        )))
    }

    pub(super) async fn spec_show(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};

        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        let spec = store
            .get(&id)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, format!("Spec not found: {e}")))?;

        let mut output = format!(
            "# {} ({})\n\n**Type**: {} | **Status**: {} | **Version**: {}\n",
            spec.title, spec.id, spec.spec_type, spec.status, spec.version
        );

        if !spec.summary.is_empty() {
            output.push_str(&format!("\n## Summary\n{}\n", spec.summary));
        }

        if !spec.goals.is_empty() {
            output.push_str("\n## Goals\n");
            for goal in &spec.goals {
                output.push_str(&format!("- {goal}\n"));
            }
        }

        if !spec.in_scope.is_empty() {
            output.push_str("\n## In Scope\n");
            for item in &spec.in_scope {
                output.push_str(&format!("- {item}\n"));
            }
        }

        if !spec.out_of_scope.is_empty() {
            output.push_str("\n## Out of Scope\n");
            for item in &spec.out_of_scope {
                output.push_str(&format!("- {item}\n"));
            }
        }

        if !spec.users.is_empty() {
            output.push_str("\n## Target Users\n");
            for user in &spec.users {
                output.push_str(&format!("- {user}\n"));
            }
        }

        if !spec.technical_requirements.is_empty() {
            output.push_str("\n## Technical Requirements\n");
            for req in &spec.technical_requirements {
                output.push_str(&format!("- {req}\n"));
            }
        }

        if !spec.acceptance_criteria.is_empty() {
            output.push_str("\n## Acceptance Criteria\n");
            for criterion in &spec.acceptance_criteria {
                output.push_str(&format!("- {criterion}\n"));
            }
        }

        if !spec.design_notes.is_empty() {
            output.push_str(&format!("\n## Design Notes\n{}\n", spec.design_notes));
        }

        if !spec.additional_notes.is_empty() {
            output.push_str(&format!(
                "\n## Additional Notes\n{}\n",
                spec.additional_notes
            ));
        }

        if let Some(task_id) = &spec.task_id {
            output.push_str(&format!("\n**Linked Task**: {task_id}\n"));
        }

        if !spec.tags.is_empty() {
            output.push_str(&format!("\n**Tags**: {}\n", spec.tags.join(", ")));
        }

        output.push_str(&format!(
            "\n---\nCreated: {} | Updated: {}",
            spec.created_at.format("%Y-%m-%d %H:%M"),
            spec.updated_at.format("%Y-%m-%d %H:%M")
        ));

        if let Some(approved_at) = spec.approved_at {
            output.push_str(&format!(
                " | Approved: {} by {}",
                approved_at.format("%Y-%m-%d %H:%M"),
                spec.approved_by.as_deref().unwrap_or("unknown")
            ));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn spec_update(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};
        use cas_types::{Spec, SpecType};
        use chrono::Utc;

        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        let mut spec = store
            .get(&id)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, format!("Spec not found: {e}")))?;

        // Check if we should create a new version
        if req.new_version.unwrap_or(false) {
            let new_id = store.generate_id().map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to generate spec ID: {e}"),
                )
            })?;

            let mut new_spec = Spec::with_scope(new_id.clone(), spec.title.clone(), spec.scope);
            new_spec.summary = req.summary.unwrap_or(spec.summary);
            new_spec.goals = if req.goals.is_some() {
                Self::parse_comma_list(&req.goals)
            } else {
                spec.goals
            };
            new_spec.in_scope = if req.in_scope.is_some() {
                Self::parse_comma_list(&req.in_scope)
            } else {
                spec.in_scope
            };
            new_spec.out_of_scope = if req.out_of_scope.is_some() {
                Self::parse_comma_list(&req.out_of_scope)
            } else {
                spec.out_of_scope
            };
            new_spec.users = if req.users.is_some() {
                Self::parse_comma_list(&req.users)
            } else {
                spec.users
            };
            new_spec.technical_requirements = if req.technical_requirements.is_some() {
                Self::parse_comma_list(&req.technical_requirements)
            } else {
                spec.technical_requirements
            };
            new_spec.acceptance_criteria = if req.acceptance_criteria.is_some() {
                Self::parse_comma_list(&req.acceptance_criteria)
            } else {
                spec.acceptance_criteria
            };
            new_spec.design_notes = req.design_notes.unwrap_or(spec.design_notes);
            new_spec.additional_notes = req.additional_notes.unwrap_or(spec.additional_notes);
            new_spec.spec_type = req
                .spec_type
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(spec.spec_type);
            new_spec.task_id = req.task_id.or(spec.task_id);
            new_spec.source_ids = if req.source_ids.is_some() {
                Self::parse_comma_list(&req.source_ids)
            } else {
                spec.source_ids
            };
            new_spec.tags = if req.tags.is_some() {
                Self::parse_comma_list(&req.tags)
            } else {
                spec.tags
            };
            new_spec.version = spec.version + 1;
            new_spec.previous_version_id = Some(id.clone());

            store.add(&new_spec).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to create new version: {e}"),
                )
            })?;

            return Ok(Self::success(format!(
                "Created new version: {} (v{}) supersedes {} (v{})",
                new_id, new_spec.version, id, spec.version
            )));
        }

        // Update in place
        if let Some(title) = req.title {
            spec.title = title;
        }
        if let Some(summary) = req.summary {
            spec.summary = summary;
        }
        if req.goals.is_some() {
            spec.goals = Self::parse_comma_list(&req.goals);
        }
        if req.in_scope.is_some() {
            spec.in_scope = Self::parse_comma_list(&req.in_scope);
        }
        if req.out_of_scope.is_some() {
            spec.out_of_scope = Self::parse_comma_list(&req.out_of_scope);
        }
        if req.users.is_some() {
            spec.users = Self::parse_comma_list(&req.users);
        }
        if req.technical_requirements.is_some() {
            spec.technical_requirements = Self::parse_comma_list(&req.technical_requirements);
        }
        if req.acceptance_criteria.is_some() {
            spec.acceptance_criteria = Self::parse_comma_list(&req.acceptance_criteria);
        }
        if let Some(design_notes) = req.design_notes {
            spec.design_notes = design_notes;
        }
        if let Some(additional_notes) = req.additional_notes {
            spec.additional_notes = additional_notes;
        }
        if let Some(spec_type_str) = req.spec_type {
            if let Ok(spec_type) = spec_type_str.parse::<SpecType>() {
                spec.spec_type = spec_type;
            }
        }
        if let Some(task_id) = req.task_id {
            spec.task_id = Some(task_id);
        }
        if req.source_ids.is_some() {
            spec.source_ids = Self::parse_comma_list(&req.source_ids);
        }
        if req.tags.is_some() {
            spec.tags = Self::parse_comma_list(&req.tags);
        }
        spec.updated_at = Utc::now();

        store.update(&spec).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to update spec: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Updated spec: {} - {}",
            id, spec.title
        )))
    }

    pub(super) async fn spec_delete(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};

        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        // Get spec info before deleting
        let spec = store
            .get(&id)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, format!("Spec not found: {e}")))?;

        store.delete(&id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to delete spec: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Deleted spec: {} - {}",
            id, spec.title
        )))
    }

    pub(super) async fn spec_list(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};
        use cas_types::SpecStatus;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        let status_filter: Option<SpecStatus> = req.status.as_deref().and_then(|s| s.parse().ok());

        let specs = store.list(status_filter).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list specs: {e}"),
            )
        })?;

        let limit = req.limit.unwrap_or(20);
        let specs: Vec<_> = specs.into_iter().take(limit).collect();

        if specs.is_empty() {
            return Ok(Self::success("No specs found."));
        }

        let mut output = format!("Found {} spec(s):\n\n", specs.len());

        for spec in specs {
            output.push_str(&format!(
                "- **{}** - {} [{}] ({})\n",
                spec.id, spec.title, spec.spec_type, spec.status
            ));
            if !spec.summary.is_empty() {
                let preview: String = spec.summary.chars().take(80).collect();
                let ellipsis = if spec.summary.len() > 80 { "..." } else { "" };
                output.push_str(&format!("  {preview}{ellipsis}\n"));
            }
        }

        Ok(Self::success(output))
    }

    pub(super) async fn spec_approve(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};
        use cas_types::SpecStatus;
        use chrono::Utc;

        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        let mut spec = store
            .get(&id)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, format!("Spec not found: {e}")))?;

        spec.status = SpecStatus::Approved;
        spec.approved_at = Some(Utc::now());
        // cas-3c26: an MCP approval is an agent's; stamp it as one, with the
        // registry's role, so it never reads as the operator's sign-off.
        let approver = self.inner.get_agent_id().ok();
        let role = approver.as_deref().and_then(|id| {
            crate::store::open_agent_store(&self.inner.cas_root)
                .ok()?
                .get(id)
                .ok()
                .map(|agent| agent.role.to_string())
        });
        spec.approved_by = spec_approval_stamp(approver.as_deref(), role.as_deref());
        spec.updated_at = Utc::now();

        store.update(&spec).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to approve spec: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Approved spec: {} - {}\n\nStatus changed from {} to approved.",
            id, spec.title, "draft/under_review"
        )))
    }

    pub(super) async fn spec_reject(&self, req: SpecRequest) -> Result<CallToolResult, McpError> {
        use cas_store::{SpecStore, SqliteSpecStore};
        use cas_types::SpecStatus;
        use chrono::Utc;

        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let store = SqliteSpecStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spec store: {e}"),
            )
        })?;
        store.init().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to initialize spec store: {e}"),
            )
        })?;

        let mut spec = store
            .get(&id)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, format!("Spec not found: {e}")))?;

        spec.status = SpecStatus::Rejected;
        spec.updated_at = Utc::now();

        store.update(&spec).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to reject spec: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Rejected spec: {} - {}",
            id, spec.title
        )))
    }
}

/// cas-3c26: who approved a spec, as recorded. An MCP approval is always an
/// agent's, so it is stamped as one (`agent:<id> (<registry role>)`) and can
/// never read as the operator's sign-off.
pub(crate) fn spec_approval_stamp(agent_id: Option<&str>, role: Option<&str>) -> Option<String> {
    Some(format!(
        "agent:{} ({})",
        agent_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or("unknown"),
        role.filter(|role| !role.trim().is_empty())
            .unwrap_or("unregistered role")
    ))
}

#[cfg(test)]
mod spec_approval_tests {
    use super::spec_approval_stamp;

    #[test]
    fn cas_3c26_spec_approval_is_stamped_as_an_agents_never_the_operators() {
        assert_eq!(
            spec_approval_stamp(Some("sup-1"), Some("supervisor")).as_deref(),
            Some("agent:sup-1 (supervisor)")
        );
        assert_eq!(
            spec_approval_stamp(Some("w-2"), None).as_deref(),
            Some("agent:w-2 (unregistered role)")
        );
        assert_eq!(
            spec_approval_stamp(None, None).as_deref(),
            Some("agent:unknown (unregistered role)")
        );
    }
}
