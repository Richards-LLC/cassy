//! A Commander command's retained receipt, read after its daemon is gone
//! (cas-b521). The daemon is the authority while it runs; this route only
//! reads the row it left in the project store and never writes.
use super::*;
use crate::ui::factory::SessionMetadata;
use crate::ui::factory::daemon::runtime::supervisor_commands::{Birth, SavedRecord};
use crate::ui::factory::supervisor_commands::{SupervisorCommandPhase as Phase, SupervisorCommandReceipt as Receipt};

enum Retained {
    Found(Receipt),
    /// The recording daemon may still type or finish this command.
    Live,
    Missing,
}

fn valid_client_ref(client_ref: &str) -> bool {
    !client_ref.is_empty() && client_ref.len() <= 128
        && client_ref.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn cas_root(metadata: &SessionMetadata) -> Option<std::path::PathBuf> {
    metadata.cas_root.as_deref().map(std::path::PathBuf::from).or_else(|| {
        crate::store::find_cas_root_ignoring_env(std::path::Path::new(metadata.project_dir.as_deref()?)).ok()
    })
}

/// Terminal rows are final whoever asks. An open row is decided only once the
/// recording birth is gone: the claim commits before any PTY byte, so a queued
/// row of a dead birth was never typed, and no other birth may restore it.
fn classify(saved: SavedRecord, birth: &SessionMetadata) -> Retained {
    let mut receipt = saved.receipt;
    // A reader cannot step a chooser, so no retained receipt is interactive.
    receipt.interactive = false;
    if !matches!(receipt.phase, Phase::Queued | Phase::Submitted) { return Retained::Found(receipt); }
    if crate::ui::factory::daemon_identity_is_live(birth) { return Retained::Live; }
    if receipt.phase == Phase::Queued && saved.birth.matches(&Birth::of(birth)) {
        receipt.phase = Phase::Refused;
        receipt.detail = "Command not sent: its factory session ended before it could be typed.".into();
    } else if receipt.phase == Phase::Queued {
        receipt.phase = Phase::Unconfirmed;
        receipt.detail = "This command was queued in an earlier session; whether it was typed is unknown. Do not resend automatically.".into();
    } else {
        receipt.phase = Phase::Unconfirmed;
        receipt.detail = "The session ended after this command was typed; its outcome was not confirmed. Do not resend automatically.".into();
    }
    Retained::Found(receipt)
}

fn retained(session: &str, device_id: &str, client_ref: &str) -> anyhow::Result<Retained> {
    // One birth owns the name; an older birth's rows are never consulted.
    let Some(birth) = crate::ui::factory::SessionManager::new().recorded_birth(session) else { return Ok(Retained::Missing) };
    let Some(root) = cas_root(&birth) else { return Ok(Retained::Missing) };
    let Some(row) = crate::store::supervisor_commands::read(&root, session, &birth.supervisor.name, device_id, client_ref)? else { return Ok(Retained::Missing) };
    // The daemon skips an unreadable row; so does its reader.
    let Ok(saved) = serde_json::from_value::<SavedRecord>(row) else { return Ok(Retained::Missing) };
    if saved.receipt.device_id != device_id || saved.receipt.client_ref != client_ref { return Ok(Retained::Missing); }
    Ok(classify(saved, &birth))
}

pub(super) async fn read<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path((session, client_ref)): Path<(String, String)>,
    request: Request<Body>,
) -> Response {
    let headers = request.headers().clone();
    let uri = request.uri().path().to_owned();
    let context = match authorize(&state, HubAction::SessionRead, Scope::PaneRead, &headers, "GET", &uri) {
        Ok(Some(context)) => context,
        // Commands belong to an operator-verified device; there is no anonymous owner.
        Ok(None) => return with_cors(unauthorized(), &headers),
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    let Some(auth) = state.auth.clone() else { return with_cors(unauthorized(), &headers) };
    // Validate before any path is joined; every miss looks the same.
    if !valid_session_name(&session) || !valid_client_ref(&client_ref) {
        return with_cors(generic_not_found(), &headers);
    }
    let device = context.device_id.clone();
    let (name, reference) = (session.clone(), client_ref.clone());
    let outcome = tokio::task::spawn_blocking(move || retained(&name, &device, &reference)).await;
    let response = match outcome {
        Ok(Ok(Retained::Found(receipt))) => {
            let outcome = match receipt.phase {
                Phase::Queued => "queued",
                Phase::Submitted => "submitted",
                Phase::Observed => "observed",
                Phase::Refused => "denied",
                Phase::Unconfirmed => "unconfirmed",
                Phase::Cancelled => "cancelled",
            };
            let detail = serde_json::json!({"client_ref": receipt.client_ref, "command": receipt.command, "phase": receipt.phase, "source": "retained"}).to_string();
            if let Err(error) = auth.audit_operation(&context, outcome, "supervisor_command_receipt", Scope::PaneRead, &session, Some(detail), chrono::Utc::now()) {
                return with_cors(internal_error(error), &headers);
            }
            Json(receipt).into_response()
        }
        Ok(Ok(Retained::Live)) => (StatusCode::CONFLICT, Json(serde_json::json!({"error":"session_live"}))).into_response(),
        Ok(Ok(Retained::Missing)) => generic_not_found(),
        Ok(Err(error)) => {
            tracing::warn!(%error, "retained command receipt unavailable");
            (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"store_unavailable","retryable":true}))).into_response()
        }
        Err(error) => internal_error(error.into()),
    };
    with_cors(response, &headers)
}
