//! Cloud coordinator client for multi-agent coordination
//!
//! Provides HTTP client for communicating with cas-cloud for
//! distributed agent coordination across multiple machines.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::cloud::CloudConfig;
use crate::error::CasError;
use crate::types::{Agent, AgentStatus, AgentType, ClaimResult, LeaseStatus, TaskLease};

/// Default timeout for HTTP requests
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Short timeout for non-critical requests (heartbeats)
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// Cloud coordinator for distributed multi-agent coordination
#[derive(Clone)]
pub struct CloudCoordinator {
    config: CloudConfig,
    timeout: Duration,
    agent_id: Option<String>,
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
}

/// Response from task claim
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimResponse {
    pub status: String,
    pub result: Option<String>,
    pub lock: Option<LockInfo>,
    pub error: Option<String>,
    pub owner_agent_id: Option<String>,
}

/// Lock information from cloud
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub id: String,
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
        })
    }

    /// Set the request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
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

    /// Register an agent with the cloud
    pub fn register(&mut self, agent: &Agent) -> Result<AgentInfo, CasError> {
        let url = format!("{}/api/agents/register", self.config.endpoint);

        let factory_id = std::env::var("CAS_FACTORY_ID").ok();
        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .set("Content-Type", "application/json")
            .send_json(serde_json::json!({
                "id": agent.id,
                "name": agent.name,
                "agent_type": format!("{:?}", agent.agent_type).to_lowercase(),
                "role": format!("{:?}", agent.role).to_lowercase(),
                "pid": agent.pid,
                "parent_id": agent.parent_id,
                "machine_id": agent.machine_id,
                "factory_id": factory_id,
                "clone_path": agent.metadata.get("clone_path"),
                "metadata": agent.metadata,
            }));

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
                    "Failed to register agent ({code}): {body}"
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

        let url = format!("{}/api/agents/{}/heartbeat", self.config.endpoint, agent_id);

        let response = ureq::post(&url)
            .timeout(HEARTBEAT_TIMEOUT)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .call();

        match response {
            Ok(resp) => {
                let reg: AgentRegistration = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(reg.agent)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Heartbeat failed ({code}): {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// Shutdown the current agent
    pub fn shutdown(&self) -> Result<u32, CasError> {
        let agent_id = self
            .agent_id
            .as_ref()
            .ok_or_else(|| CasError::Other("No agent registered".to_string()))?;

        let url = format!("{}/api/agents/{}/shutdown", self.config.endpoint, agent_id);

        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .call();

        match response {
            Ok(resp) => {
                #[derive(Deserialize)]
                struct ShutdownResponse {
                    released_locks: u32,
                }
                let resp: ShutdownResponse = resp
                    .into_json()
                    .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;
                Ok(resp.released_locks)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!("Shutdown failed ({code}): {body}")))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
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
            .send_json(serde_json::json!({
                "agent_id": agent_id,
                "duration_secs": duration_secs,
                "reason": reason,
            }));

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
                // Conflict - already claimed
                let claim_resp: ClaimResponse = resp.into_json().unwrap_or(ClaimResponse {
                    status: "error".to_string(),
                    result: None,
                    lock: None,
                    error: Some("already_claimed".to_string()),
                    owner_agent_id: None,
                });

                Ok(ClaimResult::AlreadyClaimed {
                    task_id: task_id.to_string(),
                    held_by: claim_resp.owner_agent_id.unwrap_or_default(),
                    expires_at: chrono::Utc::now(),
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

        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
            .set("Content-Type", "application/json")
            .send_json(serde_json::json!({
                "agent_id": agent_id,
            }));

        match response {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!("Release failed ({code}): {body}")))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
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
            .send_json(serde_json::json!({
                "agent_id": agent_id,
                "duration_secs": duration_secs,
            }));

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
                Ok(resp.lock.map(|l| lock_info_to_lease(&l)))
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!("Get lock failed ({code}): {body}")))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// List all agents
    pub fn list_agents(&self, status: Option<AgentStatus>) -> Result<Vec<Agent>, CasError> {
        let mut url = format!("{}/api/agents", self.config.endpoint);
        if let Some(s) = status {
            url.push_str(&format!("?status={s:?}").to_lowercase());
        }

        let response = ureq::get(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token()))
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

    /// List all active locks
    pub fn list_locks(&self) -> Result<Vec<TaskLease>, CasError> {
        let url = format!("{}/api/agents/locks", self.config.endpoint);

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
}

/// Convert LockInfo to TaskLease
fn lock_info_to_lease(info: &LockInfo) -> TaskLease {
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
mod tests {
    use crate::cloud::coordinator::*;

    #[test]
    fn test_lock_info_conversion() {
        let info = LockInfo {
            id: "lock-123".to_string(),
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
        };

        let agent = agent_info_to_agent(info);
        assert_eq!(agent.id, "agent-123");
        assert_eq!(agent.agent_type, AgentType::Worker);
        assert_eq!(agent.status, AgentStatus::Active);
        assert_eq!(agent.pid, Some(12345));
    }
}
