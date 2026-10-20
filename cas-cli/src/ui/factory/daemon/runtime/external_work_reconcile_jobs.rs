//! Bounded off-loop reconciliation for journaled external thread work.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::coordination::external_work::{
    ExternalWorkService, HttpCloudCoordinationPort, OperationContext, RemoteError,
    SqliteRequestJournal, VioletThreadResolver,
};

pub(crate) fn reconcile_due(cas_dir: &Path) -> Result<usize, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let journal = SqliteRequestJournal::open(cas_dir).map_err(|error| error.to_string())?;
    if !journal.has_due(now_ms).map_err(|error| error.to_string())? {
        return Ok(0);
    }

    use crate::store::AgentStore;
    let session_id = std::env::var("CAS_SESSION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| crate::agent_id::read_session_for_mcp(cas_dir).ok())
        .ok_or_else(|| {
            "no authenticated Cassy session for external-work reconciliation".to_string()
        })?;
    let agents = crate::store::open_agent_store(cas_dir).map_err(|error| error.to_string())?;
    let agent = agents.get(&session_id).map_err(|error| error.to_string())?;
    if agent.role != crate::types::AgentRole::Supervisor {
        return Err("external-work reconciliation requires a registered supervisor".into());
    }
    let project_id = crate::cloud::resolve_canonical_id(cas_dir)
        .ok_or_else(|| "project has no canonical Cloud identity".to_string())?;
    let cloud = crate::cloud::CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_dir)
        .map_err(|error| error.to_string())?;
    let token = cloud
        .token
        .clone()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "external-work reconciliation requires Cassy Cloud login".to_string())?;
    let violet = build_violet(cas_dir, &agents, &agent)?;
    let service = ExternalWorkService::new(
        journal,
        Arc::new(HttpCloudCoordinationPort::new(cloud.endpoint, token)),
        violet,
        OperationContext {
            project_id,
            registration_id: agent.id,
        },
    );
    reconcile_service_due(&service, now_ms)
}

pub(crate) fn reconcile_service_due(
    service: &ExternalWorkService,
    now_ms: i64,
) -> Result<usize, String> {
    service
        .reconcile_due(now_ms)
        .map(|results| results.len())
        .map_err(|error| error.to_string())
}

#[cfg(feature = "mcp-proxy")]
struct DaemonVioletResolver {
    runtime: std::sync::Mutex<tokio::runtime::Runtime>,
    proxy: Arc<cmcp_core::ProxyEngine>,
    caller: cmcp_core::ProxyCaller,
}

#[cfg(feature = "mcp-proxy")]
impl VioletThreadResolver for DaemonVioletResolver {
    fn resolve(&self, external_ref: &str, timeout: Duration) -> Result<Value, RemoteError> {
        let args = serde_json::Map::from_iter([("external_ref".to_string(), json!(external_ref))]);
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| RemoteError::unavailable("Violet runtime lock poisoned"))?;
        let result = runtime.block_on(async {
            tokio::time::timeout(
                timeout,
                self.proxy.call_tool(
                    &self.caller,
                    cmcp_core::config::VIOLET_SERVER,
                    "violet_read",
                    Some(args),
                ),
            )
            .await
        });
        // cas-85c1: the MCP resolver's classification, so a daemon reconcile
        // retries a reconnecting drop exactly as a supervisor's claim does.
        crate::coordination::external_work::violet_proxy_outcome(result)
    }
}

#[cfg(feature = "mcp-proxy")]
fn build_violet(
    cas_dir: &Path,
    agents: &Arc<dyn crate::store::AgentStore>,
    agent: &crate::types::Agent,
) -> Result<Arc<dyn VioletThreadResolver>, String> {
    let config =
        crate::mcp::load_proxy_config_for_process(cas_dir).map_err(|error| error.to_string())?;
    let mut active_task_ids = agents
        .list_agent_leases(&agent.id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|lease| lease.task_id)
        .collect::<Vec<_>>();
    active_task_ids.sort();
    let caller = cmcp_core::ProxyCaller {
        agent_id: agent.id.clone(),
        role: agent.role,
        session_id: agent.id.clone(),
        factory_session: agent.factory_session.clone(),
        active_task_ids,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let proxy = Arc::new(
        runtime
            .block_on(cmcp_core::ProxyEngine::from_configs(config.servers.clone()))
            .map_err(|error| error.to_string())?,
    );
    runtime.block_on(crate::mcp::install_proxy_policy(&proxy, &config));
    Ok(Arc::new(DaemonVioletResolver {
        runtime: std::sync::Mutex::new(runtime),
        proxy,
        caller,
    }))
}

#[cfg(not(feature = "mcp-proxy"))]
fn build_violet(
    _cas_dir: &Path,
    _agents: &Arc<dyn crate::store::AgentStore>,
    _agent: &crate::types::Agent,
) -> Result<Arc<dyn VioletThreadResolver>, String> {
    struct Unavailable;
    impl VioletThreadResolver for Unavailable {
        fn resolve(&self, _external_ref: &str, _timeout: Duration) -> Result<Value, RemoteError> {
            Err(RemoteError::unavailable(
                "this build has no MCP proxy for Violet resolution",
            ))
        }
    }
    Ok(Arc::new(Unavailable))
}
