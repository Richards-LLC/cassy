//! Browser attachments stay on this machine; ordinary SendMessage delivers their paths.
use super::*;
use base64::Engine;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path as FsPath, PathBuf};

const MAX_BYTES: usize = 10 * 1024 * 1024;
// Base64 expansion plus JSON/name overhead, bounded before parsing or decoding.
const MAX_WIRE_BYTES: usize = 14 * 1024 * 1024;

#[derive(Deserialize)]
struct UploadRequest {
    name: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct UploadedFile {
    artifact_id: String,
    path: String,
    mime: String,
}

pub(super) async fn upload<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path((session, upload_id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response {
    let headers = request.headers().clone();
    let context = match authorize(
        &state,
        HubAction::Mutation,
        Scope::MessageSend,
        &headers,
        "POST",
        request.uri().path(),
    ) {
        Ok(Some(context)) => context,
        Ok(None) => return with_cors(unauthorized(), &headers),
        Err(error) if error.to_string() == "scope denied" => {
            return with_cors(
                launch_error(
                    StatusCode::FORBIDDEN,
                    "scope_denied",
                    "This pairing does not allow sending files.",
                ),
                &headers,
            );
        }
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    // A machine pairing is machine-wide. Only its own catalog may resolve a project;
    // neither a client path nor the hub process's ambient CAS_ROOT can select it.
    let catalog = match state.catalog.list().await {
        Ok(catalog) => catalog,
        Err(error) => return with_cors(internal_error(error), &headers),
    };
    let Some(project) = catalog
        .iter()
        .find(|item| item.name == session)
        .and_then(|item| item.project_dir.clone())
    else {
        return with_cors(
            launch_error(
                StatusCode::FORBIDDEN,
                "session_denied",
                "This session is not available on this machine.",
            ),
            &headers,
        );
    };
    if uuid::Uuid::parse_str(&upload_id).is_err() || !valid_session_name(&session) {
        return with_cors(
            launch_error(
                StatusCode::BAD_REQUEST,
                "invalid_upload",
                "Choose the file again; its upload identity is invalid.",
            ),
            &headers,
        );
    }
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_WIRE_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return with_cors(too_large(), &headers),
    };
    let request: UploadRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return with_cors(
                launch_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_upload",
                    "The upload is not valid JSON.",
                ),
                &headers,
            );
        }
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&request.data) {
        Ok(bytes) if bytes.len() <= MAX_BYTES => bytes,
        Ok(_) => return with_cors(too_large(), &headers),
        Err(_) => {
            return with_cors(
                launch_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_upload",
                    "The file could not be decoded. Choose it again.",
                ),
                &headers,
            );
        }
    };
    let Some(mime) = sniff_mime(&bytes) else {
        return with_cors(
            launch_error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_file",
                "This file type is not supported. Send an image, text, PDF, ZIP, audio or video file.",
            ),
            &headers,
        );
    };
    let outcome = tokio::task::spawn_blocking(move || {
        save(
            &project,
            &session,
            &context.device_id,
            &upload_id,
            &request.name,
            &bytes,
            mime,
        )
    })
    .await;
    let response = match outcome {
        Ok(Ok(Some(file))) => Json(file).into_response(),
        Ok(Ok(None)) => launch_error(
            StatusCode::CONFLICT,
            "upload_conflict",
            "This upload identity already holds different bytes. Remove the file and choose it again.",
        ),
        other => {
            tracing::warn!(?other, "Commander attachment could not be stored");
            launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upload_storage_failed",
                "Could not store the file on this machine. Your file is kept in this page; try sending again.",
            )
        }
    };
    with_cors(response, &headers)
}

fn too_large() -> Response {
    launch_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "file_too_large",
        "Each file must be no larger than 10 MiB.",
    )
}

/// No MIME or extension supplied by the browser is authoritative.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        Some("application/zip")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Some("video/mp4")
    } else if bytes.starts_with(b"ID3") || bytes.starts_with(b"\xff\xfb") {
        Some("audio/mpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some("audio/wav")
    } else if !bytes.is_empty()
        && std::str::from_utf8(bytes).is_ok_and(|text| {
            text.chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        })
    {
        Some("text/plain")
    } else {
        None
    }
}

fn safe_name(name: &str) -> String {
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .take(120)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        "attachment".into()
    } else {
        name
    }
}

/// New directories are private. Existing namespace components must be real directories.
fn private_directory(path: &FsPath) -> anyhow::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "attachment directory is not a real directory"
    );
    Ok(())
}

fn private_file(path: &FsPath, create_new: bool) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

/// A process-independent lock spans file persistence and artifact registration, so a lost
/// response or hub restart replays the same identity rather than minting another artifact.
fn save(
    project: &str,
    session: &str,
    device: &str,
    upload_id: &str,
    name: &str,
    bytes: &[u8],
    mime: &str,
) -> anyhow::Result<Option<UploadedFile>> {
    let root = crate::store::find_cas_root_ignoring_env(FsPath::new(project))?;
    let config = crate::config::Config::load(&root)?;
    let base =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    std::fs::create_dir_all(&base)?;
    let mut directory = base.canonicalize()?;
    let project_dir = crate::config::project_factory_artifacts_root(&root, &directory);
    private_directory(&project_dir)?;
    directory = project_dir.join("commander");
    private_directory(&directory)?;
    directory.push(session);
    private_directory(&directory)?;
    let identity = format!("{device}\0{session}\0{upload_id}");
    let artifact_id = format!("art-{:x}", Sha256::digest(identity.as_bytes()));
    directory.push(&artifact_id);
    private_directory(&directory)?;
    let lock = private_file(&directory.join(".lock"), false)?;
    lock.lock_exclusive()?;
    let store = cas_store::SqliteArtifactStore::open(&root)?;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let existing = store.get(&artifact_id)?;
    if existing.as_ref().is_some_and(|row| row.sha256 != sha256) {
        return Ok(None);
    }
    let name = existing
        .as_ref()
        .map(|row| row.name.clone())
        .unwrap_or_else(|| safe_name(name));
    let path: PathBuf = directory.join(&name);
    if path.exists() {
        anyhow::ensure!(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "attachment file is a symlink"
        );
        if crate::artifacts::digest_file(&path)?.0 != sha256 {
            return Ok(None);
        }
    } else {
        let pending = directory.join(".pending");
        // Only this upload's private scratch file can remain after an interrupted write.
        if pending.exists() {
            std::fs::remove_file(&pending)?;
        }
        let mut file = private_file(&pending, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(pending, &path)?;
        File::open(&directory)?.sync_all()?;
    }
    if existing.is_none() {
        store.record_local(&cas_store::NewArtifact {
            id: artifact_id.clone(),
            task_id: format!("commander-{session}"),
            name,
            mime: mime.into(),
            size_bytes: bytes.len() as u64,
            sha256,
        })?;
    }
    Ok(Some(UploadedFile {
        artifact_id,
        path: path.display().to_string(),
        mime: mime.into(),
    }))
}
