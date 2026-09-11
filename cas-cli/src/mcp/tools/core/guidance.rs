//! Request-local rendering context for Cassy-authored recovery templates.
//! This never transforms response text or participates in authorization.
use crate::mcp::server::CasCore;

tokio::task_local! {
    static CALLER_PREFIX: (&'static str, Option<&'static str>);
}

pub(crate) async fn with_caller_prefix<T>(
    prefix: &'static str,
    supervisor: Option<&'static str>,
    future: impl std::future::Future<Output = T>,
) -> T {
    CALLER_PREFIX.scope((prefix, supervisor), future).await
}

/// Free lifecycle gates use the service caller captured inside panic dispatch.
/// Direct core/CLI callers retain the existing process-own harness fallback.
pub(crate) fn caller_prefix() -> &'static str {
    CALLER_PREFIX
        .try_with(|prefix| prefix.0)
        .unwrap_or_else(|_| crate::harness_policy::own_tool_prefix())
}

/// Delegated supervisor commands use registered recipient evidence in MCP
/// requests. Direct legacy gate callers retain the explicit supervisor policy.
pub(crate) fn supervisor_prefix() -> &'static str {
    CALLER_PREFIX
        .try_with(|prefix| prefix.1.unwrap_or(""))
        .unwrap_or_else(|_| {
            crate::harness_policy::supervisor_harness_from_env()
                .backend()
                .capabilities()
                .tool_prefix
        })
}

impl CasCore {
    pub(crate) fn supervisor_guidance_prefix(&self) -> Option<&'static str> {
        let id = self.get_registered_agent_id_read_only().ok()?;
        let store = self.open_agent_store().ok()?;
        let caller = store.get(&id).ok()?;
        if caller.role == cas_types::AgentRole::Supervisor {
            return crate::harness_policy::agent_tool_prefix(&caller)
                .or_else(|| Some(crate::harness_policy::own_tool_prefix()));
        }
        if let Some(parent) = caller
            .parent_id
            .as_deref()
            .and_then(|id| store.get(id).ok())
            .filter(|parent| parent.role == cas_types::AgentRole::Supervisor)
        {
            return crate::harness_policy::agent_tool_prefix(&parent);
        }
        let session = caller.factory_session.as_deref()?;
        let agents = store.list(None).ok()?;
        let mut supervisors = agents.iter().filter(|agent| {
            agent.role == cas_types::AgentRole::Supervisor
                && agent.factory_session.as_deref() == Some(session)
        });
        let supervisor = supervisors.next()?;
        if supervisors.next().is_some() {
            return None;
        }
        crate::harness_policy::agent_tool_prefix(supervisor)
    }

    /// Unknown named recipients receive a neutral action hint, never the caller alias.
    pub(crate) fn recipient_guidance_prefix(&self, recipient: &str) -> Option<&'static str> {
        let store = self.open_agent_store().ok()?;
        let agent = super::task::resolve_agent_identity(store.as_ref(), recipient)?;
        crate::harness_policy::agent_tool_prefix(&agent)
    }

    pub(crate) fn guidance_prefix(&self) -> &'static str {
        self.get_registered_agent_id_read_only()
            .ok()
            .and_then(|id| self.open_agent_store().ok()?.get(&id).ok())
            .as_ref()
            .and_then(crate::harness_policy::agent_tool_prefix)
            .unwrap_or_else(crate::harness_policy::own_tool_prefix)
    }
}
