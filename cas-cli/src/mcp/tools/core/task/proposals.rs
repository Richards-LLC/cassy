use crate::mcp::tools::core::imports::*;

use cas_store::{ExternalTaskDependencyProjection, ExternalTaskDependencyStore};

use crate::cloud::task_proposals::{
    CreateTaskProposalRequest, ProposalProvenance, TaskProposal, TaskProposalClient,
    authorize_registered_role, render_proposal,
};
use crate::cloud::{CloudConfig, canonical_id_from_config_toml};

const EXPLICIT_ORIGIN_REQUIRED: &str = "Cross-project task proposals require an explicit [project] canonical_id in .cas/config.toml; nothing is inferred from cwd, folder name, or git remote.";

impl CasCore {
    fn proposal_authority(&self) -> Result<cas_types::Agent, McpError> {
        // This lookup and registered-store role check intentionally happen
        // before CloudConfig loading or any network-capable client exists.
        let agent_id = self.get_agent_id().map_err(|_| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                crate::cloud::task_proposals::ROLE_REQUIREMENT,
            )
        })?;
        let agent = self.open_agent_store()?.get(&agent_id).map_err(|_| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                crate::cloud::task_proposals::ROLE_REQUIREMENT,
            )
        })?;
        authorize_registered_role(Some(agent.role))
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        Ok(agent)
    }

    fn proposal_client_after_authority(&self) -> Result<TaskProposalClient, McpError> {
        let project_config = CloudConfig::load_from_cas_dir(&self.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to load project cloud configuration: {error}"),
            )
        })?;
        let user_config = CloudConfig::load_user().unwrap_or_default();
        let token = project_config
            .token
            .clone()
            .or(user_config.token.clone())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Cross-project task proposals require an authenticated cloud bearer token.",
                )
            })?;
        let team_id = project_config
            .active_team_id_with_user_config(Some(&user_config))
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Cross-project task creation requires membership in a team shared by the origin and target projects.",
                )
            })?;
        let endpoint = if project_config.endpoint.trim().is_empty() {
            user_config.endpoint
        } else {
            project_config.endpoint
        };
        Ok(TaskProposalClient::new(endpoint, token, team_id))
    }

    fn explicit_local_project(&self) -> Result<String, McpError> {
        canonical_id_from_config_toml(&self.cas_root)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, EXPLICIT_ORIGIN_REQUIRED))
    }

    fn require_local_project(&self, requested: &str) -> Result<String, McpError> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cross-project task proposals require an explicit project; nothing is inferred from cwd.",
            ));
        }
        let local = self.explicit_local_project()?;
        if requested != local {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "Requested project `{requested}` does not match this CAS database's explicit canonical project `{local}`."
                ),
            ));
        }
        Ok(local)
    }

    pub(crate) async fn cas_task_proposal_create(
        &self,
        req: TaskCreateRequest,
        target_project: &str,
        blocks_origin_task_id: Option<&str>,
    ) -> Result<CallToolResult, McpError> {
        let agent = self.proposal_authority()?;
        let origin_project = self.explicit_local_project()?;
        let target_project = target_project.trim();
        if target_project.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cross-project task proposals require an explicit target project; nothing is inferred from cwd.",
            ));
        }
        let client = self.proposal_client_after_authority()?;

        if let Some(origin_task_id) = blocks_origin_task_id {
            self.open_task_store()?
                .get(origin_task_id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("Origin blocker task not found: {error}"),
                    )
                })?;
        }

        let request = CreateTaskProposalRequest {
            client_request_id: uuid::Uuid::new_v4().to_string(),
            origin_project_canonical_id: origin_project,
            target_project_canonical_id: target_project.to_string(),
            origin_session_id: agent
                .cc_session_id
                .clone()
                .unwrap_or_else(|| agent.id.clone()),
            origin_agent_id: agent.id.clone(),
            origin_agent_name: Some(agent.name.clone()),
            origin_agent_role: agent.role.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            client_build: option_env!("CAS_GIT_HASH").unwrap_or("unknown").to_string(),
            task: serde_json::json!({
                "title": req.title,
                "description": req.description.unwrap_or_default(),
                "priority": req.priority,
                "task_type": req.task_type,
                "labels": req.labels.as_deref().unwrap_or_default().split(',')
                    .map(str::trim).filter(|label| !label.is_empty()).collect::<Vec<_>>(),
                "design": req.design.unwrap_or_default(),
                "acceptance_criteria": req.acceptance_criteria.unwrap_or_default(),
                "external_ref": req.external_ref.unwrap_or_default(),
            }),
            blocks_origin_task_id: blocks_origin_task_id.map(ToString::to_string),
        };
        let response = tokio::task::spawn_blocking(move || client.create(&request))
            .await
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Cloud proposal request panicked: {error}"),
                )
            })?
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;

        if let Some(origin_task_id) = blocks_origin_task_id {
            let projection = ExternalTaskDependencyProjection {
                origin_task_id: origin_task_id.to_string(),
                proposal_id: response.proposal_id.clone(),
                target_project_canonical_id: target_project.to_string(),
                target_task_id: response.target_task_id.clone(),
                proposal_state: response.state.clone(),
                target_task_status: None,
                resolution_state: "unresolved".to_string(),
                resolved_at: None,
            };
            ExternalTaskDependencyStore::open(&self.cas_root)
                .and_then(|store| store.upsert(&projection))
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, format!("Proposal was created, but local external blocker projection failed: {error}")))?;
            let task_store = self.open_task_store()?;
            let mut origin = task_store
                .get(origin_task_id)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
            if origin.status == TaskStatus::Open {
                origin.status = TaskStatus::Blocked;
                origin.updated_at = chrono::Utc::now();
                task_store.update(&origin).map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Proposal was created, but marking origin task blocked failed: {error}"
                        ),
                    )
                })?;
            }
        }

        let proposal = proposal_from_create(response.provenance, response.state);
        Ok(Self::success(format!(
            "Created cross-project task proposal. No foreign task was inserted locally.\n\n{}",
            render_proposal(&proposal)
        )))
    }

    pub(crate) async fn cas_task_proposal_inbox(
        &self,
        target_project: &str,
    ) -> Result<CallToolResult, McpError> {
        self.proposal_authority()?;
        let target_project = self.require_local_project(target_project)?;
        let client = self.proposal_client_after_authority()?;
        let proposals = tokio::task::spawn_blocking(move || client.inbox_all(&target_project))
            .await
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        if proposals.is_empty() {
            return Ok(Self::success(
                "No proposed cross-project tasks in this project inbox.",
            ));
        }
        Ok(Self::success(
            proposals
                .iter()
                .map(render_proposal)
                .collect::<Vec<_>>()
                .join("\n---\n"),
        ))
    }

    pub(crate) async fn cas_task_proposal_accept(
        &self,
        proposal_id: &str,
        target_project: &str,
    ) -> Result<CallToolResult, McpError> {
        self.proposal_authority()?;
        let target_project = self.require_local_project(target_project)?;
        let proposal_id = proposal_id.to_string();
        let client = self.proposal_client_after_authority()?;
        let proposal =
            tokio::task::spawn_blocking(move || client.accept(&proposal_id, &target_project))
                .await
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
                .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        Ok(Self::success(format!(
            "Accepted proposal atomically as one open target task.\n\n{}",
            render_proposal(&proposal)
        )))
    }

    pub(crate) async fn cas_task_proposal_reject(
        &self,
        proposal_id: &str,
        target_project: &str,
        reason: Option<&str>,
    ) -> Result<CallToolResult, McpError> {
        self.proposal_authority()?;
        let target_project = self.require_local_project(target_project)?;
        let proposal_id = proposal_id.to_string();
        let reason = reason.map(ToString::to_string);
        let client = self.proposal_client_after_authority()?;
        let proposal = tokio::task::spawn_blocking(move || {
            client.reject(&proposal_id, &target_project, reason.as_deref())
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        Ok(Self::success(format!(
            "Rejected proposal; no task was materialized.\n\n{}",
            render_proposal(&proposal)
        )))
    }

    pub(crate) async fn cas_task_proposal_reconcile(
        &self,
        origin_project: &str,
    ) -> Result<CallToolResult, McpError> {
        self.proposal_authority()?;
        let origin_project = self.require_local_project(origin_project)?;
        let projection_store = ExternalTaskDependencyStore::open(&self.cas_root)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let cursor = projection_store
            .cursor(&origin_project)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let client = self.proposal_client_after_authority()?;
        let request_project = origin_project.clone();
        let request_cursor = cursor.clone();
        let feed = tokio::task::spawn_blocking(move || {
            client.dependency_feed_all(&request_project, request_cursor.as_deref())
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;

        let task_store = self.open_task_store()?;
        let mut touched = std::collections::BTreeSet::new();
        for dependency in &feed.dependencies {
            touched.insert(dependency.origin_task_id.clone());
            projection_store
                .upsert(&ExternalTaskDependencyProjection {
                    origin_task_id: dependency.origin_task_id.clone(),
                    proposal_id: dependency.proposal_id.clone(),
                    target_project_canonical_id: dependency.target_project_canonical_id.clone(),
                    target_task_id: dependency.target_task_id.clone(),
                    proposal_state: dependency.proposal_state.clone(),
                    target_task_status: dependency.target_task_status.clone(),
                    resolution_state: dependency.resolution_state.clone(),
                    resolved_at: dependency.resolved_at.clone(),
                })
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        }
        projection_store
            .set_cursor(&origin_project, feed.cursor.as_deref())
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;

        let mut reopened = Vec::new();
        let mut rejected = Vec::new();
        for task_id in touched {
            let blockers = projection_store
                .list_blocking_for_task(&task_id)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
            rejected.extend(
                blockers
                    .iter()
                    .filter(|dependency| dependency.resolution_state == "handoff_rejected")
                    .map(|_| task_id.clone()),
            );
            if !blockers.is_empty() {
                if let Ok(mut task) = task_store.get(&task_id) {
                    if task.status == TaskStatus::Open {
                        task.status = TaskStatus::Blocked;
                        task.updated_at = chrono::Utc::now();
                        task_store.update(&task).map_err(|error| {
                            Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                        })?;
                    }
                }
                continue;
            }
            if blockers.is_empty()
                && task_store
                    .get_blockers(&task_id)
                    .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
                    .is_empty()
            {
                if let Ok(mut task) = task_store.get(&task_id) {
                    if task.status == TaskStatus::Blocked {
                        task.status = TaskStatus::Open;
                        task.updated_at = chrono::Utc::now();
                        task_store.update(&task).map_err(|error| {
                            Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                        })?;
                        reopened.push(task_id);
                    }
                }
            }
        }
        rejected.sort();
        rejected.dedup();
        Ok(Self::success(format!(
            "Reconciled {} external dependency signal(s) for explicit project `{}`. Reopened: {}. Rejected handoffs still blocking: {}.",
            feed.dependencies.len(),
            origin_project,
            if reopened.is_empty() {
                "none".into()
            } else {
                reopened.join(", ")
            },
            if rejected.is_empty() {
                "none".into()
            } else {
                rejected.join(", ")
            },
        )))
    }
}

fn proposal_from_create(provenance: ProposalProvenance, state: String) -> TaskProposal {
    let server = &provenance.server_attested;
    TaskProposal {
        proposal_id: server.proposal_id.clone(),
        target_task_id: server.target_task_id.clone(),
        state,
        origin_project_canonical_id: Some(server.origin_project_canonical_id.clone()),
        target_project_canonical_id: Some(server.target_project_canonical_id.clone()),
        received_at: Some(server.received_at.clone()),
        decided_by_user_id: None,
        decided_at: None,
        rejection_reason: None,
        task: None,
        provenance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> TaskCreateRequest {
        TaskCreateRequest {
            title: "Foreign work".into(),
            description: Some("Description".into()),
            priority: 2,
            task_type: "task".into(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
            depth: None,
        }
    }

    fn core_with_role(role: cas_types::AgentRole) -> (tempfile::TempDir, CasCore) {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(temp.path()).unwrap();
        let store = crate::store::open_agent_store(&cas_root).unwrap();
        let mut agent = cas_types::Agent::new("proposal-test-agent".into(), "proposal-test".into());
        agent.role = role;
        store.register(&agent).unwrap();
        let core = CasCore::with_daemon(cas_root, None, None);
        core.set_agent_id_for_testing(agent.id);
        (temp, core)
    }

    #[tokio::test]
    async fn worker_is_refused_before_cloud_config_is_loaded() {
        let (_temp, core) = core_with_role(cas_types::AgentRole::Worker);
        let error = core
            .cas_task_proposal_create(request(), "target-project", None)
            .await
            .unwrap_err();
        assert_eq!(
            error.message.as_ref(),
            crate::cloud::task_proposals::ROLE_REQUIREMENT
        );
    }

    #[tokio::test]
    async fn supervisor_without_explicit_origin_never_falls_back_to_cwd_or_git() {
        let (_temp, core) = core_with_role(cas_types::AgentRole::Supervisor);
        let error = core
            .cas_task_proposal_create(request(), "target-project", None)
            .await
            .unwrap_err();
        assert_eq!(error.message.as_ref(), EXPLICIT_ORIGIN_REQUIRED);
    }

    #[tokio::test]
    async fn task_list_scope_names_project_and_rejects_global() {
        let (_temp, core) = core_with_role(cas_types::AgentRole::Supervisor);
        let task_store = core.open_task_store().unwrap();
        task_store
            .add(&cas_types::Task::new(
                "cas-scope".into(),
                "Scope truth".into(),
            ))
            .unwrap();
        let project = core
            .cas_task_list(Parameters(TaskListRequest {
                limit: None,
                scope: "project".into(),
                status: None,
                task_type: None,
                label: None,
                assignee: None,
                epic: None,
                sort: None,
                sort_order: None,
            }))
            .await
            .unwrap();
        let rmcp::model::RawContent::Text(project_text) = &project.content[0].raw else {
            panic!("task list must return text")
        };
        let project_text = project_text.text.as_str();
        assert!(project_text.contains("Scope: project `"));
        assert!(project_text.contains("current CAS database"));

        let global = core
            .cas_task_list(Parameters(TaskListRequest {
                limit: None,
                scope: "global".into(),
                status: None,
                task_type: None,
                label: None,
                assignee: None,
                epic: None,
                sort: None,
                sort_order: None,
            }))
            .await
            .unwrap_err();
        assert!(global.message.contains("Global task scope is unsupported"));
    }
}
