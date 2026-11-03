//! Shared navigation tombstones beside the hub's pairing store. One atomic file
//! per generation makes concurrent dismissals additive across paired devices.
use super::*;
use sha2::{Digest, Sha256};
use std::path::{Path as FsPath, PathBuf};

fn marker(root: &FsPath, name: &str, started_at: &str) -> PathBuf {
    let identity = serde_json::to_vec(&(name, started_at)).expect("string identity");
    root.join("dismissed-sessions")
        .join(format!("{:x}.json", Sha256::digest(identity)))
}

pub(super) fn is_dismissed(root: &FsPath, session: &HubSession) -> anyhow::Result<bool> {
    let Some(started_at) = session
        .started_at
        .as_deref()
        .filter(|_| session.ended.is_some())
    else {
        return Ok(false);
    };
    match std::fs::metadata(marker(root, &session.name, started_at)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DismissRequest {
    started_at: String,
}

/// Authorizes a shared list change and binds it to this exact session birth.
async fn tombstone_path<R: SessionReadModel>(
    state: &HubState<R>,
    name: &str,
    action: &str,
    headers: &HeaderMap,
    started_at: &str,
) -> Result<PathBuf, Response> {
    let uri = format!("/v1/sessions/{name}/{action}");
    // This changes only the shared list preference, never the daemon or history.
    if let Err(error) = authorize(
        state,
        HubAction::Mutation,
        Scope::SessionRead,
        headers,
        "POST",
        &uri,
    ) {
        return Err(with_cors(unauthorized_for(&error), headers));
    }
    let Some(auth) = &state.auth else {
        return Err(with_cors(StatusCode::SERVICE_UNAVAILABLE.into_response(), headers));
    };
    let sessions = match state.catalog.list().await {
        Ok(rows) => rows,
        Err(error) => return Err(internal_error(error)),
    };
    let Some(session) = sessions.iter().find(|row| row.name == name) else {
        return Err(with_cors(StatusCode::NOT_FOUND.into_response(), headers));
    };
    if session.ended.is_none() || session.started_at.as_deref() != Some(started_at) {
        return Err(with_cors(StatusCode::CONFLICT.into_response(), headers));
    }
    Ok(marker(auth.root(), name, started_at))
}

pub(super) async fn dismiss<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DismissRequest>,
) -> Response {
    let path = match tombstone_path(&state, &name, "dismiss", &headers, &request.started_at).await {
        Ok(path) => path,
        Err(response) => return response,
    };
    let result = (|| -> anyhow::Result<()> {
        let dir = path.parent().expect("marker parent");
        std::fs::create_dir_all(dir)?;
        let temporary = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(
            temporary.as_file(),
            &serde_json::json!({ "session": name, "started_at": request.started_at }),
        )?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path)?;
        Ok(())
    })();
    if let Err(error) = result {
        return internal_error(error);
    }
    state.events.session_dismissed(&name);
    with_cors(StatusCode::NO_CONTENT.into_response(), &headers)
}

/// Lifts a dismissal for every paired device. Restoring twice is not an error.
pub(super) async fn restore<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DismissRequest>,
) -> Response {
    let path = match tombstone_path(&state, &name, "restore", &headers, &request.started_at).await {
        Ok(path) => path,
        Err(response) => return response,
    };
    match std::fs::remove_file(&path) {
        Ok(()) => state.events.session_restored(&name),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return internal_error(error.into()),
    }
    with_cors(StatusCode::NO_CONTENT.into_response(), &headers)
}
