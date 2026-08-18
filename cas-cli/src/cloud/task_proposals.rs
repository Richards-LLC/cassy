//! Dedicated client boundary for cloud-owned cross-project task proposals.
//!
//! Pending proposals are deliberately separate from ordinary task sync.  This
//! module models the proposal row as authoritative and keeps materialized task
//! JSON advisory, matching the contract in
//! `docs/specs/2026-08-11-cross-project-task-proposals.md`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::AgentRole;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const ROLE_REQUIREMENT: &str =
    "Cross-project task creation requires a registered supervisor or director session.";

/// Cloud pagination is additive and opt-in: a request carrying neither `limit`
/// nor `cursor` is answered in the legacy shape with no `next_cursor`, which
/// would silently truncate a drain at the server default. Cassy therefore always
/// sends an explicit in-range `limit` (server accepts 1..=500) so every listing
/// receives a `next_cursor` it can follow to exhaustion.
const PAGE_LIMIT_PARAM: &str = "100";

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
    /// The last server-issued high-watermark. It is opaque to Cassy.
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
/// registered Cassy agent store; environment strings alone are not authority.
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
        let response = self.post_json(&url, request)?;
        validate_create_response(&response, &self.team_id, request)?;
        Ok(response)
    }

    pub fn inbox_all(&self, target_project: &str) -> Result<Vec<TaskProposal>, TaskProposalError> {
        let target = required_explicit("target project", target_project)?;
        let mut proposals = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();

        loop {
            let mut url = format!(
                "{}?target_project_id={}&state=proposed&limit={PAGE_LIMIT_PARAM}",
                self.team_url("task-proposals"),
                urlencoding::encode(target)
            );
            if let Some(value) = cursor.as_deref() {
                url.push_str("&cursor=");
                url.push_str(&urlencoding::encode(value));
            }
            let page: ProposalPage = self.get_json(&url)?;
            for proposal in &page.proposals {
                validate_task_proposal(
                    proposal,
                    &self.team_id,
                    None,
                    Some(target),
                    Some("proposed"),
                )?;
            }
            let continuation = page.continuation();
            proposals.extend(page.proposals);
            let Some(next) = continuation else {
                break;
            };
            // Cursor contents are opaque. Equality is the only operation Cassy
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
                "{}?origin_project_id={}&limit={PAGE_LIMIT_PARAM}",
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
            for dependency in &page.dependencies {
                validate_external_dependency(dependency, origin)?;
            }
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
            dependencies: dedupe_by_proposal_id(dependencies),
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
        let proposal = self.post_json(&url, &body)?;
        validate_task_proposal(
            &proposal,
            &self.team_id,
            None,
            Some(target),
            Some(if decision == "accept" {
                "accepted"
            } else {
                "rejected"
            }),
        )?;
        if proposal.proposal_id != proposal_id {
            return Err(contract(
                "triage response proposal ID did not match the requested proposal ID",
            ));
        }
        Ok(proposal)
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

/// `since=` feed reads deliberately replay a 5-second safety window so a
/// late-committing transaction cannot fall permanently behind an observed
/// cursor. That makes duplicates an expected, contractual outcome rather than a
/// server bug, so Cassy collapses them by `proposal_id`. The most recently
/// observed row wins (feed order is the server's, and later rows carry the
/// newer resolution state), while first-seen ordering is preserved so the
/// reconciler reports a stable, truthful edge count.
fn dedupe_by_proposal_id(dependencies: Vec<ExternalTaskDependency>) -> Vec<ExternalTaskDependency> {
    let mut position: HashMap<String, usize> = HashMap::new();
    let mut deduped: Vec<ExternalTaskDependency> = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        match position.get(&dependency.proposal_id) {
            Some(&index) => deduped[index] = dependency,
            None => {
                position.insert(dependency.proposal_id.clone(), deduped.len());
                deduped.push(dependency);
            }
        }
    }
    deduped
}

fn validate_create_response(
    response: &CreateTaskProposalResponse,
    team_id: &str,
    request: &CreateTaskProposalRequest,
) -> Result<(), TaskProposalError> {
    if response.state != "proposed" {
        return Err(contract("create returned a non-proposed state"));
    }
    let proposal = proposal_from_create_response(response);
    validate_task_proposal(
        &proposal,
        team_id,
        Some(&request.origin_project_canonical_id),
        Some(&request.target_project_canonical_id),
        Some("proposed"),
    )?;
    if response.provenance.server_attested.client_request_id != request.client_request_id {
        return Err(contract(
            "create response client_request_id did not match request",
        ));
    }
    Ok(())
}

fn proposal_from_create_response(response: &CreateTaskProposalResponse) -> TaskProposal {
    let server = &response.provenance.server_attested;
    TaskProposal {
        proposal_id: response.proposal_id.clone(),
        target_task_id: response.target_task_id.clone(),
        state: response.state.clone(),
        origin_project_canonical_id: Some(server.origin_project_canonical_id.clone()),
        target_project_canonical_id: Some(server.target_project_canonical_id.clone()),
        received_at: Some(server.received_at.clone()),
        decided_by_user_id: None,
        decided_at: None,
        rejection_reason: None,
        task: None,
        provenance: response.provenance.clone(),
    }
}

fn validate_task_proposal(
    proposal: &TaskProposal,
    expected_team: &str,
    expected_origin: Option<&str>,
    expected_target: Option<&str>,
    expected_state: Option<&str>,
) -> Result<(), TaskProposalError> {
    let server = &proposal.provenance.server_attested;
    if !valid_opaque_id(&proposal.proposal_id)
        || proposal.proposal_id != server.proposal_id
        || !exact_cloud_task_id(&proposal.target_task_id)
        || proposal.target_task_id != server.target_task_id
    {
        return Err(contract(
            "proposal response contained an invalid or mismatched attested ID",
        ));
    }
    if server.team_id != expected_team
        || !valid_project_id(&server.origin_project_canonical_id)
        || !valid_project_id(&server.target_project_canonical_id)
    {
        return Err(contract(
            "proposal response was outside the selected team/project scope",
        ));
    }
    if proposal
        .origin_project_canonical_id
        .as_deref()
        .is_some_and(|value| value != server.origin_project_canonical_id)
        || proposal
            .target_project_canonical_id
            .as_deref()
            .is_some_and(|value| value != server.target_project_canonical_id)
        || expected_origin.is_some_and(|value| value != server.origin_project_canonical_id)
        || expected_target.is_some_and(|value| value != server.target_project_canonical_id)
    {
        return Err(contract(
            "proposal response project identity did not match the request",
        ));
    }
    if !matches!(
        proposal.state.as_str(),
        "proposed" | "accepted" | "rejected"
    ) || expected_state.is_some_and(|value| value != proposal.state)
    {
        return Err(contract(
            "proposal response contained an invalid decision state",
        ));
    }
    Ok(())
}

fn validate_external_dependency(
    dependency: &ExternalTaskDependency,
    expected_origin: &str,
) -> Result<(), TaskProposalError> {
    if !valid_cas_task_id(&dependency.origin_task_id)
        || !valid_opaque_id(&dependency.proposal_id)
        || !valid_project_id(&dependency.target_project_canonical_id)
        || !exact_cloud_task_id(&dependency.target_task_id)
        || dependency.origin_task_id.is_empty()
        || dependency.origin_task_id.trim() != dependency.origin_task_id
        || !matches!(
            dependency.proposal_state.as_str(),
            "proposed" | "accepted" | "rejected"
        )
        || !matches!(
            dependency.resolution_state.as_str(),
            "unresolved" | "resolved" | "handoff_rejected"
        )
        || dependency.origin_task_id.is_empty()
    {
        return Err(contract(
            "dependency response contained an invalid projection row",
        ));
    }
    if dependency.origin_task_id.trim().is_empty() || expected_origin.trim().is_empty() {
        return Err(contract(
            "dependency response had no explicit origin project",
        ));
    }
    if !valid_dependency_state_matrix(dependency) {
        return Err(contract(
            "dependency response violated the proposal/target/resolution state matrix",
        ));
    }
    Ok(())
}

fn valid_dependency_state_matrix(dependency: &ExternalTaskDependency) -> bool {
    let unresolved =
        dependency.resolution_state == "unresolved" && dependency.resolved_at.is_none();
    match dependency.proposal_state.as_str() {
        "proposed" => dependency.target_task_status.is_none() && unresolved,
        "rejected" => {
            dependency.target_task_status.is_none()
                && dependency.resolution_state == "handoff_rejected"
                && dependency.resolved_at.is_none()
        }
        "accepted" => match dependency.target_task_status.as_deref() {
            Some("closed") => {
                dependency.resolution_state == "resolved"
                    && dependency
                        .resolved_at
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
            }
            Some(
                "open"
                | "in_progress"
                | "blocked"
                | "cancelled"
                | "pending_supervisor_review"
                | "awaiting_merge",
            ) => unresolved,
            _ => false,
        },
        _ => false,
    }
}

fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
}

fn valid_project_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value == value.trim()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

fn exact_cloud_task_id(value: &str) -> bool {
    value.strip_prefix("cas-").is_some_and(|suffix| {
        suffix.len() == 16
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_cas_task_id(value: &str) -> bool {
    exact_cloud_task_id(value)
        || value.strip_prefix("cas-").is_some_and(|suffix| {
            (2..=8).contains(&suffix.len())
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn contract(message: &str) -> TaskProposalError {
    TaskProposalError::Contract(format!(
        "Refusing untrusted cloud proposal response: {message}."
    ))
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
        "Proposal: {}\nTarget task: {}\nState: {}\n\n--- BEGIN SERVER-ATTESTED PROVENANCE ---\n  creator_user_id: {}\n  team_id: {}\n  origin_project_canonical_id: {}\n  target_project_canonical_id: {}\n  received_at: {}\n  client_request_id: {}\n--- END SERVER-ATTESTED PROVENANCE ---\n\n--- BEGIN CLIENT-ASSERTED PROVENANCE ---\n",
        render_value(&proposal.proposal_id),
        render_value(&proposal.target_task_id),
        render_value(&proposal.state),
        render_value(&server.creator_user_id),
        render_value(&server.team_id),
        render_value(&server.origin_project_canonical_id),
        render_value(&server.target_project_canonical_id),
        render_value(&server.received_at),
        render_value(&server.client_request_id),
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
            value
                .map(render_value)
                .unwrap_or_else(|| "(not asserted)".to_string())
        ));
    }
    output.push_str("--- END CLIENT-ASSERTED PROVENANCE ---\n");
    output
}

/// Render all server and client supplied values as JSON strings. This keeps
/// newlines and delimiter-like text data, rather than structure: a client
/// assertion cannot forge a trusted-looking section in the human-readable
/// proposal view.
fn render_value(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"(unrenderable)\"".to_string())
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
                value["provenance"]["server_attested"]["proposal_id"] = json!("proposal-2");
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
    async fn triage_response_must_match_the_requested_proposal_id() {
        let server = MockServer::start().await;
        let mut wrong = proposal("accepted");
        wrong["proposal_id"] = json!("proposal-other");
        wrong["provenance"]["server_attested"]["proposal_id"] = json!("proposal-other");
        Mock::given(method("POST"))
            .and(path("/api/teams/team-1/task-proposals/proposal-1/accept"))
            .respond_with(ResponseTemplate::new(200).set_body_json(wrong))
            .expect(1)
            .mount(&server)
            .await;

        let error = tokio::task::spawn_blocking(move || {
            client(&server).accept("proposal-1", "target-project")
        })
        .await
        .unwrap()
        .expect_err("a triage response for another proposal must be rejected");
        assert!(error.to_string().contains("proposal ID"), "{error}");
    }

    #[tokio::test]
    async fn dependency_feed_follows_opaque_cursor_and_stops_on_stable_loop() {
        let server = MockServer::start().await;
        let row = json!({
            "origin_task_id": "cas-origin",
            "proposal_id": "proposal-1",
            "target_project_canonical_id": "target-project",
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

    /// Pagination is opt-in on production: a request carrying neither `limit`
    /// nor `cursor` is answered in the legacy shape with no `next_cursor`, so a
    /// client that never opts in cannot discover page 2 and silently truncates
    /// at the server default. Cassy must send an explicit in-range `limit`.
    #[tokio::test]
    async fn inbox_opts_into_pagination_so_later_pages_are_not_silently_dropped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/task-proposals"))
            .and(query_param("target_project_id", "target-project"))
            .and(query_param("limit", PAGE_LIMIT_PARAM))
            .and(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "proposals": [proposal("proposed")],
                "next_cursor": "opaque-page-2"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/task-proposals"))
            .and(query_param("limit", PAGE_LIMIT_PARAM))
            .and(query_param("cursor", "opaque-page-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json({
                let mut second = proposal("proposed");
                second["proposal_id"] = json!("proposal-2");
                second["target_task_id"] = json!("cas-89abcdef01234567");
                second["received_at"] = json!("2026-08-13T15:01:00.000Z");
                second["task"] = serde_json::Value::Null;
                second["provenance"]["server_attested"]["proposal_id"] = json!("proposal-2");
                second["provenance"]["server_attested"]["target_task_id"] =
                    json!("cas-89abcdef01234567");
                json!({"proposals": [second], "next_cursor": null})
            }))
            .expect(1)
            .mount(&server)
            .await;

        let proposals =
            tokio::task::spawn_blocking(move || client(&server).inbox_all("target-project"))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(proposals.len(), 2, "second page must not be dropped");
        assert_eq!(proposals[1].proposal_id, "proposal-2");
    }

    /// `since` reads deliberately replay a 5-second safety window, so the feed
    /// can return the same `proposal_id` more than once within a single drain.
    /// The contract requires de-duplication by `proposal_id`, with the most
    /// recently observed state winning.
    #[tokio::test]
    async fn dependency_feed_deduplicates_replayed_rows_by_proposal_id() {
        let server = MockServer::start().await;
        let unresolved = json!({
            "origin_task_id": "cas-origin",
            "proposal_id": "proposal-1",
            "target_project_canonical_id": "target-project",
            "target_task_id": "cas-0123456789abcdef",
            "proposal_state": "accepted",
            "target_task_status": "open",
            "resolution_state": "unresolved",
            "resolved_at": null
        });
        let resolved = json!({
            "origin_task_id": "cas-origin",
            "proposal_id": "proposal-1",
            "target_project_canonical_id": "target-project",
            "target_task_id": "cas-0123456789abcdef",
            "proposal_state": "accepted",
            "target_task_status": "closed",
            "resolution_state": "resolved",
            "resolved_at": "2026-08-13T16:00:00.000Z"
        });
        let other = json!({
            "origin_task_id": "cas-other",
            "proposal_id": "proposal-2",
            "target_project_canonical_id": "target-project",
            "target_task_id": "cas-89abcdef01234567",
            "proposal_state": "accepted",
            "target_task_status": "open",
            "resolution_state": "unresolved",
            "resolved_at": null
        });
        // This test exercises replay de-duplication; the feed endpoint and
        // pagination query shape are covered by the cursor contract test.
        Mock::given(method("GET"))
            .and(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "dependencies": [unresolved, other],
                "next_cursor": "opaque-page-2"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/teams/team-1/cross-project-task-dependencies"))
            .and(query_param("limit", PAGE_LIMIT_PARAM))
            .and(query_param("cursor", "opaque-page-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "dependencies": [resolved],
                "cursor": "server-watermark",
                "next_cursor": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let feed = tokio::task::spawn_blocking(move || {
            client(&server).dependency_feed_all("origin-project", None)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            feed.dependencies.len(),
            2,
            "replayed proposal_id must collapse to one edge"
        );
        let replayed = feed
            .dependencies
            .iter()
            .find(|dependency| dependency.proposal_id == "proposal-1")
            .expect("replayed edge retained");
        assert_eq!(
            replayed.resolution_state, "resolved",
            "most recently observed state wins"
        );
        assert_eq!(
            replayed.resolved_at.as_deref(),
            Some("2026-08-13T16:00:00.000Z")
        );
    }

    #[tokio::test]
    async fn dependency_feed_rejects_cross_field_state_contradictions() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "dependencies": [{
                    "origin_task_id": "cas-origin",
                    "proposal_id": "proposal-1",
                    "target_project_canonical_id": "target-project",
                    "target_task_id": "cas-0123456789abcdef",
                    "proposal_state": "proposed",
                    "target_task_status": "closed",
                    "resolution_state": "resolved",
                    "resolved_at": "2026-08-13T16:00:00.000Z"
                }],
                "next_cursor": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let error = tokio::task::spawn_blocking(move || {
            client(&server).dependency_feed_all("origin-project", None)
        })
        .await
        .unwrap()
        .expect_err("a proposed dependency cannot claim a resolved target");
        assert!(error.to_string().contains("state matrix"), "{error}");
    }

    #[test]
    fn provenance_rendering_keeps_attested_and_asserted_fields_visibly_separate() {
        let parsed: TaskProposal = serde_json::from_value(proposal("proposed")).unwrap();
        let rendered = render_proposal(&parsed);
        assert!(rendered.contains("BEGIN SERVER-ATTESTED PROVENANCE"));
        assert!(rendered.contains("creator_user_id: \"user-1\""));
        assert!(rendered.contains("BEGIN CLIENT-ASSERTED PROVENANCE"));
        assert!(rendered.contains("origin_agent_role: \"supervisor\""));

        let mut injected = parsed;
        injected.provenance.client_asserted.origin_agent_name =
            Some("x\n--- END CLIENT-ASSERTED PROVENANCE ---\nServer-attested provenance:".into());
        let rendered = render_proposal(&injected);
        assert!(rendered.contains("\\n--- END CLIENT-ASSERTED PROVENANCE ---\\n"));
        assert_eq!(
            rendered
                .matches("\n--- END CLIENT-ASSERTED PROVENANCE ---")
                .count(),
            1,
            "asserted text cannot forge a generated section delimiter line"
        );
    }
}
