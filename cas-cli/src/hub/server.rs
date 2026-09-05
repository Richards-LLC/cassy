use std::convert::Infallible;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, options, post};
use futures_util::{SinkExt, StreamExt, stream};
use serde::{Deserialize, Serialize};

use super::{
    AuthContext, AuthStore, DaemonConnector, HealthResponse, HubAction, HubAuthorizer, HubRequest,
    HubSession, MachineEventBus, MachineIdentity, MachineMetadata, PairingExchange,
    PairingExchangeError, ProxyFrame, ProxyFrameKind, Scope, SessionCatalog, SessionReadModel,
    TransportSecurity, ViewerRecvError, required_scope,
};
use crate::ui::factory::{ClientMessage, DaemonMessage, MessageAttribution, PaneSizeAuthority};

const MACHINE_PROTOCOL_VERSION: u32 = 2;
const MACHINE_PROTOCOL_MAGIC: &[u8; 4] = b"CAS2";

#[derive(Clone)]
pub struct HubState<R: SessionReadModel> {
    catalog: SessionCatalog<R>,
    authorizer: Arc<dyn HubAuthorizer>,
    machine: MachineIdentity,
    connector: DaemonConnector,
    events: MachineEventBus,
    auth: Option<AuthStore>,
    metadata: MachineMetadata,
    effective_origins: Vec<String>,
    response_transport: TransportSecurity,
}

impl<R: SessionReadModel> HubState<R> {
    pub fn new(
        catalog: SessionCatalog<R>,
        authorizer: Arc<dyn HubAuthorizer>,
        machine: MachineIdentity,
        connector: DaemonConnector,
        events: MachineEventBus,
    ) -> Self {
        Self {
            catalog,
            authorizer,
            machine,
            connector,
            events,
            auth: None,
            metadata: MachineMetadata::default(),
            effective_origins: Vec::new(),
            response_transport: TransportSecurity::Plaintext,
        }
    }

    pub fn with_auth(mut self, auth: AuthStore) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn with_machine_metadata(mut self, metadata: MachineMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_effective_origin(mut self, origin: impl Into<String>) -> Self {
        self.effective_origins.push(origin.into());
        self
    }

    /// Bind response policy to the server-owned listener, never request headers.
    pub fn with_response_transport(mut self, transport: TransportSecurity) -> Self {
        self.response_transport = transport;
        self
    }
}

pub fn router<R: SessionReadModel>(state: HubState<R>) -> Router {
    let response_transport = state.response_transport;
    Router::new()
        .route("/", get(commander_index))
        .route("/commander", get(commander_index))
        .route("/commander/", get(commander_index))
        .route("/commander/app.js", get(commander_javascript))
        .route("/commander/app.css", get(commander_stylesheet))
        .route("/commander/favicon.svg", get(commander_favicon))
        .route("/commander/ghostty-vt.wasm", get(commander_ghostty_wasm))
        .route(
            "/commander/ghostty-write-pty.wasm",
            get(commander_ghostty_write_wasm),
        )
        .route("/commander/symbols.woff2", get(commander_symbols_font))
        .route("/v1/health", get(health::<R>).options(preflight::<R>))
        .route(
            "/v1/auth/pairing/exchange",
            post(pairing_exchange::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/websocket-ticket",
            post(websocket_ticket::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/refresh",
            post(refresh_credential::<R>).options(preflight::<R>),
        )
        .route("/v1/machine", get(machine::<R>).options(preflight::<R>))
        .route(
            "/v1/diagnostics",
            get(diagnostics::<R>).options(preflight::<R>),
        )
        .route("/v1/sessions", get(sessions::<R>).options(preflight::<R>))
        .route("/v1/events", get(events::<R>).options(preflight::<R>))
        .route("/v1/attach", get(machine_attach::<R>))
        .route(
            "/v1/sessions/{session}/status",
            get(status::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/sessions/{session}/lease",
            get(lease_status::<R>)
                .post(acquire_lease::<R>)
                .delete(release_lease::<R>)
                .options(preflight::<R>),
        )
        .route("/v1/sessions/{session}/attach", get(attach::<R>))
        .route("/{*path}", options(preflight::<R>))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            response_transport,
            security_headers,
        ))
}

fn commander_asset(bytes: &'static [u8], content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    response
}

async fn commander_index() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/index.html"),
        "text/html; charset=utf-8",
    )
}

async fn commander_javascript() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/app.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn commander_stylesheet() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/app.css"),
        "text/css; charset=utf-8",
    )
}

async fn commander_favicon() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/favicon.svg"),
        "image/svg+xml",
    )
}

async fn commander_ghostty_wasm() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/ghostty-vt.wasm"),
        "application/wasm",
    )
}

async fn commander_ghostty_write_wasm() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/ghostty-write-pty.wasm"),
        "application/wasm",
    )
}

async fn commander_symbols_font() -> Response {
    commander_asset(
        include_bytes!("../../../hub-web/dist/symbols.woff2"),
        "font/woff2",
    )
}

async fn security_headers(
    State(transport): State<TransportSecurity>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if matches!(
        transport,
        TransportSecurity::Tls13 | TransportSecurity::TrustedLoopbackTlsProxy
    ) {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self' https: wss: http://127.0.0.1:* ws://127.0.0.1:*; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'; worker-src 'none'; manifest-src 'self'"),
    );
    response
}

async fn preflight<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let Some(origin) = origin(&headers) else {
        return unauthorized();
    };
    if uri.path() == "/v1/auth/pairing/exchange" {
        return pairing_preflight(&origin, &headers);
    }
    // A health probe contains only readiness data, so the reviewed hosted
    // Commander may read it before a credential exists. All other routes
    // still require an active paired origin.
    let allowed = (uri.path() == "/v1/health" && valid_unpaired_health_origin(&origin))
        || state.auth.as_ref().is_some_and(|auth| {
            auth.is_paired_origin(&origin, chrono::Utc::now())
                .unwrap_or(false)
        });
    if !allowed {
        return unauthorized();
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let output = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&origin) {
        output.insert("access-control-allow-origin", value);
    }
    output.insert("vary", HeaderValue::from_static("Origin"));
    output.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("GET, HEAD, POST, DELETE, OPTIONS"),
    );
    output.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("Authorization, DPoP, Content-Type"),
    );
    response
}

fn pairing_preflight(origin: &str, headers: &HeaderMap) -> Response {
    let requested_method = headers
        .get("access-control-request-method")
        .and_then(|value| value.to_str().ok());
    let requested_headers = headers
        .get("access-control-request-headers")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect::<std::collections::BTreeSet<_>>()
        });
    if !valid_pairing_origin(origin)
        || requested_method != Some("POST")
        || requested_headers.as_ref().is_none_or(|headers| {
            headers != &std::collections::BTreeSet::from(["content-type".to_owned()])
        })
    {
        return unauthorized();
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let output = response.headers_mut();
    output.insert(
        "access-control-allow-origin",
        HeaderValue::from_str(origin).expect("validated origin is a valid header value"),
    );
    output.insert("vary", HeaderValue::from_static("Origin"));
    output.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST"),
    );
    output.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("Content-Type"),
    );
    response
}

fn valid_pairing_origin(origin: &str) -> bool {
    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }
    match parsed.scheme() {
        "https" => true,
        "http" => parsed
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback()),
        _ => false,
    }
}

/// The hosted Commander is the only unpaired browser origin allowed to learn
/// hub readiness. `valid_pairing_origin` deliberately accepts arbitrary HTTPS
/// origins for an explicit pairing ceremony, which is broader than a liveness
/// read may be.
fn valid_unpaired_health_origin(origin: &str) -> bool {
    valid_pairing_origin(origin) && origin == "https://hub.petrastella.io"
}

async fn health<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    let response = Json(HealthResponse::ready()).into_response();
    // Health remains available to curl and local readiness checks. A browser
    // may read it cross-origin when it is the reviewed hosted Commander,
    // even before that origin has an active pairing. Existing paired origins
    // retain their previous health-read behavior.
    let cors_allowed = origin(&headers).is_some_and(|origin| {
        valid_unpaired_health_origin(&origin)
            || state.auth.as_ref().is_some_and(|auth| {
                auth.is_paired_origin(&origin, chrono::Utc::now())
                    .unwrap_or(false)
            })
    });
    if cors_allowed {
        with_cors(response, &headers)
    } else {
        response
    }
}

#[derive(Serialize)]
struct MachineResponse {
    schema_version: u32,
    machine_id: String,
    version: &'static str,
    capabilities: &'static [&'static str],
    transport: super::MachineTransport,
    cloud_devices: Vec<super::CloudDeviceSuggestion>,
}

async fn machine<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/machine",
    )
    .is_err()
    {
        return unauthorized();
    }
    with_cors(
        Json(MachineResponse {
            schema_version: super::HUB_SCHEMA_VERSION,
            machine_id: state.machine.id,
            version: env!("CARGO_PKG_VERSION"),
            capabilities: &[
                "session_index",
                "daemon_attach",
                "machine_events",
                "machine_multiplex_v2",
                "tailscale_serve",
                "cloud_device_suggestions",
            ],
            transport: state.metadata.transport,
            cloud_devices: state.metadata.cloud_devices,
        })
        .into_response(),
        &headers,
    )
}

async fn diagnostics<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/diagnostics",
    )
    .is_err()
    {
        return unauthorized();
    }
    let tailscale = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::task::spawn_blocking(|| {
            Command::new("tailscale")
                .args(["status", "--json"])
                .output()
        }),
    )
    .await;
    let tailscale = match tailscale {
        Ok(Ok(Ok(output))) if output.status.success() => {
            serde_json::from_slice::<serde_json::Value>(&output.stdout)
                .unwrap_or_else(|_| serde_json::json!({"error":"tailscale returned invalid JSON"}))
        }
        Ok(Ok(Ok(output))) => {
            serde_json::json!({"error": format!("tailscale status exited {}", output.status)})
        }
        Ok(Ok(Err(_))) => serde_json::json!({"error":"tailscale CLI unavailable"}),
        Ok(Err(_)) => serde_json::json!({"error":"tailscale status worker failed"}),
        Err(_) => serde_json::json!({"error":"tailscale status timed out after 3s"}),
    };
    let session_count = state
        .catalog
        .list()
        .await
        .map(|items| items.len())
        .unwrap_or_default();
    with_cors(
        Json(serde_json::json!({
            "target_node": state.machine.id,
            "tailscale_status": tailscale,
            "daemon_health": {"status":"ready", "sessions":session_count},
            "checked_at": chrono::Utc::now(),
        }))
        .into_response(),
        &headers,
    )
}

#[derive(Serialize)]
struct SessionsResponse {
    schema_version: u32,
    sessions: Vec<HubSession>,
}

async fn sessions<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        "/v1/sessions",
    )
    .is_err()
    {
        return unauthorized();
    }
    match state.catalog.list().await {
        Ok(sessions) => with_cors(
            Json(SessionsResponse {
                schema_version: super::HUB_SCHEMA_VERSION,
                sessions,
            })
            .into_response(),
            &headers,
        ),
        Err(error) => internal_error(error),
    }
}

async fn events<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        "/v1/events",
    )
    .is_err()
    {
        return unauthorized();
    }
    // Subscribe before snapshotting. A concurrent event can consequently be
    // replayed once and then observed live once; sequence+revision make that a
    // harmless idempotent upsert, while the ordering avoids a lost-event gap.
    let receiver = state.events.subscribe();
    let replay = stream::iter(
        state
            .events
            .history()
            .into_iter()
            .map(|event| Ok::<Event, Infallible>(machine_event_sse(event))),
    );
    let live = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    return Some((Ok::<Event, Infallible>(machine_event_sse(event)), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let output = replay.chain(live);
    with_cors(
        Sse::new(output)
            .keep_alive(KeepAlive::default())
            .into_response(),
        &headers,
    )
}

fn machine_event_sse(event: super::MachineEvent) -> Event {
    Event::default()
        .id(format!("{}.{}", event.sequence, event.revision))
        .event(format!("{:?}", event.kind).to_lowercase())
        .json_data(event)
        .expect("MachineEvent serialization is infallible")
}

async fn status<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/sessions/{session}/status");
    if authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        &uri,
    )
    .is_err()
    {
        return unauthorized();
    }
    match tokio::task::spawn_blocking(move || {
        let session = crate::bridge::server::session::resolve_session_by_name(&session)?;
        let root =
            crate::bridge::server::session::cas_root_for_session_with_fallback(&session, None)?;
        crate::bridge::server::session::build_status_json(&session, &root, 20)
    })
    .await
    {
        Ok(Ok(status)) => with_cors(Json(status).into_response(), &headers),
        Ok(Err(_)) => generic_not_found(),
        Err(error) => internal_error(error.into()),
    }
}

#[derive(Debug, Default, Deserialize)]
struct AttachQuery {
    #[serde(default)]
    panes: String,
    #[serde(default)]
    ticket: String,
}

async fn attach<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    Query(query): Query<AttachQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let origin = origin(&headers);
    let endpoint = format!("/v1/sessions/{session}/attach");
    let socket_auth = if let Some(auth) = &state.auth {
        let Some(origin) = origin.as_deref() else {
            return unauthorized();
        };
        match auth.consume_ws_ticket(
            &query.ticket,
            origin,
            &session,
            &endpoint,
            chrono::Utc::now(),
        ) {
            Ok(context) if context.has(Scope::PaneRead) => Some((auth.clone(), context)),
            _ => return unauthorized(),
        }
    } else {
        if !authorized(&state, HubAction::PaneRead, &headers) {
            return unauthorized();
        }
        None
    };
    let sessions = match state.catalog.list().await {
        Ok(sessions) => sessions,
        Err(error) => return internal_error(error),
    };
    let Some(candidate) = sessions
        .into_iter()
        .find(|candidate| candidate.name == session)
    else {
        return generic_not_found();
    };
    let Some(port) = candidate.ws_port else {
        return generic_not_found();
    };
    let daemon_identity = candidate.daemon_identity;
    let panes: Vec<String> = query
        .panes
        .split(',')
        .filter(|pane| !pane.is_empty())
        .map(str::to_owned)
        .collect();
    let connector = state.connector.clone();
    upgrade
        .on_upgrade(move |socket| {
            proxy_socket(
                socket,
                connector,
                session,
                port,
                panes,
                daemon_identity,
                socket_auth,
            )
        })
        .into_response()
}

async fn proxy_socket(
    socket: WebSocket,
    connector: DaemonConnector,
    session: String,
    port: u16,
    panes: Vec<String>,
    daemon_identity: Option<super::DaemonIdentity>,
    auth: Option<(AuthStore, AuthContext)>,
) {
    let Ok(mut viewer) = connector
        .attach(&session, port, panes, daemon_identity)
        .await
    else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    let mut revocations = auth
        .as_ref()
        .map(|(store, _)| store.subscribe_revocations());
    let revalidation_period = std::time::Duration::from_millis(250);
    let mut revalidation = tokio::time::interval_at(
        tokio::time::Instant::now() + revalidation_period,
        revalidation_period,
    );
    revalidation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            frame = viewer.recv() => match frame {
                Ok(frame) => {
                    audit_refused_pane_resize(&auth, &session, &frame.bytes);
                    if sink.send(Message::Binary(frame.bytes.into())).await.is_err() {
                        break;
                    }
                }
                Err(ViewerRecvError::Lagged { skipped }) => {
                    let error = serde_json::json!({"error":"viewer_lagged","skipped":skipped});
                    let _ = sink.send(Message::Text(error.to_string().into())).await;
                    break;
                }
                Err(ViewerRecvError::Closed) => break,
            },
            incoming = source.next() => match incoming {
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Text(text))) => {
                    if handle_client_message(&connector, &session, &auth, text.as_bytes()).await.is_err() {
                        let _ = sink.send(Message::Text(r#"{"error":"forbidden"}"#.into())).await;
                    }
                }
                Some(Ok(Message::Binary(bytes))) => {
                    if handle_client_message(&connector, &session, &auth, &bytes).await.is_err() {
                        let _ = sink.send(Message::Text(r#"{"error":"forbidden"}"#.into())).await;
                    }
                }
            },
            revoked = async {
                match revocations.as_mut() {
                    Some(receiver) => receiver.recv().await.ok(),
                    None => futures_util::future::pending().await,
                }
            } => {
                if revoked.as_deref() == auth.as_ref().map(|(_, context)| context.device_id.as_str()) {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
            revoked_on_disk = async {
                let Some((store, context)) = auth.as_ref() else {
                    return futures_util::future::pending::<bool>().await;
                };
                revalidation.tick().await;
                let store = store.clone();
                let context = context.clone();
                tokio::task::spawn_blocking(move || {
                    store
                        .ensure_active_context(&context, chrono::Utc::now())
                        .is_err()
                })
                .await
                .unwrap_or(true)
            } => {
                if revoked_on_disk {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

async fn handle_client_message(
    connector: &DaemonConnector,
    session: &str,
    auth: &Option<(AuthStore, AuthContext)>,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let (store, context) = auth.as_ref().context("authentication required")?;
    let mut message: ClientMessage = serde_json::from_slice(bytes)?;
    let scope = required_scope(&message).context("operation is not exposed by Commander")?;
    let now = chrono::Utc::now();
    let read_message = is_pane_read_message(&message);
    let allowed = if matches!(message, ClientMessage::ResizePane { .. }) {
        store.may_resize_panes(context, session, now)?
    } else if read_message {
        context.has(Scope::PaneRead)
    } else {
        context.has(scope) && store.has_active_lease(context, session, now)?
    };
    if !allowed {
        store.audit(
            Some(context),
            "denied",
            if read_message {
                "websocket_read"
            } else {
                "websocket_mutation"
            },
            Some(scope),
            Some(session),
            now,
        )?;
        anyhow::bail!("authorization refused")
    }
    if let ClientMessage::SendMessage { attribution, .. } = &mut message {
        *attribution = MessageAttribution {
            device_id: Some(context.device_id.clone()),
            credential_id: Some(context.credential_id.clone()),
            device_label: Some(context.device_label.clone()),
            operator_label: Some(context.operator_label.clone()),
            controller_origin: Some(context.controller_origin.clone()),
            request_id: Some(context.request_id.clone()),
        };
    }
    connector.send(session, message).await?;
    store.audit(
        Some(context),
        "allowed",
        if read_message {
            "websocket_read"
        } else {
            "websocket_mutation"
        },
        Some(scope),
        Some(session),
        now,
    )
}

pub(crate) fn is_pane_read_message(message: &ClientMessage) -> bool {
    matches!(
        message,
        ClientMessage::RequestPaneKeyframe { .. } | ClientMessage::ScrollbackRequest { .. }
    )
}

async fn pairing_exchange<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(mut exchange): Json<PairingExchange>,
) -> Response {
    let Some(auth) = &state.auth else {
        return unauthorized();
    };
    if origin(&headers).as_deref() != Some(exchange.controller_origin.as_str()) {
        return unauthorized();
    }
    let bound_origin = auth
        .pairing_exchange_matches(
            &exchange.token,
            &exchange.hub_id,
            &exchange.controller_origin,
        )
        .unwrap_or(false);
    exchange.source = exchange.controller_origin.clone();
    match auth.exchange_pairing(exchange, chrono::Utc::now()) {
        Ok(credential) => with_cors(Json(credential).into_response(), &headers),
        Err(PairingExchangeError::Throttled {
            retry_after_seconds,
        }) if bound_origin => with_cors(pairing_throttled(retry_after_seconds), &headers),
        Err(_) if bound_origin => with_cors(unauthorized(), &headers),
        Err(_) => unauthorized(),
    }
}

fn pairing_throttled(retry_after_seconds: u64) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({"error":"slow_down"})),
    )
        .into_response();
    response.headers_mut().insert(
        "retry-after",
        HeaderValue::from_str(&retry_after_seconds.to_string())
            .expect("a decimal retry delay is a valid header value"),
    );
    response.headers_mut().insert(
        "access-control-expose-headers",
        HeaderValue::from_static("Retry-After"),
    );
    response
}

async fn refresh_credential<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    let Some(auth) = &state.auth else {
        return unauthorized();
    };
    let Some(request_origin) = origin(&headers) else {
        return unauthorized();
    };
    let Some(authorization) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    else {
        return unauthorized();
    };
    let Some(proof) = headers.get("dpop").and_then(|value| value.to_str().ok()) else {
        return unauthorized();
    };
    match auth.refresh_device_credential(
        authorization,
        proof,
        &request_origin,
        "POST",
        "/v1/auth/refresh",
        chrono::Utc::now(),
    ) {
        Ok(credential) => with_cors(Json(credential).into_response(), &headers),
        Err(_) => with_cors(unauthorized(), &headers),
    }
}

#[derive(Deserialize)]
struct TicketRequest {
    #[serde(default)]
    session: Option<String>,
}

async fn websocket_ticket<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(request): Json<TicketRequest>,
) -> Response {
    let uri = "/v1/auth/websocket-ticket";
    let context = match authorize(
        &state,
        HubAction::PaneRead,
        Scope::PaneRead,
        &headers,
        "POST",
        uri,
    ) {
        Ok(Some(context)) => context,
        _ => return unauthorized(),
    };
    let (session, endpoint) = match request.session {
        Some(session) => {
            let endpoint = format!("/v1/sessions/{session}/attach");
            (session, endpoint)
        }
        None => ("*".to_owned(), "/v1/attach".to_owned()),
    };
    match state.auth.as_ref().unwrap().issue_ws_ticket(
        &context,
        &session,
        &endpoint,
        chrono::Utc::now(),
    ) {
        Ok(ticket) => with_cors(
            Json(serde_json::json!({"ticket":ticket.ticket,"expires_at":ticket.expires_at}))
                .into_response(),
            &headers,
        ),
        Err(_) => unauthorized(),
    }
}

async fn machine_attach<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Query(query): Query<AttachQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let origin = origin(&headers);
    let endpoint = "/v1/attach";
    let socket_auth = if let Some(auth) = &state.auth {
        let Some(origin) = origin.as_deref() else {
            return unauthorized();
        };
        match auth.consume_ws_ticket(&query.ticket, origin, "*", endpoint, chrono::Utc::now()) {
            Ok(context) if context.has(Scope::PaneRead) => Some((auth.clone(), context)),
            _ => return unauthorized(),
        }
    } else {
        if !authorized(&state, HubAction::PaneRead, &headers) {
            return unauthorized();
        }
        None
    };
    upgrade
        .on_upgrade(move |socket| proxy_machine_socket(socket, state, socket_auth))
        .into_response()
}

#[derive(Debug)]
enum MachineOutbound {
    Frame { session: String, frame: ProxyFrame },
    Lagged { session: String, skipped: u64 },
    Closed { session: String },
}

#[derive(Debug, Deserialize)]
struct MachineClientEnvelope {
    channel: String,
    #[serde(default)]
    subscribe: bool,
    #[serde(default)]
    panes: Vec<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    message: Option<serde_json::Value>,
    #[serde(default)]
    ping: Option<u64>,
}

/// A daemon reply saying the operator's local dashboard owns this pane's
/// geometry, so the viewer's `ResizePane` was refused (cas-37f8).
///
/// The prefix check keeps this off the hot relay path: `DaemonMessage` is an
/// externally tagged enum, so only a `PaneSize` frame is ever parsed.
pub(super) fn refused_pane_resize(bytes: &[u8]) -> Option<(String, u16, u16)> {
    if !bytes.starts_with(br#"{"PaneSize""#) {
        return None;
    }
    match serde_json::from_slice::<DaemonMessage>(bytes).ok()? {
        DaemonMessage::PaneSize {
            pane_id,
            cols,
            rows,
            authority: PaneSizeAuthority::LocalDashboard,
        } => Some((pane_id, cols, rows)),
        _ => None,
    }
}

/// Record a refused viewer resize in the hub audit log, attributed to the
/// device that asked for it.
fn audit_refused_pane_resize(
    auth: &Option<(AuthStore, AuthContext)>,
    session: &str,
    bytes: &[u8],
) {
    let Some((store, context)) = auth.as_ref() else {
        return;
    };
    let Some((pane_id, cols, rows)) = refused_pane_resize(bytes) else {
        return;
    };
    tracing::info!(
        session,
        pane = %pane_id,
        cols,
        rows,
        device = %context.device_label,
        "refused a Commander viewer's pane resize: the local dashboard owns this geometry"
    );
    let _ = store.audit(
        Some(context),
        "refused",
        "websocket_pane_resize",
        Some(Scope::PaneRead),
        Some(session),
        chrono::Utc::now(),
    );
}

fn machine_binary_frame(session: &str, frame: &ProxyFrame) -> anyhow::Result<Option<Vec<u8>>> {
    let (kind, pane_id, payload) = match frame.kind {
        ProxyFrameKind::Output => {
            let DaemonMessage::Output { pane_id, data } =
                serde_json::from_slice::<DaemonMessage>(&frame.bytes)?
            else {
                anyhow::bail!("output frame kind did not contain Output")
            };
            (1_u8, pane_id, data)
        }
        ProxyFrameKind::PaneKeyframe => {
            let DaemonMessage::PaneKeyframe { pane_id, ansi, .. } =
                serde_json::from_slice::<DaemonMessage>(&frame.bytes)?
            else {
                anyhow::bail!("keyframe frame kind did not contain PaneKeyframe")
            };
            (2_u8, pane_id, ansi)
        }
        ProxyFrameKind::Other => return Ok(None),
    };
    let session_len =
        u16::try_from(session.len()).context("session name exceeds protocol limit")?;
    let pane_len = u16::try_from(pane_id.len()).context("pane id exceeds protocol limit")?;
    let mut encoded = Vec::with_capacity(9 + session.len() + pane_id.len() + payload.len());
    encoded.extend_from_slice(MACHINE_PROTOCOL_MAGIC);
    encoded.push(kind);
    encoded.extend_from_slice(&session_len.to_be_bytes());
    encoded.extend_from_slice(&pane_len.to_be_bytes());
    encoded.extend_from_slice(session.as_bytes());
    encoded.extend_from_slice(pane_id.as_bytes());
    encoded.extend_from_slice(&payload);
    Ok(Some(encoded))
}

async fn proxy_machine_socket<R: SessionReadModel>(
    mut socket: WebSocket,
    state: HubState<R>,
    auth: Option<(AuthStore, AuthContext)>,
) {
    let handshake = tokio::time::timeout(Duration::from_secs(3), socket.recv()).await;
    let received_proto = match handshake {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| value.get("proto").and_then(serde_json::Value::as_u64)),
        _ => None,
    };
    if received_proto != Some(u64::from(MACHINE_PROTOCOL_VERSION)) {
        let error = serde_json::json!({
            "error": {
                "code": "protocol_mismatch",
                "supported": MACHINE_PROTOCOL_VERSION,
                "received": received_proto,
            }
        });
        let _ = socket.send(Message::Text(error.to_string().into())).await;
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    let hello = serde_json::json!({
        "proto": MACHINE_PROTOCOL_VERSION,
        "capabilities": ["pty_binary", "machine_multiplex", "keyframe_flow_control"]
    });
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let (mut sink, mut source) = socket.split();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel::<MachineOutbound>(64);
    let mut subscriptions = std::collections::HashMap::<String, tokio::task::JoinHandle<()>>::new();
    let mut machine_events = state.events.subscribe();
    let mut events_subscribed = false;
    let mut revocations = auth
        .as_ref()
        .map(|(store, _)| store.subscribe_revocations());
    let revalidation_period = Duration::from_millis(250);
    let mut revalidation = tokio::time::interval_at(
        tokio::time::Instant::now() + revalidation_period,
        revalidation_period,
    );
    revalidation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            outgoing = outbound_rx.recv() => match outgoing {
                Some(MachineOutbound::Frame { session, frame }) => {
                    audit_refused_pane_resize(&auth, &session, &frame.bytes);
                    let result = match machine_binary_frame(&session, &frame) {
                        Ok(Some(bytes)) => sink.send(Message::Binary(bytes.into())).await,
                        Ok(None) => {
                            let Ok(message) = serde_json::from_slice::<serde_json::Value>(&frame.bytes) else { break };
                            let envelope = serde_json::json!({"channel":format!("pty:{session}"),"message":message});
                            sink.send(Message::Text(envelope.to_string().into())).await
                        }
                        Err(_) => break,
                    };
                    if result.is_err() { break; }
                }
                Some(MachineOutbound::Lagged { session, skipped }) => {
                    let envelope = serde_json::json!({
                        "channel": format!("pty:{session}"),
                        "keyframe_required": {"skipped": skipped},
                    });
                    if sink.send(Message::Text(envelope.to_string().into())).await.is_err() { break; }
                }
                Some(MachineOutbound::Closed { session }) => {
                    subscriptions.remove(&session);
                    let envelope = serde_json::json!({"channel":format!("pty:{session}"),"closed":true});
                    if sink.send(Message::Text(envelope.to_string().into())).await.is_err() { break; }
                }
                None => break,
            },
            incoming = source.next() => match incoming {
                Some(Ok(Message::Ping(payload))) => {
                    if sink.send(Message::Pong(payload)).await.is_err() { break; }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Binary(_))) => {
                    let error = serde_json::json!({"error":{"code":"binary_client_frame","message":"Commander controls must be JSON"}});
                    if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                }
                Some(Ok(Message::Text(text))) => {
                    let Ok(envelope) = serde_json::from_str::<MachineClientEnvelope>(&text) else {
                        let error = serde_json::json!({"error":{"code":"invalid_frame","message":"invalid machine protocol frame"}});
                        if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                        continue;
                    };
                    if envelope.channel == "health" {
                        if let Some(ping) = envelope.ping {
                            let pong = serde_json::json!({"channel":"health","pong":ping});
                            if sink.send(Message::Text(pong.to_string().into())).await.is_err() { break; }
                        }
                        continue;
                    }
                    if envelope.channel == "events" && envelope.subscribe {
                        events_subscribed = true;
                        continue;
                    }
                    let channel_session = envelope.channel.strip_prefix("pty:").map(str::to_owned)
                        .or_else(|| (envelope.channel == "resize").then(|| envelope.session.clone()).flatten());
                    let Some(session) = channel_session else {
                        let error = serde_json::json!({"error":{"code":"unknown_channel","channel":envelope.channel}});
                        if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                        continue;
                    };
                    if envelope.subscribe {
                        if subscriptions.contains_key(&session) { continue; }
                        let candidate = state.catalog.list().await.ok().and_then(|sessions| {
                            sessions.into_iter().find(|candidate| candidate.name == session)
                        });
                        let Some(candidate) = candidate else {
                            let error = serde_json::json!({"channel":format!("pty:{session}"),"error":{"code":"session_not_found"}});
                            if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                            continue;
                        };
                        let Some(port) = candidate.ws_port else {
                            let error = serde_json::json!({"channel":format!("pty:{session}"),"error":{"code":"daemon_offline"}});
                            if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                            continue;
                        };
                        let Ok(mut viewer) = state.connector.attach(
                            &session,
                            port,
                            envelope.panes,
                            candidate.daemon_identity,
                        ).await else {
                            continue;
                        };
                        let tx = outbound_tx.clone();
                        let task_session = session.clone();
                        let handle = tokio::spawn(async move {
                            loop {
                                match viewer.recv().await {
                                    Ok(frame) => {
                                        if tx.send(MachineOutbound::Frame { session: task_session.clone(), frame }).await.is_err() { break; }
                                    }
                                    Err(ViewerRecvError::Lagged { skipped }) => {
                                        if tx.send(MachineOutbound::Lagged { session: task_session.clone(), skipped }).await.is_err() { break; }
                                    }
                                    Err(ViewerRecvError::Closed) => {
                                        let _ = tx.send(MachineOutbound::Closed { session: task_session.clone() }).await;
                                        break;
                                    }
                                }
                            }
                        });
                        subscriptions.insert(session, handle);
                        continue;
                    }
                    let Some(message) = envelope.message else { continue; };
                    let bytes = match serde_json::to_vec(&message) {
                        Ok(bytes) => bytes,
                        Err(_) => continue,
                    };
                    if handle_client_message(&state.connector, &session, &auth, &bytes).await.is_err() {
                        let error = serde_json::json!({"channel":format!("pty:{session}"),"error":{"code":"forbidden"}});
                        if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                    }
                }
            },
            event = async {
                if events_subscribed {
                    machine_events.recv().await.ok()
                } else {
                    futures_util::future::pending().await
                }
            } => {
                if let Some(event) = event {
                    let envelope = serde_json::json!({"channel":"events","event":event});
                    if sink.send(Message::Text(envelope.to_string().into())).await.is_err() { break; }
                }
            },
            revoked = async {
                match revocations.as_mut() {
                    Some(receiver) => receiver.recv().await.ok(),
                    None => futures_util::future::pending().await,
                }
            } => {
                if revoked.as_deref() == auth.as_ref().map(|(_, context)| context.device_id.as_str()) {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
            revoked_on_disk = async {
                let Some((store, context)) = auth.as_ref() else {
                    return futures_util::future::pending::<bool>().await;
                };
                revalidation.tick().await;
                let store = store.clone();
                let context = context.clone();
                tokio::task::spawn_blocking(move || {
                    store.ensure_active_context(&context, chrono::Utc::now()).is_err()
                }).await.unwrap_or(true)
            } => {
                if revoked_on_disk {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
    for (_, handle) in subscriptions {
        handle.abort();
    }
}

async fn acquire_lease<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
    Json(request): Json<LeaseRequest>,
) -> Response {
    let uri = format!("/v1/sessions/{session}/lease");
    let required_scope = if request.force {
        Scope::HubAdmin
    } else {
        Scope::PaneInput
    };
    let context = match authorize(
        &state,
        HubAction::Mutation,
        required_scope,
        &headers,
        "POST",
        &uri,
    ) {
        Ok(Some(context)) => context,
        _ => return unauthorized(),
    };
    match state.auth.as_ref().unwrap().acquire_or_force_lease(
        &context,
        &session,
        chrono::Utc::now(),
        request.force,
    ) {
        Ok(_) => {
            state.events.controller_changed(&session);
            match state
                .auth
                .as_ref()
                .unwrap()
                .lease_status(&context, &session, chrono::Utc::now())
            {
                Ok(summary) => with_cors(Json(summary).into_response(), &headers),
                Err(_) => unauthorized(),
            }
        }
        Err(_) => with_cors(
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error":"lease_unavailable"})),
            )
                .into_response(),
            &headers,
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
struct LeaseRequest {
    #[serde(default)]
    force: bool,
}

async fn lease_status<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/sessions/{session}/lease");
    let context = match authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        &uri,
    ) {
        Ok(Some(context)) => context,
        _ => return unauthorized(),
    };
    match state
        .auth
        .as_ref()
        .unwrap()
        .lease_status(&context, &session, chrono::Utc::now())
    {
        Ok(summary) => with_cors(Json(summary).into_response(), &headers),
        Err(_) => unauthorized(),
    }
}

async fn release_lease<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/sessions/{session}/lease");
    let context = match authorize(
        &state,
        HubAction::Mutation,
        Scope::PaneInput,
        &headers,
        "DELETE",
        &uri,
    ) {
        Ok(Some(context)) => context,
        _ => return unauthorized(),
    };
    match state
        .auth
        .as_ref()
        .unwrap()
        .release_lease(&context, &session, chrono::Utc::now())
    {
        Ok(()) => {
            state.events.controller_changed(&session);
            with_cors(StatusCode::NO_CONTENT.into_response(), &headers)
        }
        Err(_) => unauthorized(),
    }
}

fn authorize<R: SessionReadModel>(
    state: &HubState<R>,
    action: HubAction,
    scope: Scope,
    headers: &HeaderMap,
    method: &str,
    target_uri: &str,
) -> anyhow::Result<Option<AuthContext>> {
    let origin = request_origin(state, action, headers, method)?;
    if let Some(auth) = &state.auth {
        let authorization = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .context("authorization required")?;
        let proof = headers
            .get("dpop")
            .and_then(|v| v.to_str().ok())
            .context("proof required")?;
        let context = auth.authenticate_dpop(
            authorization,
            proof,
            &origin,
            method,
            target_uri,
            chrono::Utc::now(),
        )?;
        anyhow::ensure!(context.has(scope), "scope denied");
        Ok(Some(context))
    } else if state
        .authorizer
        .authorize(&HubRequest {
            action,
            origin: Some(origin),
        })
        .is_allowed()
    {
        Ok(None)
    } else {
        anyhow::bail!("unauthorized")
    }
}

fn request_origin<R: SessionReadModel>(
    state: &HubState<R>,
    action: HubAction,
    headers: &HeaderMap,
    method: &str,
) -> anyhow::Result<String> {
    if let Some(origin) = origin(headers) {
        return Ok(origin);
    }
    anyhow::ensure!(
        action != HubAction::Mutation && matches!(method, "GET" | "HEAD"),
        "origin required"
    );
    anyhow::ensure!(
        headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            == Some("same-origin"),
        "origin required"
    );
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .context("host required")?;
    state
        .effective_origins
        .iter()
        .find(|effective| {
            url::Url::parse(effective)
                .ok()
                .is_some_and(|parsed| format!("{}://{host}", parsed.scheme()) == **effective)
        })
        .cloned()
        .context("effective origin mismatch")
}

fn origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn with_cors(mut response: Response, request_headers: &HeaderMap) -> Response {
    if let Some(value) =
        origin(request_headers).and_then(|origin| HeaderValue::from_str(&origin).ok())
    {
        response
            .headers_mut()
            .insert("access-control-allow-origin", value);
        response
            .headers_mut()
            .insert("vary", HeaderValue::from_static("Origin"));
    }
    response
}

fn authorized<R: SessionReadModel>(
    state: &HubState<R>,
    action: HubAction,
    headers: &HeaderMap,
) -> bool {
    let origin = request_origin(state, action, headers, "GET").ok();
    state
        .authorizer
        .authorize(&HubRequest { action, origin })
        .is_allowed()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error":"unauthorized"})),
    )
        .into_response()
}

fn generic_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error":"not_found"})),
    )
        .into_response()
}

fn internal_error(error: anyhow::Error) -> Response {
    tracing::warn!(%error, "Commander hub read failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error":"internal_error"})),
    )
        .into_response()
}

#[cfg(test)]
mod machine_protocol_tests {
    use super::*;
    use crate::hub::proxy_frame;

    #[test]
    fn proto_2_binary_output_keeps_terminal_bytes_raw_after_the_route_header() {
        let payload = vec![0, 255, 0x1b, b'[', b'H', b'o', b'k'];
        let encoded = machine_binary_frame(
            "factory-a",
            &proxy_frame(DaemonMessage::Output {
                pane_id: "supervisor".into(),
                data: payload.clone(),
            }),
        )
        .unwrap()
        .unwrap();

        assert_eq!(&encoded[..4], MACHINE_PROTOCOL_MAGIC);
        assert_eq!(encoded[4], 1);
        assert_eq!(u16::from_be_bytes([encoded[5], encoded[6]]), 9);
        assert_eq!(u16::from_be_bytes([encoded[7], encoded[8]]), 10);
        assert_eq!(&encoded[9..18], b"factory-a");
        assert_eq!(&encoded[18..28], b"supervisor");
        assert_eq!(&encoded[28..], payload);
        assert!(!encoded.windows(6).any(|window| window == b"Output"));
    }

    #[test]
    fn non_pty_machine_messages_remain_on_the_json_channel() {
        let frame = proxy_frame(DaemonMessage::Pong);
        assert!(machine_binary_frame("factory-a", &frame).unwrap().is_none());
    }
}
