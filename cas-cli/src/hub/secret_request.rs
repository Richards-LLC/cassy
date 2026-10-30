//! A secret an agent asked for is saved on this machine, not sent (cas-1996).
//!
//! The supervisor's `kind=secret` turn holds only the request: the name (its
//! summary) and what it is for. Commander answers it here, never over
//! `SendMessage`. The value goes straight to a mode-600 file in the project's
//! `secrets.destination`; the queue row, the audit, the reply and the logs
//! carry the name and path only. The supervisor gets a reply bound to the
//! request that names the file.
use super::*;
use std::io::Write;
use std::path::{Path as FsPath, PathBuf};

/// One pasted credential, with JSON overhead. Bounded before parsing.
const MAX_WIRE_BYTES: usize = 16 * 1024;

/// The browser's answer. No `Debug`: it holds the value.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretAnswer {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    decline: bool,
}

/// Saving or declining, decided once the request is found open.
enum Outcome {
    Saved(PathBuf),
    Declined,
}

/// Why an answer was refused, in words that never include the value.
enum Refusal {
    NotOpen,
    Storage(anyhow::Error),
}

/// One answer at a time, so two devices cannot both save and reply.
static ANSWERING: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) async fn answer<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path((session, notification_id)): Path<(String, i64)>,
    request: Request<Body>,
) -> Response {
    let headers = request.headers().clone();
    let uri = request.uri().path().to_owned();
    // Handing an agent a credential is a management action.
    let scope = Scope::FactoryManage;
    let context = match authorize(&state, HubAction::Mutation, scope, &headers, "POST", &uri) {
        Ok(Some(context)) => context,
        Ok(None) => return with_cors(unauthorized(), &headers),
        Err(error) if error.to_string() == "scope denied" => {
            return with_cors(
                (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error":"scope_denied", "required_scope":scope.as_str()})),
                )
                    .into_response(),
                &headers,
            );
        }
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    let Some(auth) = state.auth.clone() else {
        return with_cors(unauthorized(), &headers);
    };
    if auth.ensure_active_context(&context, chrono::Utc::now()).is_err() {
        return with_cors(
            launch_error(StatusCode::UNAUTHORIZED, "revoked", "device credential is no longer active"),
            &headers,
        );
    }
    let catalog = match state.catalog.list().await {
        Ok(catalog) => catalog,
        Err(error) => return with_cors(internal_error(error), &headers),
    };
    let Some(project) = valid_session_name(&session)
        .then(|| catalog.iter().find(|item| item.name == session))
        .flatten()
        .and_then(|item| item.project_dir.clone())
    else {
        return with_cors(generic_not_found(), &headers);
    };
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_WIRE_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return with_cors(
                launch_error(StatusCode::PAYLOAD_TOO_LARGE, "secret_too_large", "A secret must be no larger than 16 KiB."),
                &headers,
            );
        }
    };
    // serde errors can quote the input, so none reaches the response or a log.
    let answer = match serde_json::from_slice::<SecretAnswer>(&bytes) {
        Ok(SecretAnswer { value: None, decline: true }) => None,
        Ok(SecretAnswer { value: Some(value), decline: false }) if !value.trim().is_empty() => {
            Some(value.trim().to_owned())
        }
        _ => {
            return with_cors(
                launch_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_secret",
                    "Send {\"value\": \"...\"} with the secret, or {\"decline\": true}.",
                ),
                &headers,
            );
        }
    };
    drop(bytes);
    let action = if answer.is_some() { "secret_save" } else { "secret_decline" };
    let subject = format!("notification={notification_id}");
    if let Err(error) = auth.audit_operation(
        &context, "requested", action, scope, &session, Some(subject), chrono::Utc::now(),
    ) {
        return with_cors(
            launch_error(StatusCode::INTERNAL_SERVER_ERROR, "audit_unavailable", &error.to_string()),
            &headers,
        );
    }
    let attribution = verified_attribution(&context);
    let blocking_session = session.clone();
    // Keep the caller's subscriber, so what the blocking work logs is captured with it.
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let outcome = tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || {
            save(&project, &blocking_session, notification_id, answer.as_deref(), &attribution)
        })
    })
    .await;
    let (status, body, audit_outcome, detail) = match outcome {
        Ok(Ok((name, Outcome::Saved(path)))) => (
            StatusCode::OK,
            serde_json::json!({"name": name, "path": path.display().to_string(), "saved": true}),
            "allowed",
            format!("notification={notification_id} name={name} path={}", path.display()),
        ),
        Ok(Ok((name, Outcome::Declined))) => (
            StatusCode::OK,
            serde_json::json!({"name": name, "declined": true}),
            "allowed",
            format!("notification={notification_id} name={name} declined"),
        ),
        Ok(Err(Refusal::NotOpen)) => (
            StatusCode::CONFLICT,
            serde_json::json!({"error": "secret_not_open", "detail": "This secret request is not waiting in this session. It may already be answered."}),
            "invalid",
            format!("notification={notification_id} not open"),
        ),
        Ok(Err(Refusal::Storage(error))) => {
            tracing::warn!(%error, %session, notification_id, "cas-1996: secret could not be saved");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": "secret_storage_failed", "detail": "Could not save the secret on this machine. Nothing was sent; try again."}),
                "failed",
                format!("notification={notification_id} storage failed"),
            )
        }
        Err(error) => {
            tracing::warn!(%error, %session, notification_id, "cas-1996: secret save task failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": "secret_storage_failed", "detail": "Could not save the secret on this machine. Nothing was sent; try again."}),
                "failed",
                format!("notification={notification_id} storage failed"),
            )
        }
    };
    if let Err(error) =
        auth.audit_operation(&context, audit_outcome, action, scope, &session, Some(detail), chrono::Utc::now())
    {
        tracing::warn!(%error, %session, action, "cas-1996: secret outcome audit row could not be written");
    }
    with_cors((status, Json(body)).into_response(), &headers)
}

/// Find the open request, write or skip the file, then reply to the supervisor
/// with the path and close the request.
fn save(
    project: &str,
    session: &str,
    notification_id: i64,
    value: Option<&str>,
    attribution: &MessageAttribution,
) -> Result<(String, Outcome), Refusal> {
    let _answering = ANSWERING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let project = FsPath::new(project);
    let cas_dir = crate::store::find_cas_root_ignoring_env(project).map_err(Refusal::Storage)?;
    let queue = crate::store::open_prompt_queue_store(&cas_dir).map_err(Refusal::Storage)?;
    let request = queue
        .queued_prompt(notification_id)
        .map_err(|error| Refusal::Storage(error.into()))?
        .filter(|row| {
            row.source == "supervisor"
                && row.target.eq_ignore_ascii_case("operator")
                && row.kind.as_deref() == Some(crate::ui::factory::OperatorTurnKind::Secret.as_str())
                && row.factory_session.as_deref() == Some(session)
                && row.acked_at.is_none()
        })
        .ok_or(Refusal::NotOpen)?;
    let name = request.summary.as_deref().map(str::trim).unwrap_or_default().to_owned();
    if !crate::ui::factory::valid_secret_name(&name) {
        return Err(Refusal::NotOpen);
    }
    let (outcome, text, summary) = match value {
        Some(value) => {
            let directory = destination(&cas_dir, project).map_err(Refusal::Storage)?;
            let path = write_secret(&directory, &name, value).map_err(Refusal::Storage)?;
            let text = format!(
                "Secret {name} saved to {} (mode 600). Read it from that file when you need it; \
                 the value is not in this message or any history.",
                path.display()
            );
            (Outcome::Saved(path), text, format!("Secret {name} saved"))
        }
        None => (
            Outcome::Declined,
            format!("Declined the secret {name}. Nothing was saved."),
            format!("Secret {name} declined"),
        ),
    };
    let reply = crate::ops::fleet::enqueue_commander_message_with_client_ref(
        &cas_dir, session, "supervisor", &text, Some(&summary), false, Some(notification_id), None, attribution,
    )
    .map_err(Refusal::Storage)?;
    if matches!(reply, cas_store::EnqueueOutcome::Created(_)) {
        crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(&cas_dir);
    }
    Ok((name, outcome))
}

/// The project's `secrets.destination`, else the user default in
/// `~/.cas/config.toml`, else `~/.cas/private/creds/<project>`.
fn destination(cas_dir: &FsPath, project: &FsPath) -> anyhow::Result<PathBuf> {
    let configured = |dir: &FsPath| {
        crate::config::Config::load(dir)
            .ok()
            .and_then(|config| config.secrets)
            .and_then(|secrets| secrets.destination)
            .filter(|value| !value.trim().is_empty())
    };
    let host = crate::config::host_config_dir();
    let raw = configured(cas_dir)
        .or_else(|| (host != cas_dir && host.join("config.toml").exists()).then(|| configured(&host)).flatten())
        .unwrap_or_else(|| crate::config::DEFAULT_SECRETS_DESTINATION.to_owned());
    let project_name: String = project
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    anyhow::ensure!(!project_name.is_empty() && !project_name.starts_with('.'), "project has no usable name");
    let raw = raw.trim().replace("<project>", &project_name);
    let path = match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            dirs::home_dir().context("no home directory")?.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(&raw),
    };
    anyhow::ensure!(
        path.is_absolute() && path.components().all(|part| !matches!(part, std::path::Component::ParentDir)),
        "secrets.destination must be an absolute path"
    );
    Ok(path)
}

/// Create missing folders mode 700, then write `<directory>/<name>` mode 600
/// through a private temporary file and an atomic rename. A later answer to a
/// request with the same name replaces the file.
fn write_secret(directory: &FsPath, name: &str, value: &str) -> anyhow::Result<PathBuf> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let metadata = std::fs::symlink_metadata(directory)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "secret destination is not a real directory"
    );
    let path = directory.join(name);
    let pending = directory.join(format!(".{name}.{}.pending", uuid::Uuid::new_v4()));
    let written = (|| -> anyhow::Result<()> {
        let mut file = super::composer_upload::private_file(&pending, true)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&pending, &path)?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&pending);
    }
    written?;
    Ok(path)
}
