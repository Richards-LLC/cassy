//! Session-scoped media tickets and bounded, credential-free blob streaming.
use super::*;
use cas_store::PublishedArtifact;
use cas_types::ARTIFACT_MAX_BYTES;
use std::io::{self, Read};

#[derive(Deserialize)]
pub(super) struct MediaQuery {
    t: Option<String>,
}

fn media_mime(mime: &str) -> Option<&str> {
    let valid_token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
    };
    let (family, subtype) = mime.split_once('/')?;
    (valid_token(subtype)
        && (matches!(family, "image" | "video" | "audio") || mime == "application/pdf"))
        .then_some(mime)
}

/// Fetch no URL until both the session and the thread's published ref match.
async fn scoped<R: SessionReadModel>(
    state: &HubState<R>,
    session: &str,
    artifact: &str,
) -> Result<(std::path::PathBuf, PublishedArtifact), Response> {
    let found = state
        .catalog
        .list()
        .await
        .map_err(|_| generic_not_found())?
        .into_iter()
        .find(|candidate| candidate.name == session)
        .ok_or_else(generic_not_found)?;
    let project = found.project_dir.ok_or_else(generic_not_found)?;
    let artifact = artifact.to_owned();
    let session = session.to_owned();
    tokio::task::spawn_blocking(move || {
        let root = crate::store::find_cas_root_ignoring_env(std::path::Path::new(&project)).ok()?;
        let store = cas_store::SqliteArtifactStore::open(&root).ok()?;
        let record = store.get_for_session(&artifact, &session).ok()??;
        if record.status != "committed"
            || record
                .cloud_artifact_id
                .as_deref()
                .is_none_or(str::is_empty)
            || record.size_bytes == 0
            || record.size_bytes > ARTIFACT_MAX_BYTES
            || media_mime(&record.mime).is_none()
        {
            return None;
        }
        Some((root, record))
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(generic_not_found)
}

fn private(mut response: Response, headers: &HeaderMap) -> Response {
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("private, no-store"),
    );
    with_cors(response, headers)
}

fn refusal(error: &anyhow::Error) -> Response {
    if error.is::<ScopeDenied>() {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"forbidden"})),
        )
            .into_response()
    } else {
        unauthorized_for(error)
    }
}

pub(super) async fn ticket<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path((session, artifact)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/sessions/{session}/artifacts/{artifact}/ticket");
    let context = match authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "POST",
        &uri,
    ) {
        Ok(Some(context)) => context,
        Ok(None) => return private(unauthorized(), &headers),
        Err(error) => return private(refusal(&error), &headers),
    };
    if let Err(response) = scoped(&state, &session, &artifact).await {
        return private(response, &headers);
    }
    match state
        .auth
        .as_ref()
        .expect("authenticated store")
        .issue_media_ticket(&context, &session, &artifact, chrono::Utc::now())
    {
        Ok(ticket) => private(Json(ticket).into_response(), &headers),
        Err(_) => private(unauthorized(), &headers),
    }
}

pub(super) async fn preflight<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    uri: OriginalUri,
    headers: HeaderMap,
) -> Response {
    let mut response = super::preflight(State(state), uri, headers).await;
    if response.status() == StatusCode::NO_CONTENT {
        response.headers_mut().insert(
            "access-control-allow-headers",
            HeaderValue::from_static("Authorization, DPoP, Content-Type, Range"),
        );
    }
    response
}

/// One byte range; metadata bounds it before any upstream request.
fn range(headers: &HeaderMap, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(header) = headers.get("range") else {
        return Ok(None);
    };
    let (start, end) = header
        .to_str()
        .map_err(|_| ())?
        .strip_prefix("bytes=")
        .ok_or(())?
        .split_once('-')
        .ok_or(())?;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start = start.parse::<u64>().map_err(|_| ())?;
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>().map_err(|_| ())?.min(size - 1)
        };
        (start, end)
    };
    if start >= size || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn unsatisfied(size: u64) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    response.headers_mut().insert(
        "content-range",
        HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
    );
    response
}

pub(super) async fn content<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path((session, artifact)): Path<(String, String)>,
    Query(query): Query<MediaQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(auth) = state.auth.clone() else {
        return private(unauthorized(), &headers);
    };
    let uri = format!("/v1/sessions/{session}/artifacts/{artifact}/content");
    let context = if headers.contains_key("authorization") {
        match authorize(
            &state,
            HubAction::SessionRead,
            Scope::SessionRead,
            &headers,
            "GET",
            &uri,
        ) {
            Ok(Some(context)) => context,
            Err(error) => return private(refusal(&error), &headers),
            _ => return private(unauthorized(), &headers),
        }
    } else {
        match query.t.as_deref().and_then(|ticket| {
            auth.authenticate_media_ticket(ticket, &session, &artifact, chrono::Utc::now())
                .ok()
        }) {
            Some(context)
                if origin(&headers).is_none_or(|origin| origin == context.controller_origin) =>
            {
                context
            }
            _ => return private(unauthorized(), &headers),
        }
    };
    let (root, record) = match scoped(&state, &session, &artifact).await {
        Ok(found) => found,
        Err(response) => return private(response, &headers),
    };
    let requested = match range(&headers, record.size_bytes) {
        Ok(range) => range,
        Err(()) => return private(unsatisfied(record.size_bytes), &headers),
    };
    let (start, end) = requested.unwrap_or((0, record.size_bytes - 1));
    let size = record.size_bytes;
    let count = end - start + 1;
    let mime = record.mime.clone();
    #[cfg(test)]
    let injected = state.artifact_cloud_client.clone();
    let (head_tx, head_rx) = tokio::sync::oneshot::channel();
    let (body_tx, body_rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, io::Error>>(4);
    tokio::task::spawn_blocking(move || {
        // Resolve only the server-owned cloud id after all local checks. The
        // signed URL and upstream errors never appear in client errors/logs.
        #[cfg(test)]
        let client = injected.or_else(|| crate::artifacts::cloud_client(&root));
        #[cfg(not(test))]
        let client = crate::artifacts::cloud_client(&root);
        let opened = (|| -> anyhow::Result<_> {
            auth.ensure_active_context(&context, chrono::Utc::now())?;
            let client = client.context("media unavailable")?;
            let view = client.view_url(
                record
                    .cloud_artifact_id
                    .as_deref()
                    .context("media unavailable")?,
            )?;
            if let Some(expiry) = view.expires_at.as_deref() {
                anyhow::ensure!(
                    chrono::DateTime::parse_from_rfc3339(expiry)? > chrono::Utc::now(),
                    "media unavailable"
                );
            }
            let url = url::Url::parse(&view.url)?;
            anyhow::ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && (url.scheme() == "https"
                        || (url.scheme() == "http"
                            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")))),
                "media unavailable"
            );
            let agent = ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(120))
                .build();
            let mut request = agent.get(url.as_str()).set("Accept-Encoding", "identity");
            if requested.is_some() {
                request = request.set("Range", &format!("bytes={start}-{end}"));
            }
            let response = request.call()?;
            let partial = response.status() == 206;
            anyhow::ensure!(
                response.status() == 200 || (partial && requested.is_some()),
                "media unavailable"
            );
            if let Some(length) = response.header("content-length") {
                anyhow::ensure!(
                    length.parse::<u64>()? <= ARTIFACT_MAX_BYTES,
                    "media unavailable"
                );
            }
            if partial {
                anyhow::ensure!(
                    response.header("content-range")
                        == Some(format!("bytes {start}-{end}/{size}").as_str()),
                    "media unavailable"
                );
            }
            Ok((response.into_reader(), partial))
        })();
        let (mut reader, partial) = match opened {
            Ok(opened) => opened,
            Err(_) => {
                let _ = head_tx.send(false);
                return;
            }
        };
        if head_tx.send(true).is_err() {
            return;
        }
        let transfer = (|| -> io::Result<()> {
            if !partial && start != 0 {
                let skipped = io::copy(&mut reader.by_ref().take(start), &mut io::sink())?;
                if skipped != start {
                    return Err(io::Error::other("short media stream"));
                }
            }
            let mut remaining = count;
            while remaining > 0 {
                if auth
                    .ensure_active_context(&context, chrono::Utc::now())
                    .is_err()
                {
                    return Err(io::Error::other("media authorization ended"));
                }
                let mut chunk = vec![0; remaining.min(64 * 1024) as usize];
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    return Err(io::Error::other("short media stream"));
                }
                chunk.truncate(read);
                remaining -= read as u64;
                if body_tx.blocking_send(Ok(chunk)).is_err() {
                    return Ok(());
                }
            }
            // A full or upstream-partial body must match its metadata exactly.
            if (partial || requested.is_none()) && reader.read(&mut [0u8; 1])? != 0 {
                return Err(io::Error::other("oversize media stream"));
            }
            Ok(())
        })();
        if transfer.is_err() {
            // io/transport errors may contain a capability URL; emit a fixed error.
            let _ = body_tx.blocking_send(Err(io::Error::other("media stream interrupted")));
        }
    });
    if head_rx.await != Ok(true) {
        return private(StatusCode::BAD_GATEWAY.into_response(), &headers);
    }
    let stream = stream::unfold(body_rx, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = if requested.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let out = response.headers_mut();
    out.insert(
        "content-type",
        HeaderValue::from_str(&mime).expect("validated media MIME"),
    );
    out.insert(
        "content-length",
        HeaderValue::from_str(&count.to_string()).unwrap(),
    );
    out.insert("accept-ranges", HeaderValue::from_static("bytes"));
    out.insert(
        "access-control-expose-headers",
        HeaderValue::from_static("Content-Type, Content-Length, Content-Range, Accept-Ranges"),
    );
    if requested.is_some() {
        out.insert(
            "content-range",
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).unwrap(),
        );
    }
    private(response, &headers)
}
