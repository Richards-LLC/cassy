//! Accounts transport: paired-device scopes, revocation check and durable audit
//! admission surround the existing local lifecycle engine.
use super::super::accounts::{self, Operation};
use super::*;

pub(super) async fn list<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/accounts",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
    }
    let result = tokio::task::spawn_blocking(|| {
        let home = dirs::home_dir().context("home unavailable")?;
        accounts::catalog(&home)
    })
    .await;
    let response = match result {
        Ok(Ok(catalog)) => Json(catalog).into_response(),
        _ => launch_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "accounts_unavailable",
            "Could not read accounts. Retry on this machine.",
        ),
    };
    with_cors(response, &headers)
}

pub(super) async fn operate<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(op): Json<Operation>,
) -> Response {
    let context = match authorize(
        &state,
        HubAction::Mutation,
        Scope::FactoryManage,
        &headers,
        "POST",
        "/v1/accounts/operations",
    ) {
        Ok(Some(context)) => context,
        Ok(None) => return with_cors(unauthorized(), &headers),
        Err(error) if error.is::<ScopeDenied>() => return with_cors(
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error":"scope_denied","required_scope":"factory:manage"})),
            )
                .into_response(),
            &headers,
        ),
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    if let Err(error) = op.validate() {
        return with_cors(
            launch_error(
                StatusCode::BAD_REQUEST,
                "invalid_operation",
                &error.to_string(),
            ),
            &headers,
        );
    }
    let Some(auth) = state.auth.clone() else {
        return with_cors(unauthorized(), &headers);
    };
    // Account and fleet mutations share serialization, but never replay one
    // another's responses. The browser does not automatically retry mutations.
    let _lock = state.operations.lock().await;
    let result = tokio::task::spawn_blocking(move || {
        let action = format!("accounts:{}", op.operation);
        auth.ensure_active_context(&context, chrono::Utc::now())?;
        auth.audit_operation(
            &context,
            "requested",
            &action,
            Scope::FactoryManage,
            "",
            Some(op.account.clone()),
            chrono::Utc::now(),
        )?;
        let outcome = dirs::home_dir()
            .context("home unavailable")
            .and_then(|home| accounts::apply(&home, &op));
        let verdict = if outcome.is_ok() { "allowed" } else { "failed" };
        // Report an outcome-audit failure even if disk already changed; the UI
        // refreshes after a refusal and never claims the mutation rolled back.
        auth.audit_operation(
            &context,
            verdict,
            &action,
            Scope::FactoryManage,
            "",
            Some(op.account),
            chrono::Utc::now(),
        )?;
        Ok::<_, anyhow::Error>(outcome)
    })
    .await;
    let response = match result {
        Ok(Ok(Ok(()))) => Json(serde_json::json!({"ok":true})).into_response(),
        Ok(Ok(Err(error))) => {
            tracing::warn!(%error, "Account mutation failed; disk may have changed");
            launch_error(
                StatusCode::CONFLICT,
                "account_change_failed",
                "The account could not be changed completely. Refresh the list and check cas accounts on the machine; protected or active accounts cannot be removed or renamed.",
            )
        }
        _ => launch_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_or_accounts_unavailable",
            "The operation could not be verified. Refresh accounts before trying again.",
        ),
    };
    with_cors(response, &headers)
}
