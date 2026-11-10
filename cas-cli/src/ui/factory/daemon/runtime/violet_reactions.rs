//! Bounded authenticated Violet calls from the existing blocking Slack tick.
#[cfg(feature = "mcp-proxy")]
pub(crate) fn reconcile(root: &std::path::Path, registration: &str, session: &str) {
    use super::violet_activity::ProxyChannelReader;
    use crate::cli::violet_hub::VioletError;
    use crate::coordination::external_work::CALL_REMOTE_BUDGET;
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let mut connection = None;
    let result = crate::coordination::source_reactions::reconcile(
        root,
        registration,
        session,
        chrono::Utc::now(),
        &mut |tool, args| {
            runtime.block_on(async {
                let unavailable = || {
                    let mut error = VioletError::local(
                        "violet_unavailable",
                        "Violet source status call failed or timed out",
                    );
                    error.retryable = true;
                    error
                };
                if connection.is_none() {
                    let caller = cmcp_core::ProxyCaller {
                        agent_id: registration.into(),
                        role: cas_types::AgentRole::Supervisor,
                        session_id: registration.into(),
                        factory_session: Some(session.into()),
                        active_task_ids: Vec::new(),
                    };
                    connection = Some(
                        tokio::time::timeout(
                            CALL_REMOTE_BUDGET,
                            ProxyChannelReader::connect(root, caller),
                        )
                        .await
                        .map_err(|_| unavailable())?
                        .map_err(|_| unavailable())?,
                    );
                }
                let reader = connection.as_ref().expect("connected above");
                tokio::time::timeout(CALL_REMOTE_BUDGET, reader.source_status(tool, args))
                    .await
                    .map_err(|_| unavailable())?
            })
        },
    );
    if let Some(reader) = connection {
        runtime.block_on(async {
            let _ = tokio::time::timeout(CALL_REMOTE_BUDGET, reader.shutdown()).await;
        });
    }
    if let Err(error) = result {
        tracing::warn!(%error, "source reaction reconciliation failed");
    }
}

#[cfg(not(feature = "mcp-proxy"))]
pub(crate) fn reconcile(_: &std::path::Path, _: &str, _: &str) {}
