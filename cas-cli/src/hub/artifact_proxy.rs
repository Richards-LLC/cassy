//! Session-scoped media tickets and bounded, credential-free blob streaming.
use super::*;
pub(crate) use crate::hub::auth::MediaTicket;
use cas_store::PublishedArtifact;
use cas_types::ARTIFACT_MAX_BYTES;
use std::io::{self, Read};

#[derive(Deserialize)]
pub(super) struct MediaQuery {
    t: Option<String>,
}

/// A route can add a restrictive policy without replacing the application CSP.
/// Multiple CSP fields are enforced together by browsers.
#[derive(Clone)]
pub(crate) struct AdditionalContentSecurityPolicy(pub HeaderValue);

const MEDIA_MIME_ALLOWLIST: &[&str] = &["image/*", "video/*", "audio/*", "application/pdf"];

fn mime_allowed(mime: &str, allowlist: &'static [&'static str]) -> bool {
    let valid_token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
    };
    let Some((family, subtype)) = mime.split_once('/') else {
        return false;
    };
    valid_token(family)
        && valid_token(subtype)
        && allowlist.iter().any(|allowed| {
            *allowed == mime
                || allowed
                    .strip_suffix("/*")
                    .is_some_and(|allowed_family| allowed_family == family)
        })
}

enum ArtifactSource {
    Cloud(std::path::PathBuf),
    Composer(std::fs::File),
}

struct ScopedArtifact {
    record: PublishedArtifact,
    source: ArtifactSource,
}

/// Resolve only a published thread ref or the session's own composer upload.
async fn scoped<R: SessionReadModel>(
    state: &HubState<R>,
    session: &str,
    artifact: &str,
) -> Result<ScopedArtifact, Response> {
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
        if let Some(record) = store.get(&artifact).ok()?
            && record.task_id == format!("commander-{session}")
        {
            let file = super::composer_upload::open_preview(&root, &session, &record).ok()?;
            return Some(ScopedArtifact {
                record,
                source: ArtifactSource::Composer(file),
            });
        }
        let record = store.get_for_session(&artifact, &session).ok()??;
        if record.status != "committed"
            || record
                .cloud_artifact_id
                .as_deref()
                .is_none_or(str::is_empty)
            || record.size_bytes == 0
            || record.size_bytes > ARTIFACT_MAX_BYTES
        {
            return None;
        }
        Some(ScopedArtifact {
            record,
            source: ArtifactSource::Cloud(root),
        })
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

/// Issue a reusable ticket only for a live SessionRead context and a committed
/// ref in that session's thread or its own upload. `allowlist` is a static, server-owned MIME list,
/// never request input; page routes can admit HTML without relaxing media routes.
pub(crate) async fn issue_session_artifact_ticket<R: SessionReadModel>(
    state: &HubState<R>,
    context: &AuthContext,
    session: &str,
    artifact: &str,
    allowlist: &'static [&'static str],
) -> Result<MediaTicket, Response> {
    let auth = state.auth.as_ref().ok_or_else(unauthorized)?;
    if !context.has(Scope::SessionRead) {
        return Err(refusal(&ScopeDenied.into()));
    }
    auth.ensure_active_context(context, chrono::Utc::now())
        .map_err(|error| refusal(&error))?;
    let ScopedArtifact { record, .. } = scoped(state, session, artifact).await?;
    if !mime_allowed(&record.mime, allowlist) {
        return Err(generic_not_found());
    }
    auth.issue_media_ticket(context, session, artifact, chrono::Utc::now())
        .map_err(|error| refusal(&error))
}

/// The opaque URL itself supplies the bound session after HMAC, expiry and
/// pairing-generation checks. Call fetch_session_artifact next to recheck the
/// thread ref, live permission and the route's own MIME allowlist.
pub(crate) fn authenticate_session_artifact_ticket<R: SessionReadModel>(
    state: &HubState<R>,
    ticket: &str,
    artifact: &str,
) -> Result<(String, AuthContext), Response> {
    state
        .auth
        .as_ref()
        .ok_or_else(unauthorized)?
        .authenticate_artifact_ticket(ticket, artifact, chrono::Utc::now())
        .map_err(|error| refusal(&error))
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
    match issue_session_artifact_ticket(&state, &context, &session, &artifact, MEDIA_MIME_ALLOWLIST)
        .await
    {
        Ok(ticket) => private(Json(ticket).into_response(), &headers),
        Err(response) => private(response, &headers),
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
    if state.auth.is_none() {
        return private(unauthorized(), &headers);
    }
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
        let Some(ticket) = query.t.as_deref() else {
            return private(unauthorized(), &headers);
        };
        match authenticate_session_artifact_ticket(&state, ticket, &artifact) {
            Ok((bound_session, context))
                if bound_session == session
                    && origin(&headers)
                        .is_none_or(|origin| origin == context.controller_origin) =>
            {
                context
            }
            Ok(_) => return private(unauthorized(), &headers),
            Err(response) => return private(response, &headers),
        }
    };
    fetch_session_media(&state, context, &session, &artifact, &headers).await
}

/// Fixed media facade: callers cannot relax image/video/audio/PDF eligibility.
pub(crate) async fn fetch_session_media<R: SessionReadModel>(
    state: &HubState<R>,
    context: AuthContext,
    session: &str,
    artifact: &str,
    headers: &HeaderMap,
) -> Response {
    if origin(headers).is_some_and(|origin| origin != context.controller_origin) {
        return private(unauthorized(), headers);
    }
    let mut response = fetch_session_artifact(
        state,
        context,
        session,
        artifact,
        headers,
        MEDIA_MIME_ALLOWLIST,
    )
    .await;
    if response.status().is_success()
        && response.headers().get("content-type").is_some_and(|mime| {
            mime.to_str()
                .is_ok_and(|mime| mime.eq_ignore_ascii_case("image/svg+xml"))
        })
    {
        response
            .extensions_mut()
            .insert(AdditionalContentSecurityPolicy(HeaderValue::from_static(
                "default-src 'none'; sandbox",
            )));
    }
    response
}

/// Fetch only a server-resolved, session-referenced artifact. All consumers
/// retain the 25 MiB ceiling, reusable Range handling, cancellation/backpressure,
/// no-store caching and no upstream bearer forwarding. A caller admitting HTML
/// must apply its own sandbox/CSP and HTTP Origin policy before returning it.
/// Origin binding for the media route remains in the fixed media facade.
pub(crate) async fn fetch_session_artifact<R: SessionReadModel>(
    state: &HubState<R>,
    context: AuthContext,
    session: &str,
    artifact: &str,
    headers: &HeaderMap,
    allowlist: &'static [&'static str],
) -> Response {
    let Some(auth) = state.auth.clone() else {
        return private(unauthorized(), headers);
    };
    if !context.has(Scope::SessionRead) {
        return private(refusal(&ScopeDenied.into()), headers);
    }
    if let Err(error) = auth.ensure_active_context(&context, chrono::Utc::now()) {
        return private(refusal(&error), headers);
    }
    let ScopedArtifact { record, source } = match scoped(state, session, artifact).await {
        Ok(found) => found,
        Err(response) => return private(response, headers),
    };
    if !mime_allowed(&record.mime, allowlist) || HeaderValue::from_str(&record.mime).is_err() {
        return private(generic_not_found(), headers);
    }
    let requested = match range(headers, record.size_bytes) {
        Ok(range) => range,
        Err(()) => return private(unsatisfied(record.size_bytes), headers),
    };
    let (start, end) = requested.unwrap_or((0, record.size_bytes - 1));
    let size = record.size_bytes;
    let count = end - start + 1;
    let mime = record.mime.clone();
    #[cfg(test)]
    let injected = state.artifact_cloud_client.clone();
    #[cfg(test)]
    let stream_clock = state.artifact_stream_clock.clone();
    let (head_tx, head_rx) = tokio::sync::oneshot::channel();
    let (body_tx, body_rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, io::Error>>(4);
    tokio::task::spawn_blocking(move || {
        // Resolve only the server-owned cloud id after all local checks. The
        // signed URL and upstream errors never appear in client errors/logs.
        let opened = (|| -> anyhow::Result<(Box<dyn Read + Send + Sync>, bool)> {
            auth.ensure_active_context(&context, chrono::Utc::now())?;
            let root = match source {
                ArtifactSource::Composer(file) => return Ok((Box::new(file), false)),
                ArtifactSource::Cloud(root) => root,
            };
            #[cfg(test)]
            let client = injected.or_else(|| crate::artifacts::cloud_client(&root));
            #[cfg(not(test))]
            let client = crate::artifacts::cloud_client(&root);
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
                // A paused downstream consumer can backpressure this stream for
                // longer than two minutes. Bound socket inactivity, not total
                // body lifetime; a client can re-ticket and resume after failure.
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(120))
                .timeout_write(Duration::from_secs(120))
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
            let mut last_auth_check: Option<Instant> = None;
            while remaining > 0 {
                let mut chunk = vec![0; remaining.min(64 * 1024) as usize];
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    return Err(io::Error::other("short media stream"));
                }
                // Check after reading, so a stalled upstream cannot release a
                // chunk using authorization from before the pause. The eager
                // checks above still authorize before resolving or fetching bytes.
                #[cfg(test)]
                let now = stream_clock
                    .as_ref()
                    .map_or_else(Instant::now, |clock| clock());
                #[cfg(not(test))]
                let now = Instant::now();
                if last_auth_check
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
                {
                    if auth
                        .ensure_active_context(&context, chrono::Utc::now())
                        .is_err()
                    {
                        return Err(io::Error::other("media authorization ended"));
                    }
                    last_auth_check = Some(now);
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
        return private(StatusCode::BAD_GATEWAY.into_response(), headers);
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
    private(response, headers)
}
