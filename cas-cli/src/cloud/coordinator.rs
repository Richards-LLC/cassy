//! Cloud coordinator client for multi-agent coordination
//!
//! Provides HTTP client for communicating with cas-cloud for
//! distributed agent coordination across multiple machines.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::cloud::CloudConfig;
use crate::error::CasError;
use crate::types::{Agent, AgentStatus, AgentType, ClaimResult, LeaseStatus, TaskLease};

/// Default timeout for HTTP requests
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Short timeout for non-critical requests (heartbeats)
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// The body field that scopes register and claim calls to a project
/// (petra-stella-cloud#151, cas-f707). Its value is the repo's canonical id.
/// Cloud #155 resolves claim project/team scope from this field or the
/// registered agent binding; renew and release stay owner-only.
pub const PROJECT_SCOPE_KEY: &str = "canonical_id";

/// The query parameter that scopes `GET /api/agents` (and other project
/// lists) to a project: `?project=<canonical id or alias>` (cloud PR #153).
pub const PROJECT_QUERY_KEY: &str = "project";

/// A heartbeat the cloud refused or could not take (cas-f707).
#[derive(Debug)]
pub enum HeartbeatError {
    /// The cloud does not know this agent (404) or will not accept it as
    /// the caller's (401): the agent must register again.
    NotRegistered { status: u16, body: String },
    /// Any other failure (network, 5xx, unreadable body).
    Failed(CasError),
}

impl std::fmt::Display for HeartbeatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeartbeatError::NotRegistered { status, body } => {
                write!(f, "Heartbeat failed ({status}): {body}")
            }
            HeartbeatError::Failed(error) => write!(f, "{error}"),
        }
    }
}

/// Cloud coordinator for distributed multi-agent coordination
#[derive(Clone)]
pub struct CloudCoordinator {
    config: CloudConfig,
    timeout: Duration,
    agent_id: Option<String>,
    /// The repo's canonical id, registered in each agent's metadata so peer
    /// supervisors of the same repo can find each other (cas-e477).
    canonical_id: Option<String>,
    /// The machine label registrations publish; `None` means
    /// [`crate::cloud::peers::machine_label`] (cas-c7c7).
    machine_label: Option<String>,
    factory_slot_id: Option<String>,
}

/// Response from agent registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistration {
    pub status: String,
    pub agent: AgentInfo,
}

/// Agent information from cloud
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub agent_type: String,
    pub status: String,
    pub pid: Option<u32>,
    pub session_id: Option<String>,
    pub machine_id: Option<String>,
    pub last_heartbeat: String,
    pub active_tasks: u32,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Response from task claim
///
/// cas-0078: production's 409 body is
/// `{"error":"already_claimed","owner_agent_id","agent_id","expires_at"}`
/// with no `status`, so every field is optional on the wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaimResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub lock: Option<LockInfo>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub owner_agent_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    /// When the holder's lock runs out (409 only).
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Lock information from cloud
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub expires_at: String,
    pub renewed_at: String,
    pub renewal_count: u32,
    #[serde(default = "default_epoch")]
    pub epoch: Option<u64>,
    pub claim_reason: Option<String>,
}

fn default_epoch() -> Option<u64> {
    Some(1)
}

/// Response containing a list of agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsResponse {
    pub status: String,
    pub agents: Vec<AgentInfo>,
}

/// One keyset page of `GET /api/agents` (cas-e477).
#[derive(Debug, Clone, Deserialize)]
struct AgentsPage {
    agents: Vec<AgentInfo>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    next_cursor_id: Option<String>,
    #[serde(default)]
    next_sort_cursor: Option<String>,
}

/// Agents per page; the server caps `limit` at 500.
const AGENT_PAGE_LIMIT: u32 = 500;
/// Pages read before giving up on a runaway cursor.
const MAX_AGENT_PAGES: usize = 20;

/// Response containing a list of locks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocksResponse {
    pub status: String,
    pub locks: Vec<LockInfo>,
}

impl CloudCoordinator {
    /// Create a new cloud coordinator from config
    pub fn new(config: CloudConfig) -> Result<Self, CasError> {
        if !config.is_logged_in() {
            return Err(CasError::Other("Not logged in to Cassy Cloud".to_string()));
        }

        Ok(Self {
            config,
            timeout: DEFAULT_TIMEOUT,
            agent_id: None,
            canonical_id: None,
            machine_label: None,
            factory_slot_id: None,
        })
    }

    /// Publish `label` as this machine's label instead of the hostname
    /// (a project's `[cloud] machine_label`, cas-c7c7).
    pub fn with_machine_label(mut self, label: Option<String>) -> Self {
        self.machine_label = label.filter(|label| !label.trim().is_empty());
        self
    }

    /// The machine label this coordinator publishes.
    pub fn machine_label(&self) -> String {
        self.machine_label
            .clone()
            .unwrap_or_else(crate::cloud::peers::machine_label)
    }

    /// Register agents under this repo identity (cas-e477).
    pub fn with_canonical_id(mut self, canonical_id: Option<String>) -> Self {
        self.canonical_id = canonical_id;
        self
    }

    pub(crate) fn with_factory_slot_id(mut self, slot_id: Option<String>) -> Self {
        self.factory_slot_id = slot_id.map(|id| id.trim().to_string()).filter(|id| !id.is_empty());
        self
    }

    /// Successors in one factory session share a Cloud slot; concurrent factory
    /// sessions on the same clone must not supersede each other. Use the agent
    /// row, not the shared daemon's ambient factory session, for this identity.
    fn factory_id(&self, agent: &Agent) -> Option<String> {
        let legacy = || std::env::var("CAS_FACTORY_ID").ok().filter(|id| !id.trim().is_empty());
        if agent.role != crate::types::AgentRole::Supervisor {
            return legacy();
        }
        self.factory_slot_id.clone()
            .or_else(|| {
                use sha2::{Digest, Sha256};
                let machine = agent.machine_id.as_deref().map(str::trim).filter(|id| !id.is_empty())?;
                let project = self.canonical_id.as_deref()
                    .and_then(crate::cloud::normalize_project_canonical_id)?;
                let session = agent.factory_session.as_deref()
                    .map(str::trim).filter(|id| !id.is_empty());
                let identity = match session {
                    Some(session) => serde_json::to_vec(&(machine, project, "supervisor", session)),
                    // Preserve the legacy slot for non-factory supervisors.
                    None => serde_json::to_vec(&(machine, project, "supervisor")),
                }.ok()?;
                Some(format!("cassy-slot-{}", hex::encode(Sha256::digest(identity))))
            })
            .or_else(legacy)
            .or_else(|| agent.factory_session.clone().filter(|id| !id.trim().is_empty()))
    }

    /// Set the request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Act as an agent already registered under `agent_id`. Cloud agent ids
    /// are the local agent ids (`register` upserts the id it is given), so a
    /// task-claim call needs no second registration (cas-5f28).
    pub fn with_agent_id(mut self, agent_id: &str) -> Self {
        self.agent_id = Some(agent_id.to_string());
        self
    }

    /// Get the current agent ID
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// Get the auth token
    fn token(&self) -> &str {
        self.config.token.as_deref().unwrap_or("")
    }

    /// The cloud config (endpoint, login) this coordinator was built from.
    pub fn cloud_config(&self) -> &CloudConfig {
        &self.config
    }

    /// The repo identity this coordinator scopes its calls to.
    pub fn canonical_id(&self) -> Option<&str> {
        self.canonical_id.as_deref()
    }

    /// `body` with [`PROJECT_SCOPE_KEY`] set to this coordinator's
    /// canonical id, when it has one (cas-f707).
    fn project_scoped(&self, mut body: serde_json::Value) -> serde_json::Value {
        if let (Some(canonical_id), Some(fields)) =
            (self.canonical_id.as_deref(), body.as_object_mut())
        {
            fields.insert(PROJECT_SCOPE_KEY.to_string(), canonical_id.into());
        }
        body
    }

    /// `request` with the [`PROJECT_QUERY_KEY`] query parameter, when this
    /// coordinator has a canonical id (cas-f707). Shared by every project
    /// list call.
    pub(crate) fn project_query(&self, request: ureq::Request) -> ureq::Request {
        match self.canonical_id.as_deref() {
            Some(canonical_id) => request.query(PROJECT_QUERY_KEY, canonical_id),
            None => request,
        }
    }

    /// Register an agent with the cloud
    pub fn register(&mut self, agent: &Agent) -> Result<AgentInfo, CasError> {
        self.register_with_focus(agent, None)
    }

    /// Register `agent` with its peer identity: role, repo canonical id,
    /// `focus`, factory session and hostname in metadata (cas-e477). The
    /// cloud echoes metadata back unchanged, which is what lets supervisors
    /// of one repo find each other through a user-scoped agent list.
    pub fn register_with_focus(
        &mut self,
        agent: &Agent,
        focus: Option<&str>,
    ) -> Result<AgentInfo, CasError> {
        self.register_with_presence(agent, focus, None)
    }

    /// [`Self::register_with_focus`] plus the branch checked out where the
    /// agent works, as `metadata.branch` (cas-f707).
    pub fn register_with_presence(
        &mut self,
        agent: &Agent,
        focus: Option<&str>,
        branch: Option<&str>,
    ) -> Result<AgentInfo, CasError> {
        let url = format!("{}/api/agents/register", self.config.endpoint);

        let factory_id = self.factory_id(agent);
        let machine = self.machine_label();
        let mut metadata = crate::cloud::peers::identity_metadata(
            agent,
            self.canonical_id.as_deref(),
            focus,
            Some(machine.as_str()),
        );
        if let Some(branch) = branch.map(str::trim).filter(|branch| !branch.is_empty()) {
            metadata.insert(
                crate::cloud::peers::META_BRANCH.to_string(),
                branch.to_string(),
            );
        }
        // cas-d58d: peers label this supervisor with the account it runs as.
        if let Some(email) = self.config.email.as_deref().map(str::trim).filter(|email| !email.is_empty()) {
            metadata.insert(crate::cloud::peers::META_OWNER.to_string(), email.to_string());
        }
        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .set("Content-Type", "application/json")
            .send_json(self.project_scoped(serde_json::json!({
                "id": agent.id,
                "name": agent.name,
                "agent_type": format!("{:?}", agent.agent_type).to_lowercase(),
                "role": format!("{:?}", agent.role).to_lowercase(),
                "pid": agent.pid,
                "parent_id": agent.parent_id,
                "machine_id": agent.machine_id,
                "factory_id": factory_id,
                "clone_path": agent.metadata.get("clone_path"),
                "metadata": metadata,
            })));

        match response {
            Ok(resp) => {
                let reg: AgentRegistration = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                self.agent_id = Some(reg.agent.id.clone());
                Ok(reg.agent)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Failed to register agent {}({code}): {body}",
                    if code == 429 { "rate limited " } else { "" }
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// Send heartbeat for current agent
    pub fn heartbeat(&self) -> Result<AgentInfo, CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;
        self.heartbeat_agent(agent_id)
    }

    /// Shutdown the current agent. Idempotent cleanup retries only HTTP 429,
    /// within five seconds (or the caller's shorter timeout). Run off-loop.
    pub fn shutdown(&self) -> Result<u32, CasError> {
        self.shutdown_with_retry()
            .map_err(|error| CasError::Other(error.cause))
    }

    pub(crate) fn shutdown_with_retry(&self) -> Result<u32, super::rate_limit::Failure> {
        use super::rate_limit::{self, Failure};
        let agent_id = self.agent_id.as_ref().ok_or_else(|| Failure {
            cause: "No agent registered".into(),
            retry_after: None,
            status: None,
        })?;
        let url = format!("{}/api/agents/{}/shutdown", self.config.endpoint, agent_id);
        let response = rate_limit::call(self.timeout, |left| {
            ureq::post(&url)
                .timeout(left)
                .set("Authorization", &format!("Bearer {}", self.token()))
                .call()
        })
        .map_err(|error| Failure::from_http("Shutdown", error))?;
        #[derive(Deserialize)]
        struct ShutdownResponse {
            released_locks: u32,
        }
        response
            .into_json::<ShutdownResponse>()
            .map(|response| response.released_locks)
            .map_err(|error| Failure {
                cause: format!("Failed to parse shutdown response: {error}"),
                retry_after: None,
                status: None,
            })
    }

    /// Claim a task
    pub fn claim(
        &self,
        task_id: &str,
        duration_secs: u32,
        reason: Option<&str>,
    ) -> Result<ClaimResult, CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;

        let url = format!(
            "{}/api/agents/tasks/{}/claim",
            self.config.endpoint, task_id
        );

        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .set("Content-Type", "application/json")
            .send_json(self.project_scoped(serde_json::json!({
                "agent_id": agent_id,
                "duration_secs": duration_secs,
                "reason": reason,
            })));

        match response {
            Ok(resp) => {
                let claim_resp: ClaimResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;

                if claim_resp.result == Some("claimed".to_string()) {
                    if let Some(lock) = claim_resp.lock {
                        Ok(ClaimResult::Success(lock_info_to_lease(&lock)))
                    } else {
                        Err(CasError::Other("Missing lock in response".to_string()))
                    }
                } else {
                    Ok(ClaimResult::NotClaimable {
                        task_id: task_id.to_string(),
                        reason: "Unknown error".to_string(),
                    })
                }
            }
            Err(ureq::Error::Status(409, resp)) => {
                // Conflict - already claimed. The body names the holder and
                // when its lock runs out. A missing or unparseable expiry
                // reads as now: production frees an expired lock, so a 409
                // never names a past expiry, and callers treat one as unknown.
                let claim_resp: ClaimResponse = resp.into_json().unwrap_or_default();
                let expires_at = claim_resp
                    .expires_at
                    .as_deref()
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.with_timezone(&chrono::Utc))
                    .unwrap_or_else(chrono::Utc::now);

                Ok(ClaimResult::AlreadyClaimed {
                    task_id: task_id.to_string(),
                    held_by: claim_resp
                        .owner_agent_id
                        .or(claim_resp.agent_id)
                        .unwrap_or_default(),
                    user_id: claim_resp.user_id.filter(|id| !id.trim().is_empty()),
                    expires_at,
                })
            }
            Err(ureq::Error::Status(404, _)) => Ok(ClaimResult::TaskNotFound(task_id.to_string())),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!("Claim failed ({code}): {body}")))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// Release a task
    pub fn release(&self, task_id: &str) -> Result<(), CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;

        let url = format!(
            "{}/api/agents/tasks/{}/release",
            self.config.endpoint, task_id
        );

        super::rate_limit::call(self.timeout, |left| {
            ureq::post(&url)
                .timeout(left)
                .set("Authorization", &format!("Bearer {}", self.token()))
                .set("Content-Type", "application/json")
                .send_json(self.project_scoped(serde_json::json!({ "agent_id": agent_id })))
        })
        .map(|_| ())
        .map_err(|error| {
            CasError::Other(super::rate_limit::Failure::from_http("Release", error).cause)
        })
    }

    /// cas-da31: release `task_id` only while its lock still has this epoch
    /// and renewal count (petra-stella-cloud #158 / PR #164: both fields or
    /// neither). A reacquisition or renewal since then is answered 409
    /// `lock_snapshot_changed`: `Ok(false)`, and nothing is released.
    pub fn release_expecting(
        &self,
        task_id: &str,
        epoch: u64,
        renewal_count: u32,
    ) -> Result<bool, CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;
        let url = format!(
            "{}/api/agents/tasks/{}/release",
            self.config.endpoint, task_id
        );
        let response = super::rate_limit::call(self.timeout, |left| {
            ureq::post(&url)
                .timeout(left)
                .set("Authorization", &format!("Bearer {}", self.token()))
                .set("Content-Type", "application/json")
                .send_json(self.project_scoped(serde_json::json!({
                    "agent_id": agent_id,
                    "expected_epoch": epoch,
                    "expected_renewal_count": renewal_count,
                })))
        });
        match response {
            Ok(_) => Ok(true),
            Err(ureq::Error::Status(409, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                if body.contains("lock_snapshot_changed") {
                    Ok(false)
                } else {
                    Err(CasError::Other(format!("Release failed (409): {body}")))
                }
            }
            Err(error) => Err(CasError::Other(
                super::rate_limit::Failure::from_http("Release", error).cause,
            )),
        }
    }

    /// Renew a task lease
    pub fn renew(&self, task_id: &str, duration_secs: u32) -> Result<TaskLease, CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;

        let url = format!(
            "{}/api/agents/tasks/{}/renew",
            self.config.endpoint, task_id
        );

        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .set("Content-Type", "application/json")
            .send_json(self.project_scoped(serde_json::json!({
                "agent_id": agent_id,
                "duration_secs": duration_secs,
            })));

        match response {
            Ok(resp) => {
                #[derive(Deserialize)]
                struct RenewResponse {
                    lock: LockInfo,
                }
                let resp: RenewResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(lock_info_to_lease(&resp.lock))
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!("Renew failed ({code}): {body}")))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// Get lock for a task
    pub fn get_lock(&self, task_id: &str) -> Result<Option<TaskLease>, CasError> {
        self.get_lock_record(task_id)
            .map(|lock| lock.map(|lock| lock_info_to_lease(&lock)))
    }

    fn get_lock_record(&self, task_id: &str) -> Result<Option<LockInfo>, CasError> {
        let url = format!("{}/api/agents/tasks/{}/lock", self.config.endpoint, task_id);

        let response = ureq::get(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .call();

        match response {
            Ok(resp) => {
                #[derive(Deserialize)]
                struct LockResponse {
                    lock: Option<LockInfo>,
                }
                let resp: LockResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(resp.lock)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!("Get lock failed ({code}): {body}")))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// The singular route remains account-private in Cloud #155. If it
    /// has no own lock, read the authorized project list for a teammate.
    /// The flag excludes foreign locks from account-owned stale recovery.
    pub(crate) fn get_visible_lock(
        &self,
        task_id: &str,
    ) -> Result<Option<(LockInfo, bool)>, CasError> {
        let deadline = Instant::now() + self.timeout;
        if let Some(lock) = self.get_lock_record(task_id)? {
            return Ok(Some((lock, false)));
        }
        if self.canonical_id.is_none() {
            return Ok(None);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(CasError::Other("peer check budget exhausted".into()));
        }
        Ok(self
            .clone()
            .with_timeout(left)
            .list_lock_records()?
            .into_iter()
            .find(|lock| lock.task_id == task_id)
            .map(|lock| (lock, true)))
    }

    /// List all agents
    pub fn list_agents(&self, status: Option<AgentStatus>) -> Result<Vec<Agent>, CasError> {
        let mut url = format!("{}/api/agents", self.config.endpoint);
        if let Some(s) = status {
            url.push_str(&format!("?status={s:?}").to_lowercase());
        }

        let response = self
            .project_query(
                ureq::get(&url)
                    .timeout(self.timeout)
                    .set("Authorization", &format!("Bearer {}", self.token())),
            )
            .call();

        match response {
            Ok(resp) => {
                let resp: AgentsResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(resp.agents.into_iter().map(agent_info_to_agent).collect())
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "List agents failed ({code}): {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// List project-visible active locks, including teammates (#155).
    /// Callers still filter owners through authorized repo discovery.
    pub fn list_locks(&self) -> Result<Vec<TaskLease>, CasError> {
        self.list_lock_records()
            .map(|locks| locks.iter().map(lock_info_to_lease).collect())
    }

    fn list_lock_records(&self) -> Result<Vec<LockInfo>, CasError> {
        let url = format!("{}/api/agents/locks", self.config.endpoint);

        let request = ureq::get(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()));
        let response = self.project_query(request).call();

        match response {
            Ok(resp) => {
                let resp: LocksResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(resp.locks)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "List locks failed ({code}): {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// List locks for current agent
    pub fn list_my_locks(&self) -> Result<Vec<TaskLease>, CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;

        let url = format!("{}/api/agents/{}/locks", self.config.endpoint, agent_id);

        let response = ureq::get(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .call();

        match response {
            Ok(resp) => {
                let resp: LocksResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(resp.locks.iter().map(lock_info_to_lease).collect())
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "List locks failed ({code}): {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }
    /// Heartbeat a specific agent (cas-e477). Each daemon heartbeats its own
    /// agent, not whichever agent this coordinator registered last.
    pub fn heartbeat_agent(&self, agent_id: &str) -> Result<AgentInfo, CasError> {
        self.heartbeat_presence(agent_id, None)
            .map_err(|error| match error {
                HeartbeatError::Failed(error) => error,
                not_registered => CasError::Other(not_registered.to_string()),
            })
    }

    /// [`Self::heartbeat_agent`] with the refusal that means "register
    /// again" (404 or 401) kept apart from other failures (cas-f707).
    /// `metadata`, when given, is sent as `{"metadata": {...}}` and merged by
    /// the cloud: it re-binds the project and refreshes focus and branch on
    /// every beat (cloud PR #153).
    pub fn heartbeat_presence(
        &self,
        agent_id: &str,
        metadata: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<AgentInfo, HeartbeatError> {
        let url = format!("{}/api/agents/{}/heartbeat", self.config.endpoint, agent_id);

        let request = ureq::post(&url)
            .timeout(HEARTBEAT_TIMEOUT)
            .set("Authorization", &format!("Bearer {}", self.token()));
        let response = match metadata {
            Some(metadata) => request
                .set("Content-Type", "application/json")
                .send_json(serde_json::json!({ "metadata": metadata })),
            None => request.call(),
        };

        match response {
            Ok(resp) => {
                let parse = |e: std::io::Error| {
                    HeartbeatError::Failed(CasError::Other(format!(
                        "Failed to parse response: {e}"
                    )))
                };
                let body: serde_json::Value = resp.into_json().map_err(parse)?;
                // The server answers with the bare agent; older servers wrapped
                // it as {status, agent}.
                let agent = match body.get("agent") {
                    Some(agent) if agent.is_object() => agent.clone(),
                    _ => body,
                };
                serde_json::from_value(agent).map_err(|e| {
                    HeartbeatError::Failed(CasError::Other(format!(
                        "Failed to parse response: {e}"
                    )))
                })
            }
            Err(ureq::Error::Status(status @ (401 | 404), resp)) => {
                Err(HeartbeatError::NotRegistered {
                    status,
                    body: resp.into_string().unwrap_or_default(),
                })
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(HeartbeatError::Failed(CasError::Other(format!(
                    "Heartbeat failed {}({code}): {body}",
                    if code == 429 { "rate limited " } else { "" }
                ))))
            }
            Err(ureq::Error::Transport(e)) => Err(HeartbeatError::Failed(CasError::Other(
                format!("Network error: {e}"),
            ))),
        }
    }

    /// Every agent of this user, following the cloud's keyset pages.
    ///
    /// The list is ordered by creation time, newest first, so a long-lived
    /// supervisor can sit past the first page; every page is read, up to
    /// [`MAX_AGENT_PAGES`]. The timeout bounds the entire paginated read,
    /// rather than restarting for each page (cas-50713).
    pub fn list_agent_infos(&self) -> Result<Vec<AgentInfo>, CasError> {
        let mut agents = Vec::new();
        let mut cursor: Option<(String, String, Option<String>)> = None;
        let deadline = Instant::now() + self.timeout;
        for _ in 0..MAX_AGENT_PAGES {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CasError::Other("List agents budget exhausted".into()));
            }
            let mut request = self.project_query(
                ureq::get(&format!("{}/api/agents", self.config.endpoint))
                    .timeout(remaining)
                    .set("Authorization", &format!("Bearer {}", self.token()))
                    .query("limit", &AGENT_PAGE_LIMIT.to_string()),
            );
            if let Some((at, id, sort)) = &cursor {
                request = request.query("cursor", at).query("cursor_id", id);
                if let Some(sort) = sort {
                    request = request.query("sort_cursor", sort);
                }
            }
            let page: AgentsPage = match request.call() {
                Ok(resp) => resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?,
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    return Err(CasError::Other(format!(
                        "List agents failed ({code}): {body}"
                    )));
                }
                Err(ureq::Error::Transport(e)) => {
                    return Err(CasError::Other(format!("Network error: {e}")));
                }
            };
            agents.extend(page.agents);
            match (page.next_cursor, page.next_cursor_id) {
                (Some(at), Some(id)) => cursor = Some((at, id, page.next_sort_cursor)),
                _ => return Ok(agents),
            }
        }
        Ok(agents)
    }

    /// The supervisors of `canonical_id` other than `self_id` (cas-e477).
    pub fn list_repo_peers(
        &self,
        canonical_id: &str,
        alias_class: &[String],
        self_id: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::cloud::peers::Peer>, CasError> {
        let agents = self.list_agent_infos()?;
        Ok(crate::cloud::peers::repo_peers(
            &agents,
            canonical_id,
            alias_class,
            self_id,
            now,
        ))
    }
}

/// Convert LockInfo to TaskLease
pub(crate) fn lock_info_to_lease(info: &LockInfo) -> TaskLease {
    use chrono::DateTime;

    let expires_at = DateTime::parse_from_rfc3339(&info.expires_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());

    let renewed_at = DateTime::parse_from_rfc3339(&info.renewed_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());

    let status = match info.status.as_str() {
        "active" => LeaseStatus::Active,
        "expired" => LeaseStatus::Expired,
        "released" => LeaseStatus::Released,
        "revoked" => LeaseStatus::Revoked,
        _ => LeaseStatus::Active,
    };

    TaskLease {
        task_id: info.task_id.clone(),
        agent_id: info.agent_id.clone(),
        status,
        acquired_at: renewed_at, // Best approximation
        expires_at,
        renewed_at,
        renewal_count: info.renewal_count,
        epoch: info.epoch.unwrap_or(1),
        claim_reason: info.claim_reason.clone(),
    }
}

/// Convert AgentInfo to Agent
fn agent_info_to_agent(info: AgentInfo) -> Agent {
    use chrono::DateTime;

    let last_heartbeat = DateTime::parse_from_rfc3339(&info.last_heartbeat)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());

    let agent_type = match info.agent_type.as_str() {
        "primary" => AgentType::Primary,
        "sub_agent" => AgentType::SubAgent,
        "worker" => AgentType::Worker,
        "ci" => AgentType::CI,
        _ => AgentType::Primary,
    };

    let status = match info.status.as_str() {
        "active" => AgentStatus::Active,
        "idle" => AgentStatus::Idle,
        "dead" | "stale" => AgentStatus::Stale,
        "shutdown" => AgentStatus::Shutdown,
        _ => AgentStatus::Active,
    };

    Agent {
        id: info.id,
        name: info.name,
        agent_type,
        role: cas_types::AgentRole::Standard,
        status,
        pid: info.pid,
        ppid: None,
        cc_session_id: None,
        factory_session: None,
        parent_id: None,
        machine_id: info.machine_id,
        registered_at: last_heartbeat,
        last_heartbeat,
        active_tasks: info.active_tasks,
        pid_starttime: None,
        metadata: info.metadata,
    }
}

#[cfg(test)]
#[path = "coordinator_peer_tests.rs"]
mod peer_tests;

#[cfg(test)]
mod tests {
    use crate::cloud::coordinator::*;

    #[test]
    fn test_lock_info_conversion() {
        let info = LockInfo {
            id: "lock-123".to_string(),
            user_id: None,
            task_id: "task-456".to_string(),
            agent_id: "agent-789".to_string(),
            status: "active".to_string(),
            expires_at: "2026-01-04T22:00:00Z".to_string(),
            renewed_at: "2026-01-04T21:55:00Z".to_string(),
            renewal_count: 2,
            epoch: Some(1),
            claim_reason: Some("Testing".to_string()),
        };

        let lease = lock_info_to_lease(&info);
        assert_eq!(lease.task_id, "task-456");
        assert_eq!(lease.agent_id, "agent-789");
        assert_eq!(lease.status, LeaseStatus::Active);
        assert_eq!(lease.renewal_count, 2);
    }

    #[test]
    fn test_agent_info_conversion() {
        let info = AgentInfo {
            id: "agent-123".to_string(),
            name: "Test Agent".to_string(),
            agent_type: "worker".to_string(),
            status: "active".to_string(),
            pid: Some(12345),
            session_id: None,
            machine_id: Some("machine-1".to_string()),
            last_heartbeat: "2026-01-04T21:55:00Z".to_string(),
            active_tasks: 3,
            metadata: Default::default(),
            user_id: None,
            team_id: None,
            project_id: None,
        };

        let agent = agent_info_to_agent(info);
        assert_eq!(agent.id, "agent-123");
        assert_eq!(agent.agent_type, AgentType::Worker);
        assert_eq!(agent.status, AgentStatus::Active);
        assert_eq!(agent.pid, Some(12345));
    }
}
