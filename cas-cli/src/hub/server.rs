use std::convert::Infallible;
use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, options, post};
use futures_util::{SinkExt, StreamExt, stream};
use serde::{Deserialize, Serialize};

use super::{
    AuthContext, AuthStore, DaemonConnector, HealthResponse, HubAction, HubAuthorizer, HubRequest,
    HubSession, MachineEventBus, MachineIdentity, PairingExchange, Scope, SessionCatalog,
    SessionReadModel, ViewerRecvError, required_scope,
};
use crate::ui::factory::{ClientMessage, MessageAttribution};

#[derive(Clone)]
pub struct HubState<R: SessionReadModel> {
    catalog: SessionCatalog<R>,
    authorizer: Arc<dyn HubAuthorizer>,
    machine: MachineIdentity,
    connector: DaemonConnector,
    events: MachineEventBus,
    auth: Option<AuthStore>,
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
        }
    }

    pub fn with_auth(mut self, auth: AuthStore) -> Self {
        self.auth = Some(auth);
        self
    }
}

pub fn router<R: SessionReadModel>(state: HubState<R>) -> Router {
    Router::new()
        .route("/", get(commander_index))
        .route("/commander", get(commander_index))
        .route("/commander/", get(commander_index))
        .route("/commander/app.js", get(commander_javascript))
        .route("/commander/app.css", get(commander_stylesheet))
        .route("/commander/ghostty-vt.wasm", get(commander_ghostty_wasm))
        .route(
            "/commander/ghostty-write-pty.wasm",
            get(commander_ghostty_write_wasm),
        )
        .route("/commander/symbols.woff2", get(commander_symbols_font))
        .route("/v1/health", get(health))
        .route("/v1/auth/pairing/exchange", post(pairing_exchange::<R>))
        .route("/v1/auth/websocket-ticket", post(websocket_ticket::<R>))
        .route("/v1/machine", get(machine::<R>))
        .route("/v1/sessions", get(sessions::<R>))
        .route("/v1/events", get(events::<R>))
        .route("/v1/sessions/{session}/status", get(status::<R>))
        .route(
            "/v1/sessions/{session}/lease",
            get(lease_status::<R>)
                .post(acquire_lease::<R>)
                .delete(release_lease::<R>),
        )
        .route("/v1/sessions/{session}/attach", get(attach::<R>))
        .route("/{*path}", options(preflight::<R>))
        .with_state(state)
        .layer(middleware::from_fn(security_headers))
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

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self' https: wss: http://127.0.0.1:* ws://127.0.0.1:*; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'; worker-src 'none'; manifest-src 'self'"),
    );
    response
}

async fn preflight<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    let Some(origin) = origin(&headers) else {
        return unauthorized();
    };
    let allowed = state.auth.as_ref().is_some_and(|auth| {
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

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse::ready())
}

#[derive(Serialize)]
struct MachineResponse {
    schema_version: u32,
    machine_id: String,
    version: &'static str,
    capabilities: &'static [&'static str],
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
            capabilities: &["session_index", "daemon_attach", "machine_events"],
        })
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
    let receiver = state.events.subscribe();
    let output = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let item = Event::default()
                        .id(event.sequence.to_string())
                        .event(format!("{:?}", event.kind).to_lowercase())
                        .json_data(event)
                        .expect("MachineEvent serialization is infallible");
                    return Some((Ok::<Event, Infallible>(item), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    with_cors(
        Sse::new(output)
            .keep_alive(KeepAlive::default())
            .into_response(),
        &headers,
    )
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
    let Some(port) = sessions
        .into_iter()
        .find(|candidate| candidate.name == session)
        .and_then(|candidate| candidate.ws_port)
    else {
        return generic_not_found();
    };
    let panes: Vec<String> = query
        .panes
        .split(',')
        .filter(|pane| !pane.is_empty())
        .map(str::to_owned)
        .collect();
    let connector = state.connector.clone();
    upgrade
        .on_upgrade(move |socket| {
            proxy_socket(socket, connector, session, port, panes, socket_auth)
        })
        .into_response()
}

async fn proxy_socket(
    socket: WebSocket,
    connector: DaemonConnector,
    session: String,
    port: u16,
    panes: Vec<String>,
    auth: Option<(AuthStore, AuthContext)>,
) {
    let Ok(mut viewer) = connector.attach(&session, port, panes).await else {
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
    if !context.has(scope) || !store.has_active_lease(context, session, now)? {
        store.audit(
            Some(context),
            "denied",
            "websocket_mutation",
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
        "websocket_mutation",
        Some(scope),
        Some(session),
        now,
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
    exchange.source = exchange.controller_origin.clone();
    match auth.exchange_pairing(exchange, chrono::Utc::now()) {
        Ok(credential) => with_cors(Json(credential).into_response(), &headers),
        Err(_) => unauthorized(),
    }
}

#[derive(Deserialize)]
struct TicketRequest {
    session: String,
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
    let endpoint = format!("/v1/sessions/{}/attach", request.session);
    match state.auth.as_ref().unwrap().issue_ws_ticket(
        &context,
        &request.session,
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
    if let Some(auth) = &state.auth {
        let origin = origin(headers).context("origin required")?;
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
    } else if authorized(state, action, headers) {
        Ok(None)
    } else {
        anyhow::bail!("unauthorized")
    }
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
    let origin = headers
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
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
