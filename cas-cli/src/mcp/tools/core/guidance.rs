//! Request-local rendering context for Cassy-authored recovery templates.
//! This never transforms response text or participates in authorization.
use crate::mcp::server::CasCore;

tokio::task_local! {
    static CALLER_PREFIX: &'static str;
}

pub(crate) async fn with_caller_prefix<T>(
    prefix: &'static str,
    future: impl std::future::Future<Output = T>,
) -> T {
    CALLER_PREFIX.scope(prefix, future).await
}

/// Free lifecycle gates use the service caller captured inside panic dispatch.
/// Direct core/CLI callers retain the existing process-own harness fallback.
pub(crate) fn caller_prefix() -> &'static str {
    CALLER_PREFIX
        .try_with(|prefix| *prefix)
        .unwrap_or_else(|_| crate::harness_policy::own_tool_prefix())
}

impl CasCore {
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
