//! Dedicated client boundary for cloud-owned cross-project task proposals.
//!
//! Pending proposals are deliberately separate from ordinary task sync.  This
//! module models the proposal row as authoritative and keeps materialized task
//! JSON advisory, matching the contract in
//! `docs/specs/2026-08-11-cross-project-task-proposals.md`.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::AgentRole;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const ROLE_REQUIREMENT: &str =
    "Cross-project task creation requires a registered supervisor or director session.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateTaskProposalRequest {
    pub client_request_id: String,
    pub origin_project_canonical_id: String,
    pub target_project_canonical_id: String,
    pub origin_session_id: String,
    pub origin_agent_id: String,
    pub origin_agent_name: Option<String>,
    pub origin_agent_role: String,
    pub client_version: String,
    pub client_build: String,
    pub task: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks_origin_task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerAttestedProvenance {
    pub proposal_id: String,
    pub target_task_id: String,
    pub creator_user_id: String,
    pub team_id: String,
    pub origin_project_canonical_id: String,
    pub target_project_canonical_id: String,
    pub received_at: String,
    pub client_request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientAssertedProvenance {
    pub origin_session_id: Option<String>,
    pub origin_agent_id: Option<String>,
    pub origin_agent_name: Option<String>,
    pub origin_agent_role: Option<String>,
    pub client_version: Option<String>,
    pub client_build: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposalProvenance {
    pub server_attested: ServerAttestedProvenance,
    pub client_asserted: ClientAssertedProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskProposal {
    pub proposal_id: String,
    pub target_task_id: String,
    pub state: String,
    #[serde(default)]
    pub origin_project_canonical_id: Option<String>,
    #[serde(default)]
    pub target_project_canonical_id: Option<String>,
    #[serde(default)]
    pub received_at: Option<String>,
    #[serde(default)]
    pub decided_by_user_id: Option<String>,
    #[serde(default)]
    pub decided_at: Option<String>,
    #[serde(default)]
    pub rejection_reason: Option<String>,
    #[serde(default)]
    pub task: Option<serde_json::Value>,
    pub provenance: ProposalProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateTaskProposalResponse {
    pub proposal_id: String,
    pub target_task_id: String,
    pub state: String,
    pub provenance: ProposalProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalTaskDependency {
    pub origin_task_id: String,
    pub proposal_id: String,
    pub target_task_id: String,
    pub proposal_state: String,
    pub target_task_status: Option<String>,
    pub resolution_state: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyFeed {
    pub dependencies: Vec<ExternalTaskDependency>,
    /// The last server-issued high-watermark. It is opaque to CAS.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProposalError {
    Authorization(String),
    Http { status: u16, message: String },
    Transport(String),
    Decode(String),
    Contract(String),
}

impl fmt::Display for TaskProposalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorization(message)
            | Self::Transport(message)
            | Self::Decode(message)
            | Self::Contract(message) => f.write_str(message),
            Self::Http { status, message } => write!(f, "Cloud returned HTTP {status}: {message}"),
        }
    }
}

impl std::error::Error for TaskProposalError {}

/// Fail-closed local role gate. `None` means the caller was not found in the
/// registered CAS agent store; environment strings alone are not authority.
pub fn authorize_registered_role(role: Option<AgentRole>) -> Result<AgentRole, TaskProposalError> {
    match role {
        Some(AgentRole::Supervisor | AgentRole::Director) => Err(TaskProposalError::Contract(
            "red boundary: supervisor/director authorization is not implemented".to_string(),
        )),
        _ => Err(TaskProposalError::Authorization(
            ROLE_REQUIREMENT.to_string(),
        )),
    }
}

#[derive(Debug, Clone)]
pub struct TaskProposalClient {
    endpoint: String,
    token: String,
    team_id: String,
    timeout: Duration,
}

impl TaskProposalClient {
    pub fn new(
        endpoint: impl Into<String>,
        token: impl Into<String>,
        team_id: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            token: token.into(),
            team_id: team_id.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn create(
        &self,
        _request: &CreateTaskProposalRequest,
    ) -> Result<CreateTaskProposalResponse, TaskProposalError> {
        let _ = (&self.endpoint, &self.token, &self.team_id, self.timeout);
        Err(TaskProposalError::Contract(
            "red boundary: proposal create is not implemented".to_string(),
        ))
    }

    pub fn inbox_all(&self, _target_project: &str) -> Result<Vec<TaskProposal>, TaskProposalError> {
        Err(TaskProposalError::Contract(
            "red boundary: proposal inbox is not implemented".to_string(),
        ))
    }

    pub fn accept(
        &self,
        _proposal_id: &str,
        _target_project: &str,
    ) -> Result<TaskProposal, TaskProposalError> {
        Err(TaskProposalError::Contract(
            "red boundary: proposal accept is not implemented".to_string(),
        ))
    }

    pub fn reject(
        &self,
        _proposal_id: &str,
        _target_project: &str,
        _reason: Option<&str>,
    ) -> Result<TaskProposal, TaskProposalError> {
        Err(TaskProposalError::Contract(
            "red boundary: proposal reject is not implemented".to_string(),
        ))
    }

    pub fn dependency_feed_all(
        &self,
        _origin_project: &str,
        _since: Option<&str>,
    ) -> Result<DependencyFeed, TaskProposalError> {
        Err(TaskProposalError::Contract(
            "red boundary: dependency feed is not implemented".to_string(),
        ))
    }
}

pub fn render_proposal(_proposal: &TaskProposal) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{
        body_json, header, method, path, query_param, query_param_is_missing,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provenance() -> serde_json::Value {
        json!({
            "server_attested": {
                "proposal_id": "proposal-1",
                "target_task_id": "cas-0123456789abcdef",
                "creator_user_id": "user-1",
                "team_id": "team-1",
                "origin_project_canonical_id": "origin-project",
                "target_project_canonical_id": "target-project",
                "received_at": "2026-08-13T15:00:00.000Z",
                "client_request_id": "request-1"
            },
            "client_asserted": {
                "origin_session_id": "session-1",
                "origin_agent_id": "agent-1",
                "origin_agent_name": "supervisor-one",
                "origin_agent_role": "supervisor",
                "client_version": "2.48.0",
                "client_build": "deadbeef"
            }
        })
    }

    fn proposal(state: &str) -> serde_json::Value {
        json!({
            "proposal_id": "proposal-1",
            "target_task_id": "cas-0123456789abcdef",
            "state": state,
            "origin_project_canonical_id": "origin-project",
            "target_project_canonical_id": "target-project",
            "received_at": "2026-08-13T15:00:00.000Z",
            "decided_by_user_id": null,
            "decided_at": null,
            "rejection_reason": null,
            "task": {"title": "Cross-project work", "status": "open"},
            "provenance": provenance()
        })
    }

    fn create_request() -> CreateTaskProposalRequest {
        CreateTaskProposalRequest {
            client_request_id: "request-1".to_string(),
            origin_project_canonical_id: "origin-project".to_string(),
            target_project_canonical_id: "target-project".to_string(),
            origin_session_id: "session-1".to_string(),
            origin_agent_id: "agent-1".to_string(),
            origin_agent_name: Some("supervisor-one".to_string()),
            origin_agent_role: "supervisor".to_string(),
            client_version: "2.48.0".to_string(),
            client_build: "deadbeef".to_string(),
            task: json!({"title": "Cross-project work", "priority": 1}),
            blocks_origin_task_id: Some("cas-abcd".to_string()),
        }
    }

    fn client(server: &MockServer) -> TaskProposalClient {
        TaskProposalClient::new(server.uri(), "test-token", "team-1")
    }

    #[test]
    fn role_gate_accepts_only_registered_supervisor_or_director() {
        assert_eq!(
            authorize_registered_role(Some(AgentRole::Supervisor)).unwrap(),
            AgentRole::Supervisor
        );
        assert_eq!(
            authorize_registered_role(Some(AgentRole::Director)).unwrap(),
            AgentRole::Director
        );
        for role in [None, Some(AgentRole::Worker), Some(AgentRole::Standard)] {
            let error = authorize_registered_role(role).unwrap_err();
            assert_eq!(error.to_string(), ROLE_REQUIREMENT);
        }
    }

    #[tokio::test]
    async fn create_uses_dedicated_endpoint_and_exact_wire_body() {
        let server = MockServer::start().await;
        let request = create_request();
        Mock::given(method("POST"))
            .and(path("/api/teams/team-1/task-proposals"))
            .and(header("Authorization", "Bearer test-token"))
            .and(body_json(serde_json::to_value(&request).unwrap()))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "proposal_id": "proposal-1",
                "target_task_id": "cas-0123456789abcdef",
                "state": "proposed",
                "provenance": provenance()
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = tokio::task::spawn_blocking(move || client(&server).create(&request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.target_task_id, "cas-0123456789abcdef");
        assert_eq!(result.provenance.server_attested.creator_user_id, "user-1");
    }

    #[tokio::test]
    async fn inbox_follows_opaque_cursor_to_exhaustion() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/task-proposals"))
            .and(query_param("target_project_id", "target-project"))
            .and(query_param("state", "proposed"))
            .and(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "proposals": [proposal("proposed")],
                "next_cursor": "opaque/one+two="
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/task-proposals"))
            .and(query_param("target_project_id", "target-project"))
            .and(query_param("state", "proposed"))
            .and(query_param("cursor", "opaque/one+two="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "proposals": [],
                "next_cursor": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let proposals =
            tokio::task::spawn_blocking(move || client(&server).inbox_all("target-project"))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(proposals.len(), 1);
    }

    #[tokio::test]
    async fn accept_and_reject_send_explicit_target_and_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/teams/team-1/task-proposals/proposal-1/accept"))
            .and(body_json(json!({"target_project_id": "target-project"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(proposal("accepted")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/teams/team-1/task-proposals/proposal-2/reject"))
            .and(body_json(
                json!({"target_project_id": "target-project", "reason": "not owned here"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json({
                let mut value = proposal("rejected");
                value["proposal_id"] = json!("proposal-2");
                value["rejection_reason"] = json!("not owned here");
                value.as_object_mut().unwrap().remove("task");
                value
            }))
            .expect(1)
            .mount(&server)
            .await;

        let (accepted, rejected) = tokio::task::spawn_blocking(move || {
            let client = client(&server);
            (
                client.accept("proposal-1", "target-project"),
                client.reject("proposal-2", "target-project", Some("not owned here")),
            )
        })
        .await
        .unwrap();
        assert_eq!(accepted.unwrap().state, "accepted");
        assert_eq!(
            rejected.unwrap().rejection_reason.as_deref(),
            Some("not owned here")
        );
    }

    #[tokio::test]
    async fn dependency_feed_follows_opaque_cursor_and_stops_on_stable_loop() {
        let server = MockServer::start().await;
        let row = json!({
            "origin_task_id": "cas-origin",
            "proposal_id": "proposal-1",
            "target_task_id": "cas-0123456789abcdef",
            "proposal_state": "rejected",
            "target_task_status": null,
            "resolution_state": "handoff_rejected",
            "resolved_at": null
        });
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/cross-project-task-dependencies"))
            .and(query_param("origin_project_id", "origin-project"))
            .and(query_param("since", "opaque-start"))
            .and(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "dependencies": [row],
                "cursor": "server-watermark",
                "next_cursor": "opaque-next"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/cross-project-task-dependencies"))
            .and(query_param("origin_project_id", "origin-project"))
            .and(query_param("cursor", "opaque-next"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "dependencies": [],
                "cursor": "server-watermark",
                "next_cursor": "opaque-next"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let feed = tokio::task::spawn_blocking(move || {
            client(&server).dependency_feed_all("origin-project", Some("opaque-start"))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(feed.dependencies.len(), 1);
        assert_eq!(feed.dependencies[0].resolution_state, "handoff_rejected");
        assert_eq!(feed.cursor.as_deref(), Some("server-watermark"));
    }

    #[test]
    fn provenance_rendering_keeps_attested_and_asserted_fields_visibly_separate() {
        let parsed: TaskProposal = serde_json::from_value(proposal("proposed")).unwrap();
        let rendered = render_proposal(&parsed);
        assert!(rendered.contains("Server-attested provenance"));
        assert!(rendered.contains("creator_user_id: user-1"));
        assert!(rendered.contains("Client-asserted provenance"));
        assert!(rendered.contains("origin_agent_role: supervisor"));
    }
}
