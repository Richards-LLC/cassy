//! Dedicated client boundary for cloud-owned cross-project task proposals.
//!
//! Pending proposals are deliberately separate from ordinary task sync.  This
//! module models the proposal row as authoritative and keeps materialized task
//! JSON advisory, matching the contract in
//! `docs/specs/2026-08-11-cross-project-task-proposals.md`.

use std::collections::HashSet;
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
    #[serde(default)]
    pub target_project_canonical_id: String,
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
        Some(role @ (AgentRole::Supervisor | AgentRole::Director)) => Ok(role),
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
        request: &CreateTaskProposalRequest,
    ) -> Result<CreateTaskProposalResponse, TaskProposalError> {
        let url = self.team_url("task-proposals");
        self.post_json(&url, request)
    }

    pub fn inbox_all(&self, target_project: &str) -> Result<Vec<TaskProposal>, TaskProposalError> {
        let target = required_explicit("target project", target_project)?;
        let mut proposals = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();

        loop {
            let mut url = format!(
                "{}?target_project_id={}&state=proposed",
                self.team_url("task-proposals"),
                urlencoding::encode(target)
            );
            if let Some(value) = cursor.as_deref() {
                url.push_str("&cursor=");
                url.push_str(&urlencoding::encode(value));
            }
            let page: ProposalPage = self.get_json(&url)?;
            let continuation = page.continuation();
            proposals.extend(page.proposals);
            let Some(next) = continuation else {
                break;
            };
            // Cursor contents are opaque. Equality is the only operation CAS
            // performs, solely to prevent a stable/repeated token loop.
            if next.is_empty()
                || next == cursor.as_deref().unwrap_or_default()
                || !seen.insert(next.clone())
            {
                break;
            }
            cursor = Some(next);
        }
        Ok(proposals)
    }

    pub fn accept(
        &self,
        proposal_id: &str,
        target_project: &str,
    ) -> Result<TaskProposal, TaskProposalError> {
        self.decide(proposal_id, target_project, "accept", None)
    }

    pub fn reject(
        &self,
        proposal_id: &str,
        target_project: &str,
        reason: Option<&str>,
    ) -> Result<TaskProposal, TaskProposalError> {
        self.decide(proposal_id, target_project, "reject", reason)
    }

    pub fn dependency_feed_all(
        &self,
        origin_project: &str,
        since: Option<&str>,
    ) -> Result<DependencyFeed, TaskProposalError> {
        let origin = required_explicit("origin project", origin_project)?;
        let mut dependencies = Vec::new();
        let mut page_cursor: Option<String> = None;
        let mut watermark = since.map(ToString::to_string);
        let mut seen = HashSet::new();
        if let Some(value) = since {
            seen.insert(value.to_string());
        }

        loop {
            let mut url = format!(
                "{}?origin_project_id={}",
                self.team_url("cross-project-task-dependencies"),
                urlencoding::encode(origin)
            );
            if let Some(value) = page_cursor.as_deref() {
                url.push_str("&cursor=");
                url.push_str(&urlencoding::encode(value));
            } else if let Some(value) = since {
                // `since` is the current production high-watermark contract.
                // It is passed through byte-for-byte and never parsed.
                url.push_str("&since=");
                url.push_str(&urlencoding::encode(value));
            }

            let page: DependencyPage = self.get_json(&url)?;
            let continuation = page.continuation();
            dependencies.extend(page.dependencies);
            if page.cursor.is_some() {
                watermark = page.cursor.clone();
            }
            let Some(next) = continuation else {
                break;
            };
            if next.is_empty()
                || next == page_cursor.as_deref().unwrap_or_default()
                || !seen.insert(next.clone())
            {
                break;
            }
            page_cursor = Some(next);
        }
        Ok(DependencyFeed {
            dependencies,
            cursor: watermark,
        })
    }

    fn decide(
        &self,
        proposal_id: &str,
        target_project: &str,
        decision: &str,
        reason: Option<&str>,
    ) -> Result<TaskProposal, TaskProposalError> {
        let proposal = required_explicit("proposal id", proposal_id)?;
        let target = required_explicit("target project", target_project)?;
        let url = format!(
            "{}/api/teams/{}/task-proposals/{}/{}",
            self.endpoint,
            urlencoding::encode(&self.team_id),
            urlencoding::encode(proposal),
            decision
        );
        let mut body = serde_json::json!({ "target_project_id": target });
        if let Some(reason) = reason {
            body["reason"] = serde_json::Value::String(reason.to_string());
        }
        self.post_json(&url, &body)
    }

    fn team_url(&self, resource: &str) -> String {
        format!(
            "{}/api/teams/{}/{}",
            self.endpoint,
            urlencoding::encode(&self.team_id),
            resource
        )
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, TaskProposalError> {
        let response = ureq::get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(self.timeout)
            .call();
        decode_response(response)
    }

    fn post_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &impl Serialize,
    ) -> Result<T, TaskProposalError> {
        let response = ureq::post(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(self.timeout)
            .send_json(body);
        decode_response(response)
    }
}

#[derive(Debug, Deserialize)]
struct ProposalPage {
    #[serde(default)]
    proposals: Vec<TaskProposal>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    has_more: Option<bool>,
}

impl ProposalPage {
    fn continuation(&self) -> Option<String> {
        self.next_cursor.clone().or_else(|| {
            self.has_more
                .unwrap_or(false)
                .then(|| self.cursor.clone())
                .flatten()
        })
    }
}

#[derive(Debug, Deserialize)]
struct DependencyPage {
    #[serde(default)]
    dependencies: Vec<ExternalTaskDependency>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    has_more: Option<bool>,
}

impl DependencyPage {
    fn continuation(&self) -> Option<String> {
        self.next_cursor.clone().or_else(|| {
            self.has_more
                .unwrap_or(false)
                .then(|| self.cursor.clone())
                .flatten()
        })
    }
}

fn required_explicit<'a>(field: &str, value: &'a str) -> Result<&'a str, TaskProposalError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(TaskProposalError::Contract(format!(
            "Cross-project task proposals require an explicit {field}; nothing is inferred from cwd."
        )));
    }
    Ok(value)
}

fn decode_response<T: for<'de> Deserialize<'de>>(
    response: Result<ureq::Response, ureq::Error>,
) -> Result<T, TaskProposalError> {
    match response {
        Ok(response) => response
            .into_json::<T>()
            .map_err(|error| TaskProposalError::Decode(format!("Invalid cloud response: {error}"))),
        Err(ureq::Error::Status(status, response)) => {
            let raw = response.into_string().unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|body| {
                    body.get("error")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| body.get("message").and_then(serde_json::Value::as_str))
                        .map(ToString::to_string)
                })
                .filter(|message| !message.is_empty())
                .unwrap_or(raw);
            Err(TaskProposalError::Http { status, message })
        }
        Err(ureq::Error::Transport(error)) => Err(TaskProposalError::Transport(format!(
            "Cloud request failed: {error}"
        ))),
    }
}

pub fn render_proposal(proposal: &TaskProposal) -> String {
    let server = &proposal.provenance.server_attested;
    let client = &proposal.provenance.client_asserted;
    let mut output = format!(
        "Proposal: {}\nTarget task: {}\nState: {}\n\nServer-attested provenance:\n  creator_user_id: {}\n  team_id: {}\n  origin_project_canonical_id: {}\n  target_project_canonical_id: {}\n  received_at: {}\n  client_request_id: {}\n\nClient-asserted provenance:\n",
        proposal.proposal_id,
        proposal.target_task_id,
        proposal.state,
        server.creator_user_id,
        server.team_id,
        server.origin_project_canonical_id,
        server.target_project_canonical_id,
        server.received_at,
        server.client_request_id,
    );
    for (name, value) in [
        ("origin_session_id", client.origin_session_id.as_deref()),
        ("origin_agent_id", client.origin_agent_id.as_deref()),
        ("origin_agent_name", client.origin_agent_name.as_deref()),
        ("origin_agent_role", client.origin_agent_role.as_deref()),
        ("client_version", client.client_version.as_deref()),
        ("client_build", client.client_build.as_deref()),
    ] {
        output.push_str(&format!(
            "  {name}: {}\n",
            value.unwrap_or("(not asserted)")
        ));
    }
    output
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
